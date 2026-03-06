use serde::Deserialize;
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────────────────────
// Top-level config
// ──────────────────────────────────────────────────────────────────────────────

/// Root configuration loaded from `updock.yaml`.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// HTTP server settings (webhook receiver + API).
    pub server: ServerConfig,

    /// Daemon settings.
    #[serde(default)]
    pub daemon: DaemonConfig,

    /// Harbor webhook → docker compose projects.
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,

    /// Watchtower-style automatic container polling / updating.
    #[serde(default)]
    pub watcher: WatcherConfig,

    /// Notification settings.
    #[serde(default)]
    pub notifications: NotificationConfig,
}

// ──────────────────────────────────────────────────────────────────────────────
// Server
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// Address to bind, e.g. "0.0.0.0:8080".
    #[serde(default = "default_bind")]
    pub bind: String,

    /// URL path Harbor posts to, e.g. "/webhook".
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,

    /// Shared secret sent by Harbor in the `Authorization` header.
    pub secret: Option<String>,

    /// Enable the /v1/update HTTP API trigger endpoint.
    #[serde(default)]
    pub http_api_update: bool,

    /// Bearer token required for /v1/update calls.
    pub http_api_token: Option<String>,

    /// Expose /v1/metrics Prometheus endpoint.
    #[serde(default)]
    pub http_api_metrics: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Daemon
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct DaemonConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_pid_file")]
    pub pid_file: PathBuf,

    #[serde(default = "default_log_dir")]
    pub log_dir: PathBuf,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pid_file: default_pid_file(),
            log_dir: default_log_dir(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Harbor webhook → compose project mapping
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct ProjectConfig {
    pub name: String,
    pub harbor_repository: String,
    pub tag_filter: Option<String>,
    pub compose_dir: PathBuf,
    #[serde(default)]
    pub compose_file: Option<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    #[serde(default = "default_true")]
    pub pull_before_up: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Registry credentials
// ──────────────────────────────────────────────────────────────────────────────

/// Credentials for a private container registry.
///
/// The key used in `watcher.registries` is the registry **hostname** (and
/// optional port), e.g. `registry.example.com` or `registry.example.com:5000`.
/// updock matches the hostname against the image reference at pull time.
///
/// Supported auth methods (in priority order):
/// 1. `username` + `password` — HTTP Basic / token auth (Harbor, GHCR, etc.)
/// 2. `token` — bearer / identity token (some registries return these from
///    an OAuth exchange; maps to bollard's `identitytoken` field)
///
/// If both are set, `username`/`password` takes precedence over `token`.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct RegistryConfig {
    /// Registry username.
    pub username: Option<String>,
    /// Registry password or CLI secret.
    pub password: Option<String>,
    /// Pre-obtained identity/bearer token (alternative to username+password).
    pub token: Option<String>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Watcher (Watchtower-style)
// ──────────────────────────────────────────────────────────────────────────────

/// Controls the automatic container image-update polling loop.
#[derive(Debug, Deserialize, Clone)]
pub struct WatcherConfig {
    /// Enable the watcher polling loop.
    #[serde(default)]
    pub enabled: bool,

    /// Docker socket / TCP endpoint.
    #[serde(default = "default_docker_host")]
    pub docker_host: String,

    /// Poll interval in seconds. Mutually exclusive with `schedule`.
    /// Default: 86400 (24 h).
    #[serde(default = "default_poll_interval")]
    pub poll_interval: u64,

    /// Cron schedule (6-field). Overrides `poll_interval` when set.
    /// Example: "0 0 4 * * *"
    pub schedule: Option<String>,

    /// Remove old images after updating a container.
    #[serde(default)]
    pub cleanup: bool,

    /// Remove anonymous volumes when recreating containers.
    #[serde(default)]
    pub remove_volumes: bool,

    /// Only monitor containers – do not actually update them.
    #[serde(default)]
    pub monitor_only: bool,

    /// Don't pull new images; only compare local digests.
    #[serde(default)]
    pub no_pull: bool,

    /// Don't restart containers after updating.
    #[serde(default)]
    pub no_restart: bool,

    /// Also watch containers that are stopped / exited.
    #[serde(default)]
    pub include_stopped: bool,

    /// Start stopped containers whose image was updated
    /// (requires `include_stopped`).
    #[serde(default)]
    pub revive_stopped: bool,

    /// Restart one container at a time (rolling restart).
    #[serde(default)]
    pub rolling_restart: bool,

    /// Only watch containers with label
    /// `com.centurylinklabs.watchtower.enable=true`.
    #[serde(default)]
    pub label_enable: bool,

    /// Container names to explicitly exclude.
    #[serde(default)]
    pub disable_containers: Vec<String>,

    /// Scope label value; only watch containers with matching scope.
    pub scope: Option<String>,

    /// Timeout before force-stopping a container (seconds). Default: 10.
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout: u64,

    /// Enable lifecycle hooks (pre/post-check, pre/post-update).
    #[serde(default)]
    pub lifecycle_hooks: bool,

    /// Run only once then exit (useful for CI / manual triggers).
    #[serde(default)]
    pub run_once: bool,

    /// Container names to restrict watching to. Empty = watch all.
    #[serde(default)]
    pub watch_containers: Vec<String>,

    /// Ignore containers managed by Docker Compose.
    ///
    /// When `true` (the default), the watcher skips any container that carries
    /// the `com.docker.compose.project` label, which Docker Compose always
    /// sets on every service it manages.
    ///
    /// This prevents the watcher and the webhook `compose up` path from
    /// fighting over the same containers: the webhook handles Compose stacks
    /// (stopping, pulling, and recreating all services together), while the
    /// watcher handles bare containers individually.
    ///
    /// Set to `false` only if you deliberately want the watcher to manage
    /// containers that were originally created by Compose.
    #[serde(default = "default_true")]
    pub ignore_compose_managed: bool,

    /// Per-registry credentials for private registries.
    ///
    /// Map key is the registry hostname (with optional port), e.g.:
    ///   `registry.example.com` or `registry.example.com:5000`
    ///
    /// updock matches the hostname extracted from each container's image
    /// reference and supplies the corresponding credentials when pulling.
    /// Images without a registry prefix (e.g. `nginx:latest`) are treated
    /// as Docker Hub (`index.docker.io`) — use that key to authenticate
    /// against Docker Hub.
    ///
    /// Example:
    /// ```yaml
    /// watcher:
    ///   registries:
    ///     registry.example.com:
    ///       username: myuser
    ///       password: mypassword
    ///     ghcr.io:
    ///       token: ghp_xxxxxxxxxxxxx
    ///     index.docker.io:
    ///       username: dockerhubuser
    ///       password: dockerhubpass
    /// ```
    #[serde(default)]
    pub registries: std::collections::HashMap<String, RegistryConfig>,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            docker_host: default_docker_host(),
            poll_interval: default_poll_interval(),
            schedule: None,
            cleanup: false,
            remove_volumes: false,
            monitor_only: false,
            no_pull: false,
            no_restart: false,
            include_stopped: false,
            revive_stopped: false,
            rolling_restart: false,
            label_enable: false,
            disable_containers: vec![],
            scope: None,
            stop_timeout: default_stop_timeout(),
            lifecycle_hooks: false,
            run_once: false,
            watch_containers: vec![],
            ignore_compose_managed: true,
            registries: std::collections::HashMap::new(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Notifications
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone, Default)]
pub struct NotificationConfig {
    /// Generic webhook URLs (Slack, Discord, Teams, Gotify, …).
    /// Accepts any HTTP(S) URL; payload is a JSON object with a `text` field.
    #[serde(default)]
    pub urls: Vec<String>,

    /// Slack incoming-webhook URL.
    pub slack_webhook_url: Option<String>,
    /// Slack channel override, e.g. "#updates".
    pub slack_channel: Option<String>,
    /// Slack bot username. Default: "updock".
    pub slack_username: Option<String>,

    /// Gotify server URL.
    pub gotify_url: Option<String>,
    /// Gotify app token.
    pub gotify_token: Option<String>,

    /// SMTP email notification settings.
    pub email: Option<EmailConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EmailConfig {
    pub smtp_host: String,
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
    /// Skip TLS certificate verification.
    #[serde(default)]
    pub tls_skip_verify: bool,
}

// ──────────────────────────────────────────────────────────────────────────────
// Defaults
// ──────────────────────────────────────────────────────────────────────────────

fn default_bind() -> String {
    "0.0.0.0:8080".to_string()
}
fn default_webhook_path() -> String {
    "/webhook".to_string()
}
fn default_pid_file() -> PathBuf {
    PathBuf::from("/var/run/updock.pid")
}
fn default_log_dir() -> PathBuf {
    PathBuf::from("/var/log/updock")
}
fn default_docker_host() -> String {
    "unix:///var/run/docker.sock".to_string()
}
fn default_poll_interval() -> u64 {
    86400
}
fn default_stop_timeout() -> u64 {
    10
}
fn default_smtp_port() -> u16 {
    587
}
fn default_true() -> bool {
    true
}

// ──────────────────────────────────────────────────────────────────────────────
// Loading
// ──────────────────────────────────────────────────────────────────────────────

pub fn load(path: &std::path::Path) -> anyhow::Result<Config> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Cannot read config file {}: {}", path.display(), e))?;

    let cfg: Config = serde_yaml::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("Config parse error in {}: {}", path.display(), e))?;

    validate(&cfg)?;
    Ok(cfg)
}

fn validate(cfg: &Config) -> anyhow::Result<()> {
    for p in &cfg.projects {
        if p.harbor_repository.trim().is_empty() {
            anyhow::bail!("Project '{}': harbor_repository must not be empty", p.name);
        }
        if !p.compose_dir.exists() {
            anyhow::bail!(
                "Project '{}': compose_dir '{}' does not exist",
                p.name,
                p.compose_dir.display()
            );
        }
    }

    if let Some(ref sched) = cfg.watcher.schedule {
        // Validate the cron expression at startup.
        sched
            .parse::<cron::Schedule>()
            .map_err(|e| anyhow::anyhow!("Invalid watcher.schedule '{}': {}", sched, e))?;
    }

    Ok(())
}
