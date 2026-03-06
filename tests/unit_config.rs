/// Unit tests for config loading, defaults, validation, and YAML parsing.
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};
use updock::config;

// ── helpers ───────────────────────────────────────────────────────────────────

fn write_yaml(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

// ── server defaults ───────────────────────────────────────────────────────────

#[test]
fn server_defaults_applied_when_keys_absent() {
    let f = write_yaml(
        r#"
server: {}
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.server.bind, "0.0.0.0:8080");
    assert_eq!(cfg.server.webhook_path, "/webhook");
    assert!(cfg.server.secret.is_none());
    assert!(!cfg.server.http_api_update);
    assert!(!cfg.server.http_api_metrics);
    assert!(cfg.server.http_api_token.is_none());
}

#[test]
fn server_bind_and_path_override() {
    let f = write_yaml(
        r#"
server:
  bind: "127.0.0.1:9090"
  webhook_path: "/harbor/hook"
  secret: "topsecret"
  http_api_update: true
  http_api_metrics: true
  http_api_token: "mytoken"
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.server.bind, "127.0.0.1:9090");
    assert_eq!(cfg.server.webhook_path, "/harbor/hook");
    assert_eq!(cfg.server.secret.as_deref(), Some("topsecret"));
    assert!(cfg.server.http_api_update);
    assert!(cfg.server.http_api_metrics);
    assert_eq!(cfg.server.http_api_token.as_deref(), Some("mytoken"));
}

// ── daemon defaults ───────────────────────────────────────────────────────────

#[test]
fn daemon_defaults() {
    let f = write_yaml("server: {}");
    let cfg = config::load(f.path()).unwrap();
    assert!(!cfg.daemon.enabled);
    assert_eq!(cfg.daemon.pid_file.to_str().unwrap(), "/var/run/updock.pid");
    assert_eq!(cfg.daemon.log_dir.to_str().unwrap(), "/var/log/updock");
}

// ── projects ──────────────────────────────────────────────────────────────────

#[test]
fn project_with_valid_compose_dir_loads() {
    let dir = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: myapp
    harbor_repository: "myproject/myimage"
    compose_dir: "{}"
"#,
        dir.path().display()
    );
    let f = write_yaml(&yaml);
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.projects.len(), 1);
    let p = &cfg.projects[0];
    assert_eq!(p.name, "myapp");
    assert_eq!(p.harbor_repository, "myproject/myimage");
    assert!(p.tag_filter.is_none());
    assert!(p.pull_before_up); // default_true
}

#[test]
fn project_tag_filter_and_pull_false() {
    let dir = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: tagged
    harbor_repository: "ns/img"
    tag_filter: "v*"
    compose_dir: "{}"
    pull_before_up: false
"#,
        dir.path().display()
    );
    let f = write_yaml(&yaml);
    let cfg = config::load(f.path()).unwrap();
    let p = &cfg.projects[0];
    assert_eq!(p.tag_filter.as_deref(), Some("v*"));
    assert!(!p.pull_before_up);
}

#[test]
fn project_missing_harbor_repository_fails() {
    let dir = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: bad
    harbor_repository: ""
    compose_dir: "{}"
"#,
        dir.path().display()
    );
    let f = write_yaml(&yaml);
    let err = config::load(f.path()).unwrap_err();
    assert!(
        err.to_string()
            .contains("harbor_repository must not be empty"),
        "unexpected error: {err}"
    );
}

#[test]
fn project_nonexistent_compose_dir_fails() {
    let f = write_yaml(
        r#"
server: {}
projects:
  - name: nope
    harbor_repository: "ns/img"
    compose_dir: "/this/does/not/exist/at/all"
"#,
    );
    let err = config::load(f.path()).unwrap_err();
    assert!(
        err.to_string().contains("does not exist"),
        "unexpected error: {err}"
    );
}

#[test]
fn multiple_projects_load() {
    let d1 = TempDir::new().unwrap();
    let d2 = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: alpha
    harbor_repository: "ns/alpha"
    compose_dir: "{}"
  - name: beta
    harbor_repository: "ns/beta"
    compose_dir: "{}"
"#,
        d1.path().display(),
        d2.path().display()
    );
    let f = write_yaml(&yaml);
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.projects.len(), 2);
    assert_eq!(cfg.projects[0].name, "alpha");
    assert_eq!(cfg.projects[1].name, "beta");
}

// ── watcher defaults ──────────────────────────────────────────────────────────

#[test]
fn watcher_defaults() {
    let f = write_yaml("server: {}");
    let cfg = config::load(f.path()).unwrap();
    assert!(!cfg.watcher.enabled);
    assert_eq!(cfg.watcher.docker_host, "unix:///var/run/docker.sock");
    assert_eq!(cfg.watcher.poll_interval, 86400);
    assert_eq!(cfg.watcher.stop_timeout, 10);
    assert!(cfg.watcher.schedule.is_none());
    assert!(!cfg.watcher.cleanup);
    assert!(!cfg.watcher.monitor_only);
    assert!(!cfg.watcher.no_pull);
    assert!(!cfg.watcher.no_restart);
    assert!(!cfg.watcher.include_stopped);
    assert!(!cfg.watcher.revive_stopped);
    assert!(!cfg.watcher.rolling_restart);
    assert!(!cfg.watcher.label_enable);
    assert!(!cfg.watcher.lifecycle_hooks);
    assert!(!cfg.watcher.run_once);
    assert!(cfg.watcher.watch_containers.is_empty());
    assert!(cfg.watcher.disable_containers.is_empty());
    assert!(cfg.watcher.scope.is_none());
}

#[test]
fn watcher_all_flags_enabled() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  enabled: true
  poll_interval: 300
  cleanup: true
  remove_volumes: true
  monitor_only: true
  no_pull: true
  no_restart: true
  include_stopped: true
  revive_stopped: true
  rolling_restart: true
  label_enable: true
  lifecycle_hooks: true
  run_once: true
  stop_timeout: 30
  scope: "production"
  watch_containers: ["app", "worker"]
  disable_containers: ["debug"]
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let w = &cfg.watcher;
    assert!(w.enabled);
    assert_eq!(w.poll_interval, 300);
    assert!(w.cleanup);
    assert!(w.remove_volumes);
    assert!(w.monitor_only);
    assert!(w.no_pull);
    assert!(w.no_restart);
    assert!(w.include_stopped);
    assert!(w.revive_stopped);
    assert!(w.rolling_restart);
    assert!(w.label_enable);
    assert!(w.lifecycle_hooks);
    assert!(w.run_once);
    assert_eq!(w.stop_timeout, 30);
    assert_eq!(w.scope.as_deref(), Some("production"));
    assert_eq!(w.watch_containers, ["app", "worker"]);
    assert_eq!(w.disable_containers, ["debug"]);
}

#[test]
fn watcher_valid_cron_schedule_accepted() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  enabled: false
  schedule: "0 0 4 * * *"
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.watcher.schedule.as_deref(), Some("0 0 4 * * *"));
}

#[test]
fn watcher_invalid_cron_schedule_rejected() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  enabled: false
  schedule: "not-a-cron"
"#,
    );
    let err = config::load(f.path()).unwrap_err();
    assert!(
        err.to_string().contains("Invalid watcher.schedule"),
        "unexpected error: {err}"
    );
}

// ── notifications ─────────────────────────────────────────────────────────────

#[test]
fn notification_defaults_are_empty() {
    let f = write_yaml("server: {}");
    let cfg = config::load(f.path()).unwrap();
    let n = &cfg.notifications;
    assert!(n.urls.is_empty());
    assert!(n.slack_webhook_url.is_none());
    assert!(n.gotify_url.is_none());
    assert!(n.email.is_none());
}

#[test]
fn notification_config_full() {
    let f = write_yaml(
        r##"
server: {}
notifications:
  urls:
    - "https://hooks.example.com/abc"
  slack_webhook_url: "https://hooks.slack.com/xyz"
  slack_channel: "#ops"
  slack_username: "updock-bot"
  gotify_url: "https://gotify.example.com"
  gotify_token: "secret123"
  email:
    smtp_host: "smtp.example.com"
    smtp_port: 465
    username: "user@example.com"
    password: "pass"
    from: "updock@example.com"
    to:
      - "admin@example.com"
      - "ops@example.com"
    tls_skip_verify: true
"##,
    );
    let cfg = config::load(f.path()).unwrap();
    let n = &cfg.notifications;
    assert_eq!(n.urls, ["https://hooks.example.com/abc"]);
    assert_eq!(
        n.slack_webhook_url.as_deref(),
        Some("https://hooks.slack.com/xyz")
    );
    assert_eq!(n.slack_channel.as_deref(), Some("#ops"));
    assert_eq!(n.slack_username.as_deref(), Some("updock-bot"));
    assert_eq!(n.gotify_url.as_deref(), Some("https://gotify.example.com"));
    assert_eq!(n.gotify_token.as_deref(), Some("secret123"));
    let email = n.email.as_ref().unwrap();
    assert_eq!(email.smtp_host, "smtp.example.com");
    assert_eq!(email.smtp_port, 465);
    assert_eq!(email.username.as_deref(), Some("user@example.com"));
    assert_eq!(email.from, "updock@example.com");
    assert_eq!(email.to, ["admin@example.com", "ops@example.com"]);
    assert!(email.tls_skip_verify);
}

#[test]
fn email_smtp_port_default() {
    let f = write_yaml(
        r#"
server: {}
notifications:
  email:
    smtp_host: "smtp.example.com"
    from: "a@b.com"
    to: ["c@d.com"]
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let email = cfg.notifications.email.as_ref().unwrap();
    assert_eq!(email.smtp_port, 587);
    assert!(!email.tls_skip_verify);
}

// ── file-not-found error ──────────────────────────────────────────────────────

#[test]
fn load_missing_file_returns_error() {
    let err = config::load(std::path::Path::new("/no/such/file.yaml")).unwrap_err();
    assert!(
        err.to_string().contains("Cannot read config file"),
        "unexpected error: {err}"
    );
}

#[test]
fn load_invalid_yaml_returns_error() {
    let f = write_yaml("{{{{: [bad yaml");
    let err = config::load(f.path()).unwrap_err();
    assert!(
        err.to_string().contains("Config parse error"),
        "unexpected error: {err}"
    );
}

// ── project compose_file override ─────────────────────────────────────────────

#[test]
fn project_compose_file_override() {
    let dir = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: custom
    harbor_repository: "ns/img"
    compose_dir: "{}"
    compose_file: "docker-compose.prod.yml"
"#,
        dir.path().display()
    );
    let f = write_yaml(&yaml);
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(
        cfg.projects[0].compose_file.as_deref(),
        Some("docker-compose.prod.yml")
    );
}

#[test]
fn project_env_vars_loaded() {
    let dir = TempDir::new().unwrap();
    let yaml = format!(
        r#"
server: {{}}
projects:
  - name: envtest
    harbor_repository: "ns/img"
    compose_dir: "{}"
    env:
      FOO: bar
      SOME_VAR: "hello world"
"#,
        dir.path().display()
    );
    let f = write_yaml(&yaml);
    let cfg = config::load(f.path()).unwrap();
    let env = &cfg.projects[0].env;
    assert_eq!(env.get("FOO").map(|s| s.as_str()), Some("bar"));
    assert_eq!(env.get("SOME_VAR").map(|s| s.as_str()), Some("hello world"));
}

// ── registry credentials ──────────────────────────────────────────────────────

#[test]
fn watcher_registries_defaults_to_empty() {
    let f = write_yaml(
        r#"
server: {}
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert!(cfg.watcher.registries.is_empty());
}

#[test]
fn watcher_registries_username_password() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  registries:
    registry.example.com:
      username: myuser
      password: mypass
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let reg = cfg.watcher.registries.get("registry.example.com").unwrap();
    assert_eq!(reg.username.as_deref(), Some("myuser"));
    assert_eq!(reg.password.as_deref(), Some("mypass"));
    assert!(reg.token.is_none());
}

#[test]
fn watcher_registries_token_only() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  registries:
    ghcr.io:
      token: ghp_abc123
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let reg = cfg.watcher.registries.get("ghcr.io").unwrap();
    assert!(reg.username.is_none());
    assert!(reg.password.is_none());
    assert_eq!(reg.token.as_deref(), Some("ghp_abc123"));
}

#[test]
fn watcher_registries_multiple_entries() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  registries:
    registry.example.com:
      username: u1
      password: p1
    ghcr.io:
      token: tok123
    index.docker.io:
      username: hubuser
      password: hubpass
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert_eq!(cfg.watcher.registries.len(), 3);
    assert!(cfg.watcher.registries.contains_key("registry.example.com"));
    assert!(cfg.watcher.registries.contains_key("ghcr.io"));
    assert!(cfg.watcher.registries.contains_key("index.docker.io"));
}

#[test]
fn watcher_registries_with_port() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  registries:
    registry.example.com:5000:
      username: portuser
      password: portpass
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let reg = cfg
        .watcher
        .registries
        .get("registry.example.com:5000")
        .unwrap();
    assert_eq!(reg.username.as_deref(), Some("portuser"));
}

#[test]
fn watcher_registry_empty_fields_are_none() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  registries:
    registry.example.com: {}
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    let reg = cfg.watcher.registries.get("registry.example.com").unwrap();
    assert!(reg.username.is_none());
    assert!(reg.password.is_none());
    assert!(reg.token.is_none());
}

// ── ignore_compose_managed ────────────────────────────────────────────────────

#[test]
fn watcher_ignore_compose_managed_defaults_to_true() {
    let f = write_yaml(
        r#"
server: {}
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert!(
        cfg.watcher.ignore_compose_managed,
        "ignore_compose_managed should default to true"
    );
}

#[test]
fn watcher_ignore_compose_managed_can_be_disabled() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  ignore_compose_managed: false
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert!(!cfg.watcher.ignore_compose_managed);
}

#[test]
fn watcher_ignore_compose_managed_explicit_true() {
    let f = write_yaml(
        r#"
server: {}
watcher:
  ignore_compose_managed: true
"#,
    );
    let cfg = config::load(f.path()).unwrap();
    assert!(cfg.watcher.ignore_compose_managed);
}
