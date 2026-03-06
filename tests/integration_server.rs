/// Integration tests for the HTTP server (webhook, health, metrics, update).
///
/// Uses `axum-test` to spin up the router in-process and fire real HTTP
/// requests against it without binding a TCP socket.
use axum::http::StatusCode;
use axum_test::TestServer;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

use updock::{
    config::{Config, DaemonConfig, NotificationConfig, ServerConfig, WatcherConfig},
    metrics::Metrics,
    notify::Notifier,
    server::build_router,
};

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a minimal valid ServerConfig with all optional features disabled.
fn server_cfg_minimal() -> ServerConfig {
    ServerConfig {
        bind: "0.0.0.0:8080".to_string(),
        webhook_path: "/webhook".to_string(),
        secret: None,
        http_api_update: false,
        http_api_metrics: false,
        http_api_token: None,
    }
}

fn make_config(server: ServerConfig) -> Arc<Config> {
    Arc::new(Config {
        server,
        daemon: DaemonConfig::default(),
        projects: vec![],
        watcher: WatcherConfig::default(),
        notifications: NotificationConfig::default(),
    })
}

fn make_notifier() -> Arc<Notifier> {
    Notifier::new(NotificationConfig::default())
}

fn make_server(config: Arc<Config>) -> TestServer {
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(config, metrics, notifier, None);
    TestServer::new(router).unwrap()
}

/// A minimal valid Harbor PUSH_ARTIFACT JSON payload.
fn push_payload(repo_full: &str, tag: &str) -> serde_json::Value {
    json!({
        "type": "PUSH_ARTIFACT",
        "occur_at": 1700000000,
        "operator": "admin",
        "event_data": {
            "resources": [{"tag": tag, "digest": "sha256:abc", "resource_url": ""}],
            "repository": {
                "name": repo_full.split('/').last().unwrap_or(repo_full),
                "namespace": repo_full.split('/').next().unwrap_or(""),
                "full_name": repo_full,
                "type": "private"
            }
        }
    })
}

fn delete_payload(repo_full: &str, tag: &str) -> serde_json::Value {
    json!({
        "type": "DELETE_ARTIFACT",
        "occur_at": 1700000001,
        "operator": "admin",
        "event_data": {
            "resources": [{"tag": tag}],
            "repository": {
                "name": repo_full.split('/').last().unwrap_or(repo_full),
                "full_name": repo_full,
                "type": "private"
            }
        }
    })
}

// ── /health ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn health_returns_200() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server.get("/health").await;
    response.assert_status_ok();
    assert_eq!(response.text(), "updock ok\n");
}

#[tokio::test]
async fn health_always_accessible_without_auth() {
    // Even when a secret is configured, /health is open.
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("mysecret".to_string());
    let server = make_server(make_config(cfg));
    let response = server.get("/health").await;
    response.assert_status_ok();
}

// ── /webhook – no auth ────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_no_projects_returns_no_match() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server
        .post("/webhook")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "no match\n");
}

#[tokio::test]
async fn webhook_non_push_event_returns_ignored() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server
        .post("/webhook")
        .json(&delete_payload("ns/img", "v1"))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "ignored\n");
}

#[tokio::test]
async fn webhook_invalid_json_returns_400() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server
        .post("/webhook")
        .bytes(axum::body::Bytes::from_static(b"not-json"))
        .content_type("application/json")
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn webhook_empty_body_returns_400() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server
        .post("/webhook")
        .bytes(axum::body::Bytes::from_static(b""))
        .content_type("application/json")
        .await;
    response.assert_status(StatusCode::BAD_REQUEST);
}

// ── /webhook – auth ───────────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_with_secret_missing_header_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("topsecret".to_string());
    let server = make_server(make_config(cfg));
    let response = server
        .post("/webhook")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_with_secret_wrong_token_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("topsecret".to_string());
    let server = make_server(make_config(cfg));
    let response = server
        .post("/webhook")
        .add_header("Authorization", "wrongtoken")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_with_secret_correct_bearer_returns_200() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("topsecret".to_string());
    let server = make_server(make_config(cfg));
    // Harbor sends `Authorization: Bearer <token>`
    let response = server
        .post("/webhook")
        .add_header("Authorization", "Bearer topsecret")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn webhook_with_secret_correct_raw_token_returns_200() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("topsecret".to_string());
    let server = make_server(make_config(cfg));
    // Also accept raw token without "Bearer " prefix
    let response = server
        .post("/webhook")
        .add_header("Authorization", "topsecret")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn webhook_with_secret_x_harbor_token_header_returns_200() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("topsecret".to_string());
    let server = make_server(make_config(cfg));
    let response = server
        .post("/webhook")
        .add_header("X-Harbor-Token", "topsecret")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status_ok();
}

// ── /webhook – matching projects ──────────────────────────────────────────────

#[tokio::test]
async fn webhook_matching_project_returns_deployment_triggered() {
    let dir = TempDir::new().unwrap();
    use updock::config::ProjectConfig;
    let project = ProjectConfig {
        name: "myapp".to_string(),
        harbor_repository: "ns/myimage".to_string(),
        tag_filter: None,
        compose_dir: dir.path().to_path_buf(),
        compose_file: None,
        env: Default::default(),
        pull_before_up: false, // don't actually pull in tests
    };
    let config = Arc::new(Config {
        server: server_cfg_minimal(),
        daemon: DaemonConfig::default(),
        projects: vec![project],
        watcher: WatcherConfig::default(),
        notifications: NotificationConfig::default(),
    });
    let server = make_server(config);
    let response = server
        .post("/webhook")
        .json(&push_payload("ns/myimage", "v1.0"))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "deployment triggered\n");
}

#[tokio::test]
async fn webhook_project_filtered_by_tag_no_match() {
    let dir = TempDir::new().unwrap();
    use updock::config::ProjectConfig;
    let project = ProjectConfig {
        name: "myapp".to_string(),
        harbor_repository: "ns/myimage".to_string(),
        tag_filter: Some("v*".to_string()),
        compose_dir: dir.path().to_path_buf(),
        compose_file: None,
        env: Default::default(),
        pull_before_up: false,
    };
    let config = Arc::new(Config {
        server: server_cfg_minimal(),
        daemon: DaemonConfig::default(),
        projects: vec![project],
        watcher: WatcherConfig::default(),
        notifications: NotificationConfig::default(),
    });
    let server = make_server(config);
    // "latest" does not match "v*" filter
    let response = server
        .post("/webhook")
        .json(&push_payload("ns/myimage", "latest"))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "no match\n");
}

#[tokio::test]
async fn webhook_project_filtered_by_tag_match() {
    let dir = TempDir::new().unwrap();
    use updock::config::ProjectConfig;
    let project = ProjectConfig {
        name: "myapp".to_string(),
        harbor_repository: "ns/myimage".to_string(),
        tag_filter: Some("v*".to_string()),
        compose_dir: dir.path().to_path_buf(),
        compose_file: None,
        env: Default::default(),
        pull_before_up: false,
    };
    let config = Arc::new(Config {
        server: server_cfg_minimal(),
        daemon: DaemonConfig::default(),
        projects: vec![project],
        watcher: WatcherConfig::default(),
        notifications: NotificationConfig::default(),
    });
    let server = make_server(config);
    // "v2.3" matches "v*"
    let response = server
        .post("/webhook")
        .json(&push_payload("ns/myimage", "v2.3"))
        .await;
    response.assert_status_ok();
    assert_eq!(response.text(), "deployment triggered\n");
}

// ── /webhook – custom path ────────────────────────────────────────────────────

#[tokio::test]
async fn webhook_on_custom_path() {
    let mut cfg = server_cfg_minimal();
    cfg.webhook_path = "/harbor/hook".to_string();
    let server = make_server(make_config(cfg));

    // Default /webhook should 404 (route not registered).
    let default_response = server
        .post("/webhook")
        .json(&push_payload("ns/img", "latest"))
        .await;
    assert_eq!(default_response.status_code(), StatusCode::NOT_FOUND);

    // Custom path should work.
    let custom_response = server
        .post("/harbor/hook")
        .json(&push_payload("ns/img", "latest"))
        .await;
    custom_response.assert_status_ok();
}

// ── /v1/metrics ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn metrics_not_enabled_returns_404() {
    // http_api_metrics = false → route not mounted.
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server.get("/v1/metrics").await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn metrics_enabled_returns_prometheus_text() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_metrics = true;
    let server = make_server(make_config(cfg));
    let response = server.get("/v1/metrics").await;
    response.assert_status_ok();
    let body = response.text();
    // Prometheus text format starts with # HELP or # TYPE lines.
    assert!(
        body.contains("# HELP") || body.contains("# TYPE") || body.contains("updock_"),
        "Expected Prometheus-format body, got: {body}"
    );
}

#[tokio::test]
async fn metrics_with_token_no_auth_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_metrics = true;
    cfg.http_api_token = Some("metricstoken".to_string());
    let server = make_server(make_config(cfg));
    let response = server.get("/v1/metrics").await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn metrics_with_token_correct_bearer_returns_200() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_metrics = true;
    cfg.http_api_token = Some("metricstoken".to_string());
    let server = make_server(make_config(cfg));
    let response = server
        .get("/v1/metrics")
        .add_header("Authorization", "Bearer metricstoken")
        .await;
    response.assert_status_ok();
}

#[tokio::test]
async fn metrics_with_token_wrong_bearer_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_metrics = true;
    cfg.http_api_token = Some("metricstoken".to_string());
    let server = make_server(make_config(cfg));
    let response = server
        .get("/v1/metrics")
        .add_header("Authorization", "Bearer wrongtoken")
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

// ── /v1/update ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn update_not_enabled_returns_404() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server.post("/v1/update").await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn update_enabled_no_watcher_returns_503() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    // No update_tx → watcher not running.
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, None);
    let server = TestServer::new(router).unwrap();
    let response = server.post("/v1/update").await;
    response.assert_status(StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.text(), "watcher not enabled\n");
}

#[tokio::test]
async fn update_enabled_with_watcher_returns_202() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, Some(tx));
    let server = TestServer::new(router).unwrap();
    let response = server.post("/v1/update").await;
    response.assert_status(StatusCode::ACCEPTED);
    assert_eq!(response.text(), "update triggered\n");
}

#[tokio::test]
async fn update_already_pending_returns_202() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    // Channel capacity = 1; send once to fill it, then second call returns "already pending".
    let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
    tx.try_send(()).unwrap(); // fill the channel
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, Some(tx));
    let server = TestServer::new(router).unwrap();
    let response = server.post("/v1/update").await;
    response.assert_status(StatusCode::ACCEPTED);
    assert_eq!(response.text(), "update already pending\n");
}

#[tokio::test]
async fn update_with_token_no_auth_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    cfg.http_api_token = Some("updatetoken".to_string());
    let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, Some(tx));
    let server = TestServer::new(router).unwrap();
    let response = server.post("/v1/update").await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn update_with_token_correct_bearer_returns_202() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    cfg.http_api_token = Some("updatetoken".to_string());
    let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, Some(tx));
    let server = TestServer::new(router).unwrap();
    let response = server
        .post("/v1/update")
        .add_header("Authorization", "Bearer updatetoken")
        .await;
    response.assert_status(StatusCode::ACCEPTED);
}

#[tokio::test]
async fn update_with_token_wrong_bearer_returns_401() {
    let mut cfg = server_cfg_minimal();
    cfg.http_api_update = true;
    cfg.http_api_token = Some("updatetoken".to_string());
    let (tx, _rx) = tokio::sync::mpsc::channel::<()>(1);
    let metrics = Metrics::new();
    let notifier = make_notifier();
    let router = build_router(make_config(cfg), metrics, notifier, Some(tx));
    let server = TestServer::new(router).unwrap();
    let response = server
        .post("/v1/update")
        .add_header("Authorization", "Bearer wrongtoken")
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

// ── route-not-found ───────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_route_returns_404() {
    let server = make_server(make_config(server_cfg_minimal()));
    let response = server.get("/does-not-exist").await;
    assert_eq!(response.status_code(), StatusCode::NOT_FOUND);
}

// ── security: timing-safe comparison ─────────────────────────────────────────

#[tokio::test]
async fn webhook_auth_rejects_prefix_of_correct_secret() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("longpassword".to_string());
    let server = make_server(make_config(cfg));
    // "longpass" is a prefix of "longpassword" – must still be rejected.
    let response = server
        .post("/webhook")
        .add_header("Authorization", "longpass")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn webhook_auth_rejects_suffix_of_correct_secret() {
    let mut cfg = server_cfg_minimal();
    cfg.secret = Some("longpassword".to_string());
    let server = make_server(make_config(cfg));
    // "word" is a suffix – must be rejected.
    let response = server
        .post("/webhook")
        .add_header("Authorization", "word")
        .json(&push_payload("ns/img", "latest"))
        .await;
    response.assert_status(StatusCode::UNAUTHORIZED);
}
