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
    Config as ContainerConfig, CreateContainerOptions, InspectContainerOptions,
    KillContainerOptions, ListContainersOptions, RemoveContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::image::{CreateImageOptions, RemoveImageOptions};
use bollard::models::{ContainerInspectResponse, EndpointSettings, HostConfig};
use bollard::network::ConnectNetworkOptions;
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
        if self.cfg.label_enable {
            match labels.get(LABEL_ENABLE).map(|s| s.as_str()) {
                Some("true") => {}
                _ => return false,
            }
        } else {
            // If label-enable mode is off, exclude containers explicitly disabled.
            if labels.get(LABEL_ENABLE).map(|s| s.as_str()) == Some("false") {
                return false;
            }
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

        let pulled_id = self.pull_image(&image_name).await?;

        Ok(pulled_id != pre_pull_id && !pre_pull_id.is_empty())
    }

    /// Pull an image; return its new image ID.
    async fn pull_image(&self, image: &str) -> Result<String> {
        let (repo, tag) = split_image_tag(image);

        let mut stream = self.docker.create_image(
            Some(CreateImageOptions {
                from_image: repo,
                tag,
                ..Default::default()
            }),
            None,
            None,
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

        // ── pre-update hook ──────────────────────────────────────────────────
        if self.cfg.lifecycle_hooks {
            self.run_lifecycle_hook(c, LABEL_PRE_UPDATE, "pre-update").await;
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

        // ── capture config before removal ────────────────────────────────────
        let host_config: Option<HostConfig> = c.host_config.clone();
        let image_name = container_image(c);
        let env = c.config.as_ref().and_then(|cfg| cfg.env.clone());
        let cmd = c.config.as_ref().and_then(|cfg| cfg.cmd.clone());
        let entrypoint = c.config.as_ref().and_then(|cfg| cfg.entrypoint.clone());
        let exposed_ports = c.config.as_ref().and_then(|cfg| cfg.exposed_ports.clone());
        let volumes = c.config.as_ref().and_then(|cfg| cfg.volumes.clone());
        let working_dir = c.config.as_ref().and_then(|cfg| cfg.working_dir.clone());
        let original_labels = c.config.as_ref().and_then(|cfg| cfg.labels.clone());

        // Capture network memberships so we can restore them after recreation.
        let networks: HashMap<String, EndpointSettings> = c
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.clone())
            .unwrap_or_default();

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
            self.pull_image(&image_name).await?;
        }

        // ── re-create ────────────────────────────────────────────────────────
        if !self.cfg.no_restart {
            info!(container = %name, image = %image_name, "recreating container");

            let new_config = ContainerConfig {
                image: Some(image_name.clone()),
                env,
                cmd,
                entrypoint,
                exposed_ports,
                volumes,
                working_dir,
                labels: original_labels,
                host_config,
                ..Default::default()
            };

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

            self.docker
                .start_container(&clean_name, None::<StartContainerOptions<String>>)
                .await
                .context("start_container failed")?;

            // ── restore network memberships ──────────────────────────────────
            // Docker only connects the container to a single network during
            // create_container.  Re-attach all networks that were present
            // before removal so the container ends up on the same networks.
            for (network_name, endpoint) in &networks {
                let connect_cfg = ConnectNetworkOptions {
                    container: clean_name.clone(),
                    endpoint_config: endpoint.clone(),
                };
                if let Err(e) = self.docker.connect_network(network_name, connect_cfg).await {
                    // Non-fatal: log and continue; the container is already
                    // running and the main network (from host_config) is fine.
                    warn!(
                        container = %clean_name,
                        network = %network_name,
                        err = %e,
                        "failed to reconnect container to network"
                    );
                }
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
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn connect_docker(host: &str) -> Result<Docker> {
    if host.starts_with("unix://") || host == "unix:///var/run/docker.sock" {
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
}
