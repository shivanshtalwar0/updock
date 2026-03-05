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

use crate::{
    compose,
    config::Config,
    harbor::HarborEvent,
    matcher::matching_projects,
    metrics::Metrics,
};

// ──────────────────────────────────────────────────────────────────────────────
// Shared application state
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub metrics: Arc<Metrics>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Router
// ──────────────────────────────────────────────────────────────────────────────

pub fn build_router(config: Arc<Config>, metrics: Arc<Metrics>) -> Router {
    let path = config.server.webhook_path.clone();
    let state = AppState { config, metrics };

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
/// This endpoint just acknowledges the request; the actual scan is triggered
/// by sending an internal signal.  Because the watcher runs in its own
/// tokio task, the simplest approach is to record a note and let the next
/// interval fire.  For a truly synchronous trigger the caller should poll
/// `/v1/metrics` or use the `/health` endpoint.
///
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

    info!("/v1/update called – watcher will run on next cycle");
    (StatusCode::ACCEPTED, "update scheduled\n").into_response()
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
        return (StatusCode::OK, "ignored\n").into_response();
    }

    // ── 4. Match projects ───────────────────────────────────────────────────
    let repo = event.repository_full_name();
    let tags = event.pushed_tags();
    let projects = matching_projects(&state.config.projects, &repo, &tags);

    if projects.is_empty() {
        info!(repository = %repo, "no projects matched, nothing to do");
        return (StatusCode::OK, "no match\n").into_response();
    }

    // ── 5. Trigger deployments (in parallel) ───────────────────────────────
    let mut handles = Vec::new();
    for project in projects {
        let project = project.clone();
        let handle = tokio::spawn(async move {
            if let Err(err) = compose::deploy(&project).await {
                error!(
                    project = %project.name,
                    %err,
                    "deployment failed"
                );
            }
        });
        handles.push(handle);
    }

    tokio::spawn(async move {
        for h in handles {
            let _ = h.await;
        }
    });

    (StatusCode::OK, "deployment triggered\n").into_response()
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
