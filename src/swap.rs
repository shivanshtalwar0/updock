/// Start-first ("zero-downtime") container replacement.
///
/// The stop-first update (stop → remove → create → start) leaves a service with
/// nothing answering while its new container boots, kills requests in flight
/// after `stop_timeout`, and lost every network alias: `create_container` already
/// attaches the container to its `NetworkMode` network (without aliases), so the
/// later `connect_network` failed with "endpoint … already exists in network" and
/// the aliases were never restored.
///
/// Start-first instead:
///
/// 1. creates the new container under a temporary name, on the same networks
///    but **without** the service's aliases — nothing routes to it yet;
/// 2. starts it and waits until it is ready (its Docker HEALTHCHECK, an HTTP
///    probe from labels, or a grace delay);
/// 3. gives it the service's aliases (and the canonical name as an alias), so
///    both the old and the new container answer;
/// 4. waits a short drain delay, then stops the old container gracefully
///    (its stop signal, then `stop timeout`) and removes it;
/// 5. renames the new container to the canonical name.
///
/// If the new container never gets ready it is removed and the old one keeps
/// serving: nothing the members see changes.
///
/// A container that publishes host ports or pins a static IP cannot have a twin
/// running beside it; such containers fall back to stop-first (`plan` says why).
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use bollard::container::{Config as ContainerConfig, NetworkingConfig};
use bollard::models::{ContainerInspectResponse, EndpointSettings};
use tracing::{info, warn};

// ── labels ─────────────────────────────────────────────────────────────────────

/// Opt a container in to updates (with `watcher.label_enable`); the Watchtower label works too.
pub const LABEL_UPDOCK_ENABLE: &str = "updock.enable";
/// `start-first` (default) or `stop-first`.
pub const LABEL_STRATEGY: &str = "updock.strategy";
/// HTTP readiness probe: the container port to call (on the container's own IP).
pub const LABEL_HEALTH_PORT: &str = "updock.health.port";
/// HTTP readiness probe path (default `/`). Any answer below 500 counts as ready.
pub const LABEL_HEALTH_PATH: &str = "updock.health.path";
/// How long the new container may take to get ready, in seconds (default 120).
pub const LABEL_HEALTH_TIMEOUT: &str = "updock.health.timeout";
/// With no HEALTHCHECK and no probe: how long the new container must stay up (default 10 s).
pub const LABEL_HEALTH_GRACE: &str = "updock.health.grace";
/// After the new container answers on the service's names, before the old one is stopped (default 3 s).
pub const LABEL_DRAIN_DELAY: &str = "updock.drain.delay";
/// How long the old container gets to finish its requests after its stop signal (default 30 s).
pub const LABEL_STOP_TIMEOUT: &str = "updock.stop.timeout";

const DEFAULT_HEALTH_TIMEOUT: u64 = 120;
const DEFAULT_GRACE: u64 = 10;
const DEFAULT_DRAIN_DELAY: u64 = 3;
const DEFAULT_STOP_TIMEOUT: u64 = 30;
const POLL: Duration = Duration::from_millis(1000);

// ── the plan ───────────────────────────────────────────────────────────────────

/// How the new container proves it is ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    /// The image's (or container's) Docker HEALTHCHECK reports `healthy`.
    Healthcheck,
    /// `GET http://<container ip>:<port><path>` answers below 500.
    Http { port: u16, path: String },
    /// Still running after this long.
    Grace(Duration),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapPlan {
    /// The service's name, without the leading `/`.
    pub canonical: String,
    /// The new container's name until the swap is done.
    pub temp: String,
    pub readiness: Readiness,
    pub health_timeout: Duration,
    pub drain_delay: Duration,
    pub stop_timeout: Duration,
}

/// Why a container is replaced stop-first instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopFirst(pub String);

fn label_secs(labels: &HashMap<String, String>, key: &str, default: u64) -> Duration {
    Duration::from_secs(labels.get(key).and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default))
}

fn labels_of(c: &ContainerInspectResponse) -> HashMap<String, String> {
    c.config.as_ref().and_then(|cfg| cfg.labels.clone()).unwrap_or_default()
}

/// The start-first plan for `c`, or why it must be replaced stop-first.
/// `default_stop_timeout` is `watcher.stop_timeout`; the label wins.
pub fn plan(c: &ContainerInspectResponse, default_stop_timeout: u64) -> std::result::Result<SwapPlan, StopFirst> {
    let labels = labels_of(c);
    if labels.get(LABEL_STRATEGY).map(|s| s.trim()) == Some("stop-first") {
        return Err(StopFirst("updock.strategy=stop-first".into()));
    }
    let host = c.host_config.as_ref();
    let published = host
        .and_then(|h| h.port_bindings.as_ref())
        .map(|b| b.values().any(|v| v.as_ref().is_some_and(|list| !list.is_empty())))
        .unwrap_or(false);
    if published {
        return Err(StopFirst("it publishes host ports, which two containers cannot share".into()));
    }
    let mode = host.and_then(|h| h.network_mode.clone()).unwrap_or_default();
    if mode == "host" || mode == "none" || mode.starts_with("container:") {
        return Err(StopFirst(format!("network mode `{mode}` has no names to move")));
    }
    let networks = c.network_settings.as_ref().and_then(|n| n.networks.clone()).unwrap_or_default();
    if networks.is_empty() {
        return Err(StopFirst("it is on no network".into()));
    }
    let static_ip = networks.values().any(|e| {
        e.ipam_config
            .as_ref()
            .is_some_and(|ipam| ipam.ipv4_address.as_deref().is_some_and(|a| !a.is_empty()) || ipam.ipv6_address.as_deref().is_some_and(|a| !a.is_empty()))
    });
    if static_ip {
        return Err(StopFirst("it pins a static IP, which two containers cannot share".into()));
    }

    let canonical = c.name.as_deref().unwrap_or("unknown").trim_start_matches('/').to_string();
    let short = c.id.as_deref().unwrap_or("new").chars().take(8).collect::<String>();
    let has_healthcheck = c
        .config
        .as_ref()
        .and_then(|cfg| cfg.healthcheck.as_ref())
        .and_then(|h| h.test.as_ref())
        .is_some_and(|t| !t.is_empty() && t.first().map(|s| s.as_str()) != Some("NONE"));
    let readiness = if let Some(port) = labels.get(LABEL_HEALTH_PORT).and_then(|p| p.trim().parse::<u16>().ok()) {
        let path = labels.get(LABEL_HEALTH_PATH).map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into());
        Readiness::Http { port, path: if path.starts_with('/') { path } else { format!("/{path}") } }
    } else if has_healthcheck {
        Readiness::Healthcheck
    } else {
        Readiness::Grace(label_secs(&labels, LABEL_HEALTH_GRACE, DEFAULT_GRACE))
    };
    Ok(SwapPlan {
        temp: format!("{canonical}-updock-{short}"),
        canonical,
        readiness,
        health_timeout: label_secs(&labels, LABEL_HEALTH_TIMEOUT, DEFAULT_HEALTH_TIMEOUT),
        drain_delay: label_secs(&labels, LABEL_DRAIN_DELAY, DEFAULT_DRAIN_DELAY),
        // The old container finishes its requests: never less than the drain default unless a label says so.
        stop_timeout: label_secs(&labels, LABEL_STOP_TIMEOUT, default_stop_timeout.max(DEFAULT_STOP_TIMEOUT)),
    })
}

// ── container config ───────────────────────────────────────────────────────────

/// One network endpoint as a new container should join it: the old one's aliases
/// (without the old container's id, deduplicated, `canonical` first) when
/// `with_aliases`, none otherwise. Addresses, ids and MACs the old endpoint was
/// given are left for Docker to assign.
pub fn endpoint_for(old: &EndpointSettings, old_id: &str, canonical: &str, with_aliases: bool) -> EndpointSettings {
    let aliases = if with_aliases {
        let short = old_id.chars().take(12).collect::<String>();
        let mut out: Vec<String> = vec![canonical.to_string()];
        for a in old.aliases.iter().flatten() {
            let a = a.trim();
            if a.is_empty() || a == short || a == old_id || out.iter().any(|o| o == a) {
                continue;
            }
            out.push(a.to_string());
        }
        Some(out)
    } else {
        None
    };
    EndpointSettings {
        aliases,
        links: old.links.clone(),
        driver_opts: old.driver_opts.clone(),
        ..Default::default()
    }
}

/// Every network of `c`, as the new container joins them at create.
pub fn endpoints_for(c: &ContainerInspectResponse, canonical: &str, with_aliases: bool) -> HashMap<String, EndpointSettings> {
    let old_id = c.id.as_deref().unwrap_or_default();
    c.network_settings
        .as_ref()
        .and_then(|n| n.networks.clone())
        .unwrap_or_default()
        .iter()
        .map(|(name, e)| (name.clone(), endpoint_for(e, old_id, canonical, with_aliases)))
        .collect()
}

/// The create request for a replacement of `c` running `image`: its whole
/// configuration (user, healthcheck, stop signal… which the old recreate dropped)
/// and every network at once, so nothing has to be reconnected afterwards.
pub fn replacement_config(c: &ContainerInspectResponse, image: &str, canonical: &str, with_aliases: bool) -> ContainerConfig<String> {
    let cfg = c.config.clone().unwrap_or_default();
    ContainerConfig {
        image: Some(image.to_string()),
        env: cfg.env,
        cmd: cfg.cmd,
        entrypoint: cfg.entrypoint,
        exposed_ports: cfg.exposed_ports,
        volumes: cfg.volumes,
        working_dir: cfg.working_dir,
        labels: cfg.labels,
        user: cfg.user,
        healthcheck: cfg.healthcheck,
        stop_signal: cfg.stop_signal,
        stop_timeout: cfg.stop_timeout,
        tty: cfg.tty,
        open_stdin: cfg.open_stdin,
        stdin_once: cfg.stdin_once,
        attach_stdin: cfg.attach_stdin,
        attach_stdout: cfg.attach_stdout,
        attach_stderr: cfg.attach_stderr,
        domainname: cfg.domainname,
        shell: cfg.shell,
        host_config: c.host_config.clone(),
        networking_config: Some(NetworkingConfig { endpoints_config: endpoints_for(c, canonical, with_aliases) }),
        ..Default::default()
    }
}

// ── docker operations ──────────────────────────────────────────────────────────

/// What a swap asks of Docker (and of time), so it can be tested without one.
#[allow(async_fn_in_trait)]
pub trait SwapDocker {
    async fn create(&self, name: &str, config: ContainerConfig<String>) -> Result<()>;
    async fn start(&self, name: &str) -> Result<()>;
    async fn inspect(&self, name: &str) -> Result<ContainerInspectResponse>;
    async fn stop(&self, name: &str, timeout: Duration) -> Result<()>;
    async fn remove(&self, name: &str) -> Result<()>;
    async fn rename(&self, name: &str, to: &str) -> Result<()>;
    async fn disconnect(&self, network: &str, container: &str) -> Result<()>;
    async fn connect(&self, network: &str, container: &str, endpoint: EndpointSettings) -> Result<()>;
    /// `GET url`: true when it answered below 500.
    async fn probe(&self, url: &str) -> bool;
    async fn sleep(&self, d: Duration);
}

/// The first IP the container has on any network.
fn ip_of(c: &ContainerInspectResponse) -> Option<String> {
    c.network_settings
        .as_ref()
        .and_then(|n| n.networks.as_ref())
        .and_then(|nets| nets.values().filter_map(|e| e.ip_address.clone()).find(|ip| !ip.is_empty()))
}

/// Wait until `name` is ready by `readiness`, or fail (it exited, reported
/// unhealthy past its start period, or `timeout` passed).
pub async fn wait_ready<D: SwapDocker>(d: &D, name: &str, readiness: &Readiness, timeout: Duration) -> Result<()> {
    let mut waited = Duration::ZERO;
    loop {
        let c = d.inspect(name).await?;
        let state = c.state.clone().unwrap_or_default();
        if state.running != Some(true) {
            return Err(anyhow!("{name} stopped while starting (exit code {:?})", state.exit_code));
        }
        let ready = match readiness {
            Readiness::Healthcheck => {
                let status = state.health.as_ref().and_then(|h| h.status).map(|s| s.to_string()).unwrap_or_default();
                if status == "unhealthy" {
                    return Err(anyhow!("{name} reported unhealthy"));
                }
                status == "healthy"
            }
            Readiness::Http { port, path } => match ip_of(&c) {
                Some(ip) => d.probe(&format!("http://{ip}:{port}{path}")).await,
                None => false,
            },
            Readiness::Grace(g) => waited >= *g,
        };
        if ready {
            return Ok(());
        }
        if waited >= timeout {
            return Err(anyhow!("{name} was not ready within {}s", timeout.as_secs()));
        }
        d.sleep(POLL).await;
        waited += POLL;
    }
}

/// Replace `old` (inspected) by a container of `image`, start-first. On any
/// failure before the old container is stopped, the new one is removed and the
/// old one keeps serving.
pub async fn swap<D: SwapDocker>(d: &D, old: &ContainerInspectResponse, image: &str, plan: &SwapPlan) -> Result<()> {
    let canonical = plan.canonical.as_str();
    let temp = plan.temp.as_str();

    // A twin left by an interrupted swap would block the name.
    let _ = d.remove(temp).await;

    d.create(temp, replacement_config(old, image, canonical, false)).await.context("create the new container")?;
    if let Err(e) = d.start(temp).await {
        let _ = d.remove(temp).await;
        return Err(e.context("start the new container"));
    }
    info!(container = %canonical, new = %temp, readiness = ?plan.readiness, "new container started; waiting until it is ready");
    if let Err(e) = wait_ready(d, temp, &plan.readiness, plan.health_timeout).await {
        warn!(container = %canonical, err = %e, "new container never got ready; keeping the old one");
        let _ = d.stop(temp, Duration::from_secs(5)).await;
        let _ = d.remove(temp).await;
        return Err(e);
    }

    // It answers on the service's names alongside the old container.
    for (network, endpoint) in endpoints_for(old, canonical, true) {
        let moved = async {
            d.disconnect(&network, temp).await?;
            d.connect(&network, temp, endpoint).await
        }
        .await;
        if let Err(e) = moved {
            warn!(container = %canonical, network = %network, err = %e, "could not give the new container its names; keeping the old one");
            let _ = d.stop(temp, Duration::from_secs(5)).await;
            let _ = d.remove(temp).await;
            return Err(e);
        }
    }
    info!(container = %canonical, "new container ready and answering; draining the old one");
    d.sleep(plan.drain_delay).await;

    let old_id = old.id.clone().unwrap_or_else(|| canonical.to_string());
    d.stop(&old_id, plan.stop_timeout).await.context("stop the old container")?;
    d.remove(&old_id).await.context("remove the old container")?;
    d.rename(temp, canonical).await.context("give the new container its name")?;
    Ok(())
}

// ── bollard ────────────────────────────────────────────────────────────────────

/// Docker itself.
pub struct Bollard<'a> {
    pub docker: &'a bollard::Docker,
    pub http: reqwest::Client,
}

impl SwapDocker for Bollard<'_> {
    async fn create(&self, name: &str, config: ContainerConfig<String>) -> Result<()> {
        self.docker
            .create_container(Some(bollard::container::CreateContainerOptions { name: name.to_string(), platform: None }), config)
            .await?;
        Ok(())
    }
    async fn start(&self, name: &str) -> Result<()> {
        self.docker.start_container(name, None::<bollard::container::StartContainerOptions<String>>).await?;
        Ok(())
    }
    async fn inspect(&self, name: &str) -> Result<ContainerInspectResponse> {
        Ok(self.docker.inspect_container(name, Some(bollard::container::InspectContainerOptions { size: false })).await?)
    }
    async fn stop(&self, name: &str, timeout: Duration) -> Result<()> {
        self.docker.stop_container(name, Some(bollard::container::StopContainerOptions { t: timeout.as_secs() as i64 })).await?;
        Ok(())
    }
    async fn remove(&self, name: &str) -> Result<()> {
        self.docker
            .remove_container(name, Some(bollard::container::RemoveContainerOptions { v: false, force: true, link: false }))
            .await?;
        Ok(())
    }
    async fn rename(&self, name: &str, to: &str) -> Result<()> {
        self.docker.rename_container(name, bollard::container::RenameContainerOptions { name: to.to_string() }).await?;
        Ok(())
    }
    async fn disconnect(&self, network: &str, container: &str) -> Result<()> {
        self.docker
            .disconnect_network(network, bollard::network::DisconnectNetworkOptions { container: container.to_string(), force: true })
            .await?;
        Ok(())
    }
    async fn connect(&self, network: &str, container: &str, endpoint: EndpointSettings) -> Result<()> {
        self.docker
            .connect_network(network, bollard::network::ConnectNetworkOptions { container: container.to_string(), endpoint_config: endpoint })
            .await?;
        Ok(())
    }
    async fn probe(&self, url: &str) -> bool {
        match self.http.get(url).timeout(Duration::from_secs(5)).send().await {
            Ok(r) => r.status().as_u16() < 500,
            Err(_) => false,
        }
    }
    async fn sleep(&self, d: Duration) {
        tokio::time::sleep(d).await;
    }
}
