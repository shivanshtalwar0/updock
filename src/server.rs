use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tracing::{error, info, warn};

use tokio::sync::mpsc;

use crate::{
    compose::{self, DeployResult},
    config::Config,
    harbor::HarborEvent,
    matcher::matching_projects,
    metrics::Metrics,
    notify::Notifier,
};

// ──────────────────────────────────────────────────────────────────────────────
// Shared application state
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub metrics: Arc<Metrics>,
    pub notifier: Arc<Notifier>,
    /// Optional channel to request an immediate watcher scan.
    pub update_tx: Option<mpsc::Sender<()>>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Router
// ──────────────────────────────────────────────────────────────────────────────

pub fn build_router(
    config: Arc<Config>,
    metrics: Arc<Metrics>,
    notifier: Arc<Notifier>,
    update_tx: Option<mpsc::Sender<()>>,
) -> Router {
    let path = config.server.webhook_path.clone();
    let state = AppState { config, metrics, notifier, update_tx };

    let mut router = Router::new()
        .route(&path, post(webhook_handler))
        .route("/health", get(health_handler));

    // Conditionally mount optional API endpoints based on config.
    {
        let s = &state;
        if s.config.server.http_api_metrics {
            router = router.route("/v1/metrics", get(metrics_handler));
        }
        if s.config.server.http_api_update {
            router = router.route("/v1/update", post(update_handler));
        }
    }

    router.with_state(state)
}

// ──────────────────────────────────────────────────────────────────────────────
// Handlers
// ──────────────────────────────────────────────────────────────────────────────

/// GET /health – simple liveness check.
async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, "updock ok\n")
}

/// GET /v1/metrics – Prometheus text exposition.
async fn metrics_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Optionally require the same API bearer token if one is configured.
    if let Some(ref token) = state.config.server.http_api_token {
        if !validate_bearer(token, &headers) {
            return (StatusCode::UNAUTHORIZED, "Unauthorized\n".to_string()).into_response();
        }
    }
    let body = state.metrics.render();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

/// POST /v1/update – manually trigger a watcher scan.
///
/// Sends a signal to the watcher loop via an mpsc channel, causing it to
/// run a scan immediately instead of waiting for the next scheduled interval.
/// Requires Bearer token auth when `http_api_token` is set.
async fn update_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(ref token) = state.config.server.http_api_token {
        if !validate_bearer(token, &headers) {
            warn!("update trigger rejected: invalid or missing Bearer token");
            return (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response();
        }
    }

    if let Some(ref tx) = state.update_tx {
        // Non-blocking send: if the channel is full the watcher is already
        // scheduled to run imminently, so we don't need to queue another.
        match tx.try_send(()) {
            Ok(()) => {
                info!("/v1/update called – immediate watcher scan triggered");
                (StatusCode::ACCEPTED, "update triggered\n").into_response()
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                info!("/v1/update called – scan already pending");
                (StatusCode::ACCEPTED, "update already pending\n").into_response()
            }
            Err(e) => {
                warn!(err = %e, "/v1/update: watcher channel send failed");
                (StatusCode::SERVICE_UNAVAILABLE, "watcher unavailable\n").into_response()
            }
        }
    } else {
        info!("/v1/update called – watcher not enabled");
        (StatusCode::SERVICE_UNAVAILABLE, "watcher not enabled\n").into_response()
    }
}

/// POST <webhook_path> – receives a Harbor webhook and triggers deployments.
async fn webhook_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // ── 1. Authenticate ─────────────────────────────────────────────────────
    if let Some(ref secret) = state.config.server.secret {
        if !validate_secret(secret, &headers) {
            warn!("webhook rejected: invalid or missing Authorization token");
            return (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response();
        }
    }

    // ── 2. Parse payload ────────────────────────────────────────────────────
    let event: HarborEvent = match serde_json::from_slice(&body) {
        Ok(e) => e,
        Err(err) => {
            error!(%err, "failed to parse Harbor webhook payload");
            return (StatusCode::BAD_REQUEST, "Bad Request: invalid JSON\n").into_response();
        }
    };

    info!(
        event_type = %event.event_type,
        repository  = %event.repository_full_name(),
        operator    = ?event.operator,
        tags        = ?event.pushed_tags(),
        "received Harbor webhook"
    );

    // ── 3. Only act on push events ──────────────────────────────────────────
    if event.event_type != "PUSH_ARTIFACT" {
        info!(event_type = %event.event_type, "ignoring non-push event");
        state.metrics.inc_webhook_ignored();
        return (StatusCode::OK, "ignored\n").into_response();
    }

    // ── 4. A push means a watched container may have a newer image: check now
    // rather than at the next poll (the watcher checks only its opted-in containers).
    let scan_triggered = match state.update_tx {
        Some(ref tx) => {
            let sent = tx.try_send(()).is_ok();
            info!(repository = %event.repository_full_name(), sent, "push: immediate watcher scan requested");
            true
        }
        None => false,
    };

    // ── 5. Match projects ───────────────────────────────────────────────────
    let repo = event.repository_full_name();
    let tags = event.pushed_tags();
    let projects = matching_projects(&state.config.projects, &repo, &tags);

    if projects.is_empty() {
        info!(repository = %repo, "no projects matched, nothing to do");
        if scan_triggered {
            return (StatusCode::OK, "scan triggered\n").into_response();
        }
        state.metrics.inc_webhook_ignored();
        return (StatusCode::OK, "no match\n").into_response();
    }

    // ── 6. Trigger deployments (in parallel) ───────────────────────────────
    // Collect handles so we can await results for metrics + notifications.
    let mut handles = Vec::new();
    for project in projects {
        let project = project.clone();
        let handle = tokio::spawn(async move {
            let result = compose::deploy(&project).await;
            (project.name.clone(), result)
        });
        handles.push(handle);
    }

    // Await all deployments in a background task so the HTTP response is
    // returned immediately (Harbor expects a fast acknowledgement).
    let metrics = Arc::clone(&state.metrics);
    let notifier = Arc::clone(&state.notifier);

    tokio::spawn(async move {
        let mut deployed: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();

        for handle in handles {
            match handle.await {
                Ok((name, DeployResult::Deployed)) => {
                    info!(project = %name, "webhook deployment succeeded");
                    metrics.inc_webhook_deployments();
                    deployed.push(name);
                }
                Ok((name, DeployResult::UpToDate)) => {
                    info!(project = %name, "webhook deployment skipped: images already up-to-date");
                    metrics.inc_webhook_skipped();
                    skipped.push(name);
                }
                Ok((name, DeployResult::Failed(err))) => {
                    error!(project = %name, err = %err, "webhook deployment failed");
                    metrics.inc_webhook_failures();
                    failed.push(name);
                }
                Err(join_err) => {
                    error!(err = %join_err, "webhook deployment task panicked");
                }
            }
        }

        // Send notification if anything notable happened.
        if !deployed.is_empty() || !failed.is_empty() {
            let msg = format_webhook_report(&deployed, &skipped, &failed);
            notifier.send(&msg).await;
        }
    });

    (StatusCode::OK, "deployment triggered\n").into_response()
}

// ──────────────────────────────────────────────────────────────────────────────
// Notification formatting
// ──────────────────────────────────────────────────────────────────────────────

/// Format a human-readable webhook deployment summary.
fn format_webhook_report(
    deployed: &[String],
    skipped: &[String],
    failed: &[String],
) -> String {
    let mut lines = vec![format!(
        "[updock webhook] {} deployed, {} skipped, {} failed",
        deployed.len(),
        skipped.len(),
        failed.len()
    )];
    for name in deployed {
        lines.push(format!("  ✓ deployed: {}", name));
    }
    for name in skipped {
        lines.push(format!("  - skipped:  {} (already up-to-date)", name));
    }
    for name in failed {
        lines.push(format!("  ✗ failed:   {}", name));
    }
    lines.join("\n")
}

// ──────────────────────────────────────────────────────────────────────────────
// Secret / token validation
// ──────────────────────────────────────────────────────────────────────────────

/// Validates the Harbor webhook shared secret.
///
/// Harbor sends the token as:   Authorization: <token>
/// We also accept:              X-Harbor-Token: <token>
fn validate_secret(expected: &str, headers: &HeaderMap) -> bool {
    if let Some(val) = headers.get("authorization") {
        if let Ok(s) = val.to_str() {
            let candidate = s.strip_prefix("Bearer ").unwrap_or(s);
            if constant_time_eq(candidate, expected) {
                return true;
            }
        }
    }
    if let Some(val) = headers.get("x-harbor-token") {
        if let Ok(s) = val.to_str() {
            if constant_time_eq(s, expected) {
                return true;
            }
        }
    }
    false
}

/// Validates a standard `Authorization: Bearer <token>` header.
fn validate_bearer(expected: &str, headers: &HeaderMap) -> bool {
    if let Some(val) = headers.get("authorization") {
        if let Ok(s) = val.to_str() {
            if let Some(token) = s.strip_prefix("Bearer ") {
                return constant_time_eq(token, expected);
            }
        }
    }
    false
}

/// Constant-time string comparison to prevent timing attacks.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_webhook_report_all_deployed() {
        let msg = format_webhook_report(
            &["proj-a".to_string(), "proj-b".to_string()],
            &[],
            &[],
        );
        assert!(msg.contains("2 deployed"));
        assert!(msg.contains("0 failed"));
        assert!(msg.contains("✓ deployed: proj-a"));
        assert!(msg.contains("✓ deployed: proj-b"));
    }

    #[test]
    fn format_webhook_report_with_failures() {
        let msg = format_webhook_report(&[], &[], &["proj-c".to_string()]);
        assert!(msg.contains("1 failed"));
        assert!(msg.contains("✗ failed:   proj-c"));
    }

    #[test]
    fn format_webhook_report_with_skipped() {
        let msg = format_webhook_report(&[], &["proj-d".to_string()], &[]);
        assert!(msg.contains("1 skipped"));
        assert!(msg.contains("already up-to-date"));
    }
}
