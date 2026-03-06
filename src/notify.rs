/// Notification dispatcher.
///
/// Supports:
/// - Generic JSON webhook URLs (works with Slack, Discord, Teams, Gotify, etc.)
/// - Dedicated Slack incoming-webhook (with channel/username overrides)
/// - Gotify push notifications
/// - SMTP email
use std::sync::Arc;

use reqwest::Client;
use tracing::{error, info, warn};

use crate::config::NotificationConfig;
use crate::watcher::SessionReport;

pub struct Notifier {
    cfg: NotificationConfig,
    http: Client,
}

impl Notifier {
    pub fn new(cfg: NotificationConfig) -> Arc<Self> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Arc::new(Self { cfg, http })
    }

    /// Send a session-report notification if there is anything noteworthy.
    pub async fn send_report(&self, report: &SessionReport) {
        if report.updated.is_empty() && report.failed.is_empty() {
            return; // nothing to report
        }

        let msg = self.format_report(report);
        self.dispatch(&msg).await;
    }

    /// Send an arbitrary text message on all configured channels.
    #[allow(dead_code)]
    pub async fn send(&self, msg: &str) {
        self.dispatch(msg).await;
    }

    // ── internals ────────────────────────────────────────────────────────────

    fn format_report(&self, r: &SessionReport) -> String {
        let mut lines = vec![format!(
            "[updock] {} scanned, {} updated, {} failed",
            r.scanned,
            r.updated.len(),
            r.failed.len()
        )];
        for name in &r.updated {
            lines.push(format!("  ✓ updated: {}", name));
        }
        for name in &r.failed {
            lines.push(format!("  ✗ failed:  {}", name));
        }
        for name in &r.skipped {
            lines.push(format!("  - skipped: {}", name));
        }
        lines.join("\n")
    }

    async fn dispatch(&self, msg: &str) {
        // Generic webhook URLs
        for url in &self.cfg.urls {
            self.post_generic_webhook(url, msg).await;
        }

        // Dedicated Slack
        if let Some(ref url) = self.cfg.slack_webhook_url {
            self.post_slack(url, msg).await;
        }

        // Gotify
        if let (Some(url), Some(token)) =
            (&self.cfg.gotify_url, &self.cfg.gotify_token)
        {
            self.post_gotify(url, token, msg).await;
        }

        // Email
        if let Some(ref email_cfg) = self.cfg.email {
            if let Err(e) = self.send_email(email_cfg, msg).await {
                error!(err = %e, "email notification failed");
            }
        }
    }

    /// Generic webhook – POST JSON `{"text": "<msg>"}`.
    async fn post_generic_webhook(&self, url: &str, msg: &str) {
        let payload = serde_json::json!({ "text": msg });
        match self.http.post(url).json(&payload).send().await {
            Ok(r) if r.status().is_success() => {
                info!(url = %url, "generic webhook notification sent");
            }
            Ok(r) => {
                warn!(url = %url, status = %r.status(), "generic webhook returned non-success");
            }
            Err(e) => {
                error!(url = %url, err = %e, "generic webhook request failed");
            }
        }
    }

    /// Slack-specific payload (supports `username`, `channel`, `icon_emoji`).
    async fn post_slack(&self, url: &str, msg: &str) {
        let mut payload = serde_json::json!({
            "text": msg,
            "username": self.cfg.slack_username.as_deref().unwrap_or("updock"),
        });
        if let Some(ref ch) = self.cfg.slack_channel {
            payload["channel"] = serde_json::json!(ch);
        }
        match self.http.post(url).json(&payload).send().await {
            Ok(r) if r.status().is_success() => info!("Slack notification sent"),
            Ok(r) => warn!(status = %r.status(), "Slack returned non-success"),
            Err(e) => error!(err = %e, "Slack request failed"),
        }
    }

    /// Gotify push notification.
    async fn post_gotify(&self, base_url: &str, token: &str, msg: &str) {
        let url = format!("{}/message?token={}", base_url.trim_end_matches('/'), token);
        let payload = serde_json::json!({
            "title": "updock",
            "message": msg,
            "priority": 5,
        });
        match self.http.post(&url).json(&payload).send().await {
            Ok(r) if r.status().is_success() => info!("Gotify notification sent"),
            Ok(r) => warn!(status = %r.status(), "Gotify returned non-success"),
            Err(e) => error!(err = %e, "Gotify request failed"),
        }
    }

    /// SMTP email notification via lettre.
    async fn send_email(
        &self,
        cfg: &crate::config::EmailConfig,
        body: &str,
    ) -> anyhow::Result<()> {
        use lettre::{
            message::header::ContentType, transport::smtp::authentication::Credentials,
            AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
        };

        for to_addr in &cfg.to {
            let email = Message::builder()
                .from(cfg.from.parse()?)
                .to(to_addr.parse()?)
                .subject("[updock] Container update report")
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_string())?;

            // Build the SMTP transport, honouring tls_skip_verify.
            let mut builder = if cfg.tls_skip_verify {
                // Danger mode: accept any TLS certificate (dev/testing only).
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&cfg.smtp_host)
                    .port(cfg.smtp_port)
            } else {
                // Proper STARTTLS with certificate validation.
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.smtp_host)
                    .map_err(|e| anyhow::anyhow!("smtp relay setup: {}", e))?
                    .port(cfg.smtp_port)
            };

            if let (Some(user), Some(pass)) = (&cfg.username, &cfg.password) {
                builder = builder.credentials(Credentials::new(user.clone(), pass.clone()));
            }

            let mailer = builder.build();
            mailer.send(email).await.map_err(|e| anyhow::anyhow!("smtp send: {}", e))?;
            info!(to = %to_addr, "email notification sent");
        }
        Ok(())
    }
}
