/// Watchtower-style automatic container image update engine.
///
/// This module:
/// - Connects to the Docker daemon via bollard
/// - Polls for containers whose base image has a newer digest available
/// - Re-creates those containers with the same configuration
/// - Runs lifecycle hooks (pre-check / pre-update / post-update / post-check)
/// - Emits metrics and notifications
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bollard::container::{
    CreateContainerOptions, InspectContainerOptions,
    KillContainerOptions, ListContainersOptions, RemoveContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::auth::DockerCredentials;
use bollard::image::{CreateImageOptions, RemoveImageOptions};
use bollard::models::ContainerInspectResponse;
use bollard::Docker;
use futures_util::stream::StreamExt;
use tracing::{debug, error, info, warn};

use crate::config::WatcherConfig;
use crate::metrics::Metrics;
use crate::notify::Notifier;

// ──────────────────────────────────────────────────────────────────────────────
// Label constants (Watchtower-compatible)
// ──────────────────────────────────────────────────────────────────────────────

pub const LABEL_ENABLE: &str = "com.centurylinklabs.watchtower.enable";
pub const LABEL_MONITOR_ONLY: &str = "com.centurylinklabs.watchtower.monitor-only";
pub const LABEL_NO_PULL: &str = "com.centurylinklabs.watchtower.no-pull";
pub const LABEL_STOP_SIGNAL: &str = "com.centurylinklabs.watchtower.stop-signal";
pub const LABEL_SCOPE: &str = "com.centurylinklabs.watchtower.scope";
pub const LABEL_DEPENDS_ON: &str = "com.centurylinklabs.watchtower.depends-on";
pub const LABEL_PRE_CHECK: &str = "com.centurylinklabs.watchtower.lifecycle.pre-check";
pub const LABEL_PRE_UPDATE: &str = "com.centurylinklabs.watchtower.lifecycle.pre-update";
pub const LABEL_POST_UPDATE: &str = "com.centurylinklabs.watchtower.lifecycle.post-update";
pub const LABEL_POST_CHECK: &str = "com.centurylinklabs.watchtower.lifecycle.post-check";

/// Docker Compose always sets this label on every container it manages.
/// We use it to detect and skip Compose-managed containers when
/// `watcher.ignore_compose_managed = true`.
pub const LABEL_COMPOSE_PROJECT: &str = "com.docker.compose.project";

// ──────────────────────────────────────────────────────────────────────────────
// Session report (used by notifier)
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct SessionReport {
    pub scanned: usize,
    pub updated: Vec<String>,
    pub failed: Vec<String>,
    pub skipped: Vec<String>,
    pub fresh: Vec<String>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Watcher
// ──────────────────────────────────────────────────────────────────────────────

pub struct Watcher {
    docker: Docker,
    cfg: Arc<WatcherConfig>,
    metrics: Arc<Metrics>,
    notifier: Arc<Notifier>,
}

impl Watcher {
    /// Connect to the Docker daemon and construct a new `Watcher`.
    pub fn new(
        cfg: Arc<WatcherConfig>,
        metrics: Arc<Metrics>,
        notifier: Arc<Notifier>,
    ) -> Result<Self> {
        let docker = connect_docker(&cfg.docker_host)?;
        Ok(Self { docker, cfg, metrics, notifier })
    }

    /// Execute one full scan-and-update cycle.
    pub async fn run_once(&self) -> Result<SessionReport> {
        self.metrics.inc_scans_total();
        let containers = self.list_watched_containers().await?;

        if containers.is_empty() {
            info!("watcher: no containers to watch");
            self.metrics.inc_scans_skipped();
            return Ok(SessionReport::default());
        }

        let mut report = SessionReport {
            scanned: containers.len(),
            ..Default::default()
        };

        // ── pre-check hooks ──────────────────────────────────────────────────
        if self.cfg.lifecycle_hooks {
            for c in &containers {
                self.run_lifecycle_hook(c, LABEL_PRE_CHECK, "pre-check").await;
            }
        }

        // ── determine which containers have updates ──────────────────────────
        let mut to_update: Vec<ContainerInspectResponse> = Vec::new();

        for c in &containers {
            match self.has_update(c).await {
                Ok(true) => to_update.push(c.clone()),
                Ok(false) => {
                    let name = container_name(c);
                    debug!(container = %name, "no update available");
                    report.fresh.push(name);
                }
                Err(e) => {
                    let name = container_name(c);
                    warn!(container = %name, err = %e, "error checking for update");
                    report.skipped.push(name);
                }
            }
        }

        // ── update containers ────────────────────────────────────────────────
        // Sort by depends-on labels so that dependencies are updated first.
        let to_update = sort_by_depends_on(to_update);

        for c in &to_update {
            let name = container_name(c);

            if is_monitor_only(c, &self.cfg) {
                info!(container = %name, "monitor-only: update available but not applied");
                report.skipped.push(name.clone());
                self.metrics.set_updated(0); // don't count
                continue;
            }

            if self.cfg.rolling_restart {
                // small gap between rolling restarts
                tokio::time::sleep(Duration::from_secs(2)).await;
            }

            match self.update_container(c).await {
                Ok(()) => {
                    info!(container = %name, "container updated successfully");
                    report.updated.push(name.clone());
                    self.metrics.inc_containers_updated();
                }
                Err(e) => {
                    error!(container = %name, err = %e, "container update failed");
                    report.failed.push(name.clone());
                    self.metrics.inc_containers_failed();
                }
            }
        }

        // ── post-check hooks ─────────────────────────────────────────────────
        if self.cfg.lifecycle_hooks {
            for c in &containers {
                self.run_lifecycle_hook(c, LABEL_POST_CHECK, "post-check").await;
            }
        }

        self.metrics.set_scanned(report.scanned);
        self.metrics.set_updated(report.updated.len());
        self.metrics.set_failed(report.failed.len());

        // ── notifications ────────────────────────────────────────────────────
        self.notifier.send_report(&report).await;

        Ok(report)
    }

    /// Pull `name`'s image and replace it now (start-first when it can be),
    /// whatever its labels say: the manual deploy (`updock swap <name>`).
    pub async fn replace_now(&self, name: &str) -> Result<()> {
        let c = self
            .docker
            .inspect_container(name, Some(InspectContainerOptions { size: false }))
            .await
            .with_context(|| format!("no container named {name}"))?;
        self.update_container(&c).await
    }

    // ── internal helpers ─────────────────────────────────────────────────────

    /// List all containers that should be watched according to config + labels.
    async fn list_watched_containers(&self) -> Result<Vec<ContainerInspectResponse>> {
        let mut filters: HashMap<&str, Vec<&str>> = HashMap::new();

        if self.cfg.include_stopped {
            // "created", "restarting", "running", "paused", "exited"
            filters.insert("status", vec!["running", "exited", "created", "paused", "restarting"]);
        } else {
            filters.insert("status", vec!["running"]);
        }

        let list = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: self.cfg.include_stopped,
                filters,
                ..Default::default()
            }))
            .await
            .context("list_containers failed")?;

        let mut result = Vec::new();

        for summary in list {
            let id = match &summary.id {
                Some(id) => id.clone(),
                None => continue,
            };

            let info = self
                .docker
                .inspect_container(&id, Some(InspectContainerOptions { size: false }))
                .await
                .context("inspect_container failed")?;

            if !self.should_watch(&info) {
                continue;
            }

            result.push(info);
        }

        Ok(result)
    }

    /// Apply all filtering rules to decide whether a container should be watched.
    fn should_watch(&self, c: &ContainerInspectResponse) -> bool {
        let labels = container_labels(c);
        let name = container_name(c);

        // Skip containers managed by Docker Compose to avoid fighting with
        // the webhook compose-up path.  Docker Compose always sets the
        // `com.docker.compose.project` label on every container it creates.
        if self.cfg.ignore_compose_managed && labels.contains_key(LABEL_COMPOSE_PROJECT) {
            debug!(
                container = %name,
                "skipping compose-managed container (ignore_compose_managed=true)"
            );
            return false;
        }

        // Explicit name allow-list.
        if !self.cfg.watch_containers.is_empty()
            && !self.cfg.watch_containers.iter().any(|n| {
                n.trim_start_matches('/').eq_ignore_ascii_case(name.trim_start_matches('/'))
            })
        {
            return false;
        }

        // Explicit disable list.
        if self.cfg.disable_containers.iter().any(|n| {
            n.trim_start_matches('/').eq_ignore_ascii_case(name.trim_start_matches('/'))
        }) {
            return false;
        }

        // Watchtower enable label.
        // Opt-in label: `updock.enable` or Watchtower's.
        let enabled = |key: &str| labels.get(key).map(|s| s.trim().to_ascii_lowercase());
        let opted = [crate::swap::LABEL_UPDOCK_ENABLE, LABEL_ENABLE].iter().find_map(|k| enabled(k));
        if self.cfg.label_enable {
            if opted.as_deref() != Some("true") {
                return false;
            }
        } else if opted.as_deref() == Some("false") {
            // If label-enable mode is off, exclude containers explicitly disabled.
            return false;
        }

        // Scope filter.
        if let Some(ref scope) = self.cfg.scope {
            let container_scope = labels.get(LABEL_SCOPE).map(|s| s.as_str()).unwrap_or("");
            if container_scope != scope.as_str() {
                return false;
            }
        }

        true
    }

    /// Returns `true` if the container's image has a newer version available.
    async fn has_update(&self, c: &ContainerInspectResponse) -> Result<bool> {
        let image_name = container_image(c);
        let labels = container_labels(c);

        // Respect per-container no-pull label.
        let no_pull = self.cfg.no_pull
            || labels.get(LABEL_NO_PULL).map(|s| s.as_str()) == Some("true");

        if no_pull {
            // Compare local image ID with running container image ID.
            let running_id = c
                .image
                .as_deref()
                .unwrap_or_default()
                .to_string();
            let local = self
                .docker
                .inspect_image(&image_name)
                .await
                .context("inspect_image failed")?;
            let local_id = local.id.unwrap_or_default();
            return Ok(local_id != running_id && !running_id.is_empty());
        }

        // A locally built image (no registry digest) has nothing to pull: checking it
        // only fails, every cycle. It is skipped quietly.
        if let Ok(local) = self.docker.inspect_image(&image_name).await {
            if local.repo_digests.as_ref().is_none_or(|d| d.is_empty()) {
                debug!(image = %image_name, "locally built image: no registry to check");
                return Ok(false);
            }
        }

        // Resolve credentials for this image's registry.
        let creds = self.registry_credentials(&image_name);

        // Pull the image and compare the image ID.
        // We resolve the running container's current image ID by inspecting the
        // image by name *before* pulling, then compare against the ID *after*
        // the pull.  If the registry served a newer manifest, the IDs differ.
        let pre_pull_id = self
            .docker
            .inspect_image(&image_name)
            .await
            .ok()
            .and_then(|i| i.id)
            .unwrap_or_default();

        let pulled_id = self.pull_image(&image_name, creds).await?;

        Ok(pulled_id != pre_pull_id && !pre_pull_id.is_empty())
    }

    /// Pull an image; return its new image ID.
    ///
    /// `credentials` are passed directly to the Docker daemon's pull API
    /// (`X-Registry-Auth` header).  Pass `None` for public images.
    async fn pull_image(&self, image: &str, credentials: Option<DockerCredentials>) -> Result<String> {
        let (repo, tag) = split_image_tag(image);

        let mut stream = self.docker.create_image(
            Some(CreateImageOptions {
                from_image: repo,
                tag,
                ..Default::default()
            }),
            None,
            credentials,
        );

        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    if let Some(ref status) = info.status {
                        debug!(image = %image, status = %status, "pull");
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("pull failed for {}: {}", image, e));
                }
            }
        }

        let info = self.docker.inspect_image(image).await?;
        Ok(info.id.unwrap_or_default())
    }

    /// Stop, remove, recreate and start a container.
    async fn update_container(&self, c: &ContainerInspectResponse) -> Result<()> {
        let id = c.id.as_deref().unwrap_or_default();
        let name = container_name(c);
        let labels = container_labels(c);
        let _stop_signal = labels.get(LABEL_STOP_SIGNAL).cloned();

        // Was the container running before we touched it?
        let was_running = c
            .state
            .as_ref()
            .and_then(|s| s.running)
            .unwrap_or(false);

        // ── pre-update hook ──────────────────────────────────────────────────
        if self.cfg.lifecycle_hooks {
            self.run_lifecycle_hook(c, LABEL_PRE_UPDATE, "pre-update").await;
        }

        // ── start-first when the container allows it ─────────────────────────
        if was_running && !self.cfg.no_restart {
            match crate::swap::plan(c, self.cfg.stop_timeout) {
                Ok(plan) => return self.update_start_first(c, &plan).await,
                Err(crate::swap::StopFirst(why)) => {
                    warn!(container = %name, reason = %why, "replacing stop-first: the service is down until the new container starts");
                }
            }
        }

        // ── stop (custom signal if label is set, otherwise normal stop) ──────
        info!(container = %name, "stopping container");
        if let Some(ref signal) = _stop_signal {
            // Send the custom signal first, then wait via stop_container.
            let _ = self
                .docker
                .kill_container(id, Some(KillContainerOptions { signal: signal.as_str() }))
                .await;
        }
        self.docker
            .stop_container(
                id,
                Some(StopContainerOptions {
                    t: self.cfg.stop_timeout as i64,
                }),
            )
            .await
            .context("stop_container failed")?;

        // ── capture what the recreate needs before removal ───────────────────
        let image_name = container_image(c);

        // Capture the current image ID before pulling/removing, so we can
        // clean it up afterwards if watcher.cleanup = true.
        let old_image_id: Option<String> = self
            .docker
            .inspect_image(&image_name)
            .await
            .ok()
            .and_then(|i| i.id);

        // ── remove ───────────────────────────────────────────────────────────
        self.docker
            .remove_container(
                id,
                Some(RemoveContainerOptions {
                    v: self.cfg.remove_volumes,
                    force: false,
                    link: false,
                }),
            )
            .await
            .context("remove_container failed")?;

        // ── pull fresh image ─────────────────────────────────────────────────
        if !self.cfg.no_pull {
            let creds = self.registry_credentials(&image_name);
            self.pull_image(&image_name, creds).await?;
        }

        // ── re-create ────────────────────────────────────────────────────────
        if !self.cfg.no_restart {
            info!(container = %name, image = %image_name, "recreating container");

            // The whole configuration, every network with its aliases included at
            // create: attaching them afterwards failed ("endpoint already exists" on
            // the NetworkMode network) and lost the aliases.
            let new_config = crate::swap::replacement_config(c, &image_name, name.trim_start_matches('/'), true);

            let clean_name = name.trim_start_matches('/').to_string();

            self.docker
                .create_container(
                    Some(CreateContainerOptions {
                        name: clean_name.clone(),
                        platform: None,
                    }),
                    new_config,
                )
                .await
                .context("create_container failed")?;

            // Start the container if:
            //   - it was running before the update, OR
            //   - revive_stopped is true (start even previously-stopped containers)
            let should_start = was_running || self.cfg.revive_stopped;
            if should_start {
                self.docker
                    .start_container(&clean_name, None::<StartContainerOptions<String>>)
                    .await
                    .context("start_container failed")?;
            } else {
                info!(container = %clean_name, "container was stopped; leaving it stopped after update");
            }

        }

        // ── post-update hook ─────────────────────────────────────────────────
        if self.cfg.lifecycle_hooks {
            // Re-inspect to get the new container info.
            if let Ok(new_c) = self
                .docker
                .inspect_container(
                    name.trim_start_matches('/'),
                    Some(InspectContainerOptions { size: false }),
                )
                .await
            {
                self.run_lifecycle_hook(&new_c, LABEL_POST_UPDATE, "post-update").await;
            }
        }

        // ── cleanup old image ────────────────────────────────────────────────
        if self.cfg.cleanup {
            if let Some(old_id) = old_image_id {
                if !old_id.is_empty() {
                    debug!(image_id = %old_id, "removing old image");
                    let _ = self
                        .docker
                        .remove_image(
                            &old_id,
                            Some(RemoveImageOptions {
                                force: false,
                                noprune: false,
                            }),
                            None,
                        )
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Replace `c` start-first (`swap.rs`): the old container serves until the new
    /// one is ready and answers on the service's names.
    async fn update_start_first(&self, c: &ContainerInspectResponse, plan: &crate::swap::SwapPlan) -> Result<()> {
        let image_name = container_image(c);
        let old_image_id: Option<String> = c.image.clone();
        if !self.cfg.no_pull {
            let creds = self.registry_credentials(&image_name);
            self.pull_image(&image_name, creds).await?;
        }
        info!(container = %plan.canonical, image = %image_name, temp = %plan.temp, "replacing start-first");
        let engine = crate::swap::Bollard { docker: &self.docker, http: reqwest::Client::new() };
        crate::swap::swap(&engine, c, &image_name, plan).await?;

        if self.cfg.lifecycle_hooks {
            if let Ok(new_c) = self.docker.inspect_container(&plan.canonical, Some(InspectContainerOptions { size: false })).await {
                self.run_lifecycle_hook(&new_c, LABEL_POST_UPDATE, "post-update").await;
            }
        }
        if self.cfg.cleanup {
            if let Some(old_id) = old_image_id.filter(|id| !id.is_empty()) {
                debug!(image_id = %old_id, "removing old image");
                let _ = self.docker.remove_image(&old_id, Some(RemoveImageOptions { force: false, noprune: false }), None).await;
            }
        }
        Ok(())
    }

    /// Execute a lifecycle hook script inside the container via `docker exec`.
    async fn run_lifecycle_hook(
        &self,
        c: &ContainerInspectResponse,
        label: &str,
        hook_type: &str,
    ) {
        let labels = container_labels(c);
        let cmd = match labels.get(label) {
            Some(v) => v.clone(),
            None => return,
        };

        let container_name = container_name(c);
        info!(container = %container_name, hook = %hook_type, cmd = %cmd, "running lifecycle hook");

        let exec = self
            .docker
            .create_exec(
                container_name.trim_start_matches('/'),
                bollard::exec::CreateExecOptions {
                    cmd: Some(vec!["sh", "-c", &cmd]),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    ..Default::default()
                },
            )
            .await;

        let exec_id = match exec {
            Ok(e) => e.id,
            Err(e) => {
                warn!(container = %container_name, hook = %hook_type, err = %e, "create_exec failed");
                return;
            }
        };

        match self.docker.start_exec(&exec_id, None).await {
            Ok(bollard::exec::StartExecResults::Attached { mut output, .. }) => {
                while let Some(Ok(msg)) = output.next().await {
                    debug!(container = %container_name, hook = %hook_type, output = %msg, "hook output");
                }
            }
            Ok(_) => {} // detached / no output
            Err(e) => {
                warn!(container = %container_name, hook = %hook_type, err = %e, "start_exec failed");
            }
        }
    }

    /// Look up `DockerCredentials` for the registry that hosts `image`.
    ///
    /// The registry hostname is extracted from the image reference, then
    /// matched (case-insensitively) against `cfg.registries`.  Returns
    /// `None` if no credentials are configured for that registry.
    fn registry_credentials(&self, image: &str) -> Option<DockerCredentials> {
        if self.cfg.registries.is_empty() {
            return None;
        }

        let host = registry_host_from_image(image);
        // Normalise both sides to lowercase for a case-insensitive match.
        let host_lower = host.to_lowercase();

        let reg = self
            .cfg
            .registries
            .iter()
            .find(|(k, _)| k.to_lowercase() == host_lower)
            .map(|(_, v)| v)?;

        // Build DockerCredentials.
        // username+password takes priority over a bare identity token.
        if reg.username.is_some() || reg.password.is_some() {
            Some(DockerCredentials {
                username: reg.username.clone(),
                password: reg.password.clone(),
                serveraddress: Some(host),
                ..Default::default()
            })
        } else if let Some(ref tok) = reg.token {
            Some(DockerCredentials {
                identitytoken: Some(tok.clone()),
                serveraddress: Some(host),
                ..Default::default()
            })
        } else {
            None
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Extract the registry hostname (and optional port) from an image reference.
///
/// Rules (matching Docker's own image-reference parsing):
///
/// - If the first path component contains a `.`, `:`, or is `localhost`, it is
///   the registry host.
/// - Otherwise the image is a Docker Hub short-form reference and the
///   canonical registry is `index.docker.io`.
///
/// Examples:
/// ```text
/// "nginx"                              → "index.docker.io"
/// "library/nginx:latest"              → "index.docker.io"
/// "registry.example.com/myapp:v1"     → "registry.example.com"
/// "registry.example.com:5000/myapp"   → "registry.example.com:5000"
/// "localhost:5000/myapp"              → "localhost:5000"
/// "ghcr.io/org/repo:tag"             → "ghcr.io"
/// ```
pub fn registry_host_from_image(image: &str) -> String {
    // Strip any digest or tag suffix — we only need the first path component.
    // Split off the first `/`-delimited component.
    let first = match image.find('/') {
        None => {
            // No slash at all → plain image name like "nginx" → Docker Hub.
            return "index.docker.io".to_string();
        }
        Some(pos) => &image[..pos],
    };

    // A component is a registry host if it contains '.', ':', or equals "localhost".
    let is_registry_host = first.contains('.')
        || first.contains(':')
        || first.eq_ignore_ascii_case("localhost");

    if is_registry_host {
        first.to_string()
    } else {
        // e.g. "library/nginx" or "user/repo" → Docker Hub
        "index.docker.io".to_string()
    }
}

fn connect_docker(host: &str) -> Result<Docker> {    if host.starts_with("unix://") || host == "unix:///var/run/docker.sock" {
        Docker::connect_with_unix(host, 120, bollard::API_DEFAULT_VERSION)
            .context("connect to docker unix socket")
    } else if host.starts_with("tcp://") || host.starts_with("http://") {
        Docker::connect_with_http(host, 120, bollard::API_DEFAULT_VERSION)
            .context("connect to docker tcp")
    } else {
        Docker::connect_with_local_defaults().context("connect to docker (local defaults)")
    }
}

fn container_name(c: &ContainerInspectResponse) -> String {
    c.name.as_deref().unwrap_or("unknown").to_string()
}

fn container_image(c: &ContainerInspectResponse) -> String {
    c.config
        .as_ref()
        .and_then(|cfg| cfg.image.as_deref())
        .unwrap_or("unknown")
        .to_string()
}

fn container_labels(c: &ContainerInspectResponse) -> HashMap<String, String> {
    c.config
        .as_ref()
        .and_then(|cfg| cfg.labels.clone())
        .unwrap_or_default()
}

fn is_monitor_only(c: &ContainerInspectResponse, cfg: &WatcherConfig) -> bool {
    let labels = container_labels(c);
    if cfg.monitor_only {
        return true;
    }
    labels.get(LABEL_MONITOR_ONLY).map(|s| s.as_str()) == Some("true")
}

/// Sort a list of containers so that dependencies come before dependents.
///
/// Uses Kahn's algorithm for a topological sort based on the
/// `com.centurylinklabs.watchtower.depends-on` label.
///
/// The label value is a comma-separated list of container names (with or
/// without leading `/`) that must be updated **before** this container.
///
/// If the label references a container that is not in the update set (e.g.
/// it doesn't need updating this cycle), the dependency is simply ignored.
/// Cycles are also handled gracefully: any containers involved in a cycle are
/// appended at the end in their original order.
fn sort_by_depends_on(
    containers: Vec<ContainerInspectResponse>,
) -> Vec<ContainerInspectResponse> {
    if containers.len() <= 1 {
        return containers;
    }

    // Map normalised name → index.
    let name_to_idx: HashMap<String, usize> = containers
        .iter()
        .enumerate()
        .map(|(i, c)| (container_name(c).trim_start_matches('/').to_lowercase(), i))
        .collect();

    // in_degree[i] = number of dependencies that are still pending.
    let mut in_degree: Vec<usize> = vec![0; containers.len()];
    // edges[dep_idx] = list of indices that depend on dep_idx.
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); containers.len()];

    for (i, c) in containers.iter().enumerate() {
        let labels = container_labels(c);
        if let Some(deps) = labels.get(LABEL_DEPENDS_ON) {
            for dep in deps.split(',').map(|s| s.trim().trim_start_matches('/').to_lowercase()) {
                if dep.is_empty() {
                    continue;
                }
                if let Some(&dep_idx) = name_to_idx.get(&dep) {
                    if dep_idx != i {
                        edges[dep_idx].push(i);
                        in_degree[i] += 1;
                    }
                }
                // If the dependency is not in the update set, ignore it.
            }
        }
    }

    // Kahn's BFS topological sort.
    let mut queue: std::collections::VecDeque<usize> = in_degree
        .iter()
        .enumerate()
        .filter(|&(_, &d)| d == 0)
        .map(|(i, _)| i)
        .collect();

    let mut sorted: Vec<ContainerInspectResponse> = Vec::with_capacity(containers.len());
    // Keep originals in an Option<> so we can move them out.
    let mut items: Vec<Option<ContainerInspectResponse>> =
        containers.into_iter().map(Some).collect();

    while let Some(idx) = queue.pop_front() {
        if let Some(c) = items[idx].take() {
            sorted.push(c);
        }
        for &next in &edges[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                queue.push_back(next);
            }
        }
    }

    // Append any containers involved in cycles (shouldn't happen in practice).
    for item in items.into_iter().flatten() {
        sorted.push(item);
    }

    sorted
}

/// Split "image:tag" → ("image", "tag"). Defaults to "latest".
fn split_image_tag(image: &str) -> (String, String) {
    // Handle registry prefix containing ':'
    // e.g. registry:5000/repo/image:tag
    let last_slash = image.rfind('/').map(|p| p + 1).unwrap_or(0);
    let after_slash = &image[last_slash..];

    if let Some(colon) = after_slash.rfind(':') {
        let tag = after_slash[colon + 1..].to_string();
        let repo = image[..last_slash + colon].to_string();
        (repo, tag)
    } else {
        (image.to_string(), "latest".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_image_tag() {
        assert_eq!(
            split_image_tag("nginx:latest"),
            ("nginx".into(), "latest".into())
        );
        assert_eq!(
            split_image_tag("registry.example.com:5000/myapp:v1.2"),
            ("registry.example.com:5000/myapp".into(), "v1.2".into())
        );
        assert_eq!(
            split_image_tag("myapp"),
            ("myapp".into(), "latest".into())
        );
    }

    // ── registry_host_from_image ──────────────────────────────────────────────

    #[test]
    fn registry_host_plain_name_is_docker_hub() {
        assert_eq!(registry_host_from_image("nginx"), "index.docker.io");
        assert_eq!(registry_host_from_image("nginx:latest"), "index.docker.io");
    }

    #[test]
    fn registry_host_user_repo_is_docker_hub() {
        assert_eq!(registry_host_from_image("library/nginx"), "index.docker.io");
        assert_eq!(registry_host_from_image("myuser/myapp:v1"), "index.docker.io");
    }

    #[test]
    fn registry_host_fqdn_registry() {
        assert_eq!(
            registry_host_from_image("registry.example.com/myapp:v1"),
            "registry.example.com"
        );
    }

    #[test]
    fn registry_host_fqdn_with_port() {
        assert_eq!(
            registry_host_from_image("registry.example.com:5000/myapp:v1"),
            "registry.example.com:5000"
        );
    }

    #[test]
    fn registry_host_ghcr() {
        assert_eq!(
            registry_host_from_image("ghcr.io/org/repo:tag"),
            "ghcr.io"
        );
    }

    #[test]
    fn registry_host_localhost() {
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
}
