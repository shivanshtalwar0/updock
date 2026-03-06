/// Unit tests for SessionReport and watcher label constants.
use updock::watcher::{
    registry_host_from_image, SessionReport, LABEL_COMPOSE_PROJECT, LABEL_DEPENDS_ON, LABEL_ENABLE,
    LABEL_MONITOR_ONLY, LABEL_NO_PULL, LABEL_POST_CHECK, LABEL_POST_UPDATE, LABEL_PRE_CHECK,
    LABEL_PRE_UPDATE, LABEL_SCOPE, LABEL_STOP_SIGNAL,
};

// ── label constant values ─────────────────────────────────────────────────────

#[test]
fn label_enable_is_watchtower_compatible() {
    assert_eq!(LABEL_ENABLE, "com.centurylinklabs.watchtower.enable");
}

#[test]
fn label_monitor_only_is_watchtower_compatible() {
    assert_eq!(
        LABEL_MONITOR_ONLY,
        "com.centurylinklabs.watchtower.monitor-only"
    );
}

#[test]
fn label_no_pull_is_watchtower_compatible() {
    assert_eq!(LABEL_NO_PULL, "com.centurylinklabs.watchtower.no-pull");
}

#[test]
fn label_stop_signal_is_watchtower_compatible() {
    assert_eq!(
        LABEL_STOP_SIGNAL,
        "com.centurylinklabs.watchtower.stop-signal"
    );
}

#[test]
fn label_scope_is_watchtower_compatible() {
    assert_eq!(LABEL_SCOPE, "com.centurylinklabs.watchtower.scope");
}

#[test]
fn label_depends_on_is_watchtower_compatible() {
    assert_eq!(
        LABEL_DEPENDS_ON,
        "com.centurylinklabs.watchtower.depends-on"
    );
}

#[test]
fn label_pre_check_is_watchtower_compatible() {
    assert_eq!(
        LABEL_PRE_CHECK,
        "com.centurylinklabs.watchtower.lifecycle.pre-check"
    );
}

#[test]
fn label_pre_update_is_watchtower_compatible() {
    assert_eq!(
        LABEL_PRE_UPDATE,
        "com.centurylinklabs.watchtower.lifecycle.pre-update"
    );
}

#[test]
fn label_post_update_is_watchtower_compatible() {
    assert_eq!(
        LABEL_POST_UPDATE,
        "com.centurylinklabs.watchtower.lifecycle.post-update"
    );
}

#[test]
fn label_post_check_is_watchtower_compatible() {
    assert_eq!(
        LABEL_POST_CHECK,
        "com.centurylinklabs.watchtower.lifecycle.post-check"
    );
}

#[test]
fn all_lifecycle_labels_have_common_prefix() {
    let prefix = "com.centurylinklabs.watchtower.lifecycle.";
    assert!(LABEL_PRE_CHECK.starts_with(prefix));
    assert!(LABEL_PRE_UPDATE.starts_with(prefix));
    assert!(LABEL_POST_UPDATE.starts_with(prefix));
    assert!(LABEL_POST_CHECK.starts_with(prefix));
}

// ── SessionReport ─────────────────────────────────────────────────────────────

#[test]
fn session_report_default_is_all_empty() {
    let r = SessionReport::default();
    assert_eq!(r.scanned, 0);
    assert!(r.updated.is_empty());
    assert!(r.failed.is_empty());
    assert!(r.skipped.is_empty());
    assert!(r.fresh.is_empty());
}

#[test]
fn session_report_can_be_populated() {
    let r = SessionReport {
        scanned: 5,
        updated: vec!["nginx".to_string(), "redis".to_string()],
        failed: vec!["postgres".to_string()],
        skipped: vec!["monitor-only".to_string()],
        fresh: vec!["stable-app".to_string()],
    };
    assert_eq!(r.scanned, 5);
    assert_eq!(r.updated.len(), 2);
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.fresh.len(), 1);
}

#[test]
fn session_report_is_cloneable() {
    let r = SessionReport {
        scanned: 3,
        updated: vec!["app".to_string()],
        failed: vec![],
        skipped: vec![],
        fresh: vec!["db".to_string(), "cache".to_string()],
    };
    let r2 = r.clone();
    assert_eq!(r2.scanned, 3);
    assert_eq!(r2.updated, r.updated);
    assert_eq!(r2.fresh.len(), 2);
}

#[test]
fn session_report_updated_reflects_actual_updates() {
    let mut r = SessionReport::default();
    r.scanned = 10;
    r.updated.push("/myapp".to_string());
    r.updated.push("/worker".to_string());
    r.fresh.extend((0..8).map(|i| format!("/container-{}", i)));
    assert_eq!(r.scanned, 10);
    assert_eq!(r.updated.len(), 2);
    assert_eq!(r.fresh.len(), 8);
    assert_eq!(r.failed.len(), 0);
}

#[test]
fn session_report_all_failed() {
    let r = SessionReport {
        scanned: 3,
        updated: vec![],
        failed: vec!["/a".to_string(), "/b".to_string(), "/c".to_string()],
        skipped: vec![],
        fresh: vec![],
    };
    assert!(r.updated.is_empty());
    assert_eq!(r.failed.len(), 3);
}

// ── registry_host_from_image ──────────────────────────────────────────────────

#[test]
fn registry_host_plain_name_is_docker_hub() {
    assert_eq!(registry_host_from_image("nginx"), "index.docker.io");
    assert_eq!(registry_host_from_image("nginx:latest"), "index.docker.io");
}

#[test]
fn registry_host_user_slash_repo_is_docker_hub() {
    assert_eq!(
        registry_host_from_image("myuser/myapp:v1"),
        "index.docker.io"
    );
    assert_eq!(registry_host_from_image("library/nginx"), "index.docker.io");
}

#[test]
fn registry_host_fqdn_extracted() {
    assert_eq!(
        registry_host_from_image("registry.example.com/myapp:v1"),
        "registry.example.com"
    );
}

#[test]
fn registry_host_fqdn_with_port_extracted() {
    assert_eq!(
        registry_host_from_image("registry.example.com:5000/myapp:v1"),
        "registry.example.com:5000"
    );
}

#[test]
fn registry_host_ghcr() {
    assert_eq!(
        registry_host_from_image("ghcr.io/org/repo:latest"),
        "ghcr.io"
    );
}

#[test]
fn registry_host_localhost_with_port() {
    assert_eq!(
        registry_host_from_image("localhost:5000/myapp"),
        "localhost:5000"
    );
}

#[test]
fn registry_host_harbor_deep_path() {
    assert_eq!(
        registry_host_from_image("harbor.example.com/project/image:tag"),
        "harbor.example.com"
    );
}

#[test]
fn registry_host_ecr_style() {
    assert_eq!(
        registry_host_from_image("123456789.dkr.ecr.us-east-1.amazonaws.com/myapp:v1"),
        "123456789.dkr.ecr.us-east-1.amazonaws.com"
    );
}

// ── compose-managed label ─────────────────────────────────────────────────────

#[test]
fn label_compose_project_is_docker_compose_standard() {
    assert_eq!(LABEL_COMPOSE_PROJECT, "com.docker.compose.project");
}

#[test]
fn label_compose_project_differs_from_watchtower_labels() {
    // Sanity check: the compose label is a different namespace entirely.
    assert!(!LABEL_COMPOSE_PROJECT.contains("centurylinklabs"));
    assert!(LABEL_COMPOSE_PROJECT.contains("docker.compose"));
}

// ── ignore_compose_managed default ───────────────────────────────────────────

#[test]
fn watcher_config_default_ignores_compose_managed() {
    use updock::config::WatcherConfig;
    let cfg = WatcherConfig::default();
    assert!(
        cfg.ignore_compose_managed,
        "ignore_compose_managed should default to true to prevent watcher/compose conflicts"
    );
}
