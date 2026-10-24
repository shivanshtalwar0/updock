/// Start-first replacement (`swap.rs`): the plan, the replacement's config,
/// and the swap itself against a fake Docker that records every call.
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use bollard::container::Config as ContainerConfig;
use bollard::models::{
    ContainerConfig as InspectConfig, ContainerInspectResponse, ContainerState, EndpointIpamConfig, EndpointSettings, HealthConfig,
    HostConfig, NetworkSettings, PortBinding,
};
use updock::swap::{self, Readiness, StopFirst, SwapDocker};

// ── fixtures ──────────────────────────────────────────────────────────────────

const OLD_ID: &str = "70592fbef513aaaabbbbccccdddd";

fn container(labels: &[(&str, &str)], aliases: Option<Vec<&str>>) -> ContainerInspectResponse {
    let mut networks = HashMap::new();
    networks.insert(
        "voxmin-net".to_string(),
        EndpointSettings {
            aliases: aliases.map(|a| a.into_iter().map(String::from).collect()),
            ip_address: Some("172.18.0.9".into()),
            endpoint_id: Some("old-endpoint".into()),
            mac_address: Some("02:42:ac:12:00:09".into()),
            ..Default::default()
        },
    );
    ContainerInspectResponse {
        id: Some(OLD_ID.into()),
        name: Some("/voxmin-backend-testing".into()),
        image: Some("sha256:old".into()),
        config: Some(InspectConfig {
            image: Some("registry.example.com/voxmin/voxmin-backend:testing".into()),
            labels: Some(labels.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
            user: Some("node".into()),
            env: Some(vec!["A=1".into()]),
            stop_signal: Some("SIGTERM".into()),
            ..Default::default()
        }),
        host_config: Some(HostConfig { network_mode: Some("voxmin-net".into()), ..Default::default() }),
        network_settings: Some(NetworkSettings { networks: Some(networks), ..Default::default() }),
        state: Some(ContainerState { running: Some(true), ..Default::default() }),
        ..Default::default()
    }
}

fn with_ports(mut c: ContainerInspectResponse) -> ContainerInspectResponse {
    let mut bindings = HashMap::new();
    bindings.insert("8073/tcp".to_string(), Some(vec![PortBinding { host_ip: Some(String::new()), host_port: Some("7073".into()) }]));
    c.host_config.as_mut().unwrap().port_bindings = Some(bindings);
    c
}

// ── plan ──────────────────────────────────────────────────────────────────────

#[test]
fn plan_defaults_to_start_first_with_a_grace_wait_and_a_30s_drain() {
    let p = swap::plan(&container(&[], Some(vec!["voxmin-backend-testing"])), 10).expect("start-first");
    assert_eq!(p.canonical, "voxmin-backend-testing");
    assert_eq!(p.temp, "voxmin-backend-testing-updock-70592fbe");
    assert_eq!(p.readiness, Readiness::Grace(Duration::from_secs(10)));
    assert_eq!(p.stop_timeout, Duration::from_secs(30), "the old container gets at least 30 s to finish");
    assert_eq!(p.health_timeout, Duration::from_secs(120));
}

#[test]
fn plan_reads_the_probe_and_timings_from_labels() {
    let c = container(
        &[
            ("updock.health.port", "8073"),
            ("updock.health.path", "health"),
            ("updock.health.timeout", "90"),
            ("updock.drain.delay", "5"),
            ("updock.stop.timeout", "45"),
        ],
        None,
    );
    let p = swap::plan(&c, 10).unwrap();
    assert_eq!(p.readiness, Readiness::Http { port: 8073, path: "/health".into() });
    assert_eq!((p.health_timeout.as_secs(), p.drain_delay.as_secs(), p.stop_timeout.as_secs()), (90, 5, 45));
}

#[test]
fn plan_waits_for_the_image_healthcheck_when_there_is_one() {
    let mut c = container(&[], None);
    c.config.as_mut().unwrap().healthcheck = Some(HealthConfig { test: Some(vec!["CMD-SHELL".into(), "true".into()]), ..Default::default() });
    assert_eq!(swap::plan(&c, 10).unwrap().readiness, Readiness::Healthcheck);
    c.config.as_mut().unwrap().healthcheck = Some(HealthConfig { test: Some(vec!["NONE".into()]), ..Default::default() });
    assert!(matches!(swap::plan(&c, 10).unwrap().readiness, Readiness::Grace(_)), "HEALTHCHECK NONE is no healthcheck");
}

#[test]
fn plan_falls_back_to_stop_first_when_two_containers_could_not_coexist() {
    let why = |c: &ContainerInspectResponse| swap::plan(c, 10).unwrap_err().0;
    assert!(why(&with_ports(container(&[], None))).contains("host ports"));
    assert!(why(&container(&[("updock.strategy", "stop-first")], None)).contains("stop-first"));
    let mut host = container(&[], None);
    host.host_config.as_mut().unwrap().network_mode = Some("host".into());
    assert!(why(&host).contains("network mode"));
    let mut pinned = container(&[], None);
    pinned.network_settings.as_mut().unwrap().networks.as_mut().unwrap().get_mut("voxmin-net").unwrap().ipam_config =
        Some(EndpointIpamConfig { ipv4_address: Some("172.18.0.50".into()), ..Default::default() });
    assert!(why(&pinned).contains("static IP"));
    assert!(matches!(swap::plan(&container(&[], None), 10), Ok(_)));
    let _ = StopFirst("x".into());
}

// ── the replacement's config ──────────────────────────────────────────────────

#[test]
fn the_replacement_joins_every_network_at_create_with_clean_aliases() {
    // The bug: aliases were lost on every update ("endpoint … already exists in network").
    let c = container(&[], Some(vec!["voxmin-backend-testing", "voxmin-backend-testing", "70592fbef513", "api", ""]));
    let cfg = swap::replacement_config(&c, "registry.example.com/voxmin/voxmin-backend:testing", "voxmin-backend-testing", true);
    let endpoints = cfg.networking_config.unwrap().endpoints_config;
    let net = &endpoints["voxmin-net"];
    assert_eq!(net.aliases.clone().unwrap(), vec!["voxmin-backend-testing".to_string(), "api".into()], "canonical first, old id and duplicates dropped");
    assert_eq!((net.ip_address.clone(), net.endpoint_id.clone(), net.mac_address.clone()), (None, None, None), "addresses are Docker's to assign");
    let quiet = swap::replacement_config(&c, "img", "voxmin-backend-testing", false).networking_config.unwrap().endpoints_config;
    assert_eq!(quiet["voxmin-net"].aliases, None, "the twin answers on no service name until it is ready");
}

#[test]
fn the_replacement_keeps_what_the_old_recreate_dropped() {
    let mut c = container(&[("a", "b")], None);
    c.config.as_mut().unwrap().healthcheck = Some(HealthConfig { test: Some(vec!["CMD".into(), "true".into()]), ..Default::default() });
    let cfg: ContainerConfig<String> = swap::replacement_config(&c, "img:new", "voxmin-backend-testing", true);
    assert_eq!(cfg.image.as_deref(), Some("img:new"));
    assert_eq!(cfg.user.as_deref(), Some("node"));
    assert_eq!(cfg.stop_signal.as_deref(), Some("SIGTERM"));
    assert!(cfg.healthcheck.is_some());
    assert_eq!(cfg.labels.unwrap()["a"], "b");
    assert_eq!(cfg.host_config.unwrap().network_mode.as_deref(), Some("voxmin-net"));
}

// ── the swap ──────────────────────────────────────────────────────────────────

/// A Docker whose new container gets ready after `ready_after` checks (never with `None`).
struct Fake {
    log: RefCell<Vec<String>>,
    checks: RefCell<u32>,
    ready_after: Option<u32>,
    readiness: Readiness,
}

impl Fake {
    fn new(ready_after: Option<u32>, readiness: Readiness) -> Self {
        Self { log: RefCell::new(vec![]), checks: RefCell::new(0), ready_after, readiness }
    }
    fn push(&self, s: String) {
        self.log.borrow_mut().push(s);
    }
    fn calls(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
    fn ready_now(&self) -> bool {
        let mut n = self.checks.borrow_mut();
        *n += 1;
        self.ready_after.is_some_and(|k| *n > k)
    }
}

impl SwapDocker for Fake {
    async fn create(&self, name: &str, config: ContainerConfig<String>) -> anyhow::Result<()> {
        let aliases = config.networking_config.unwrap().endpoints_config["voxmin-net"].aliases.clone();
        self.push(format!("create {name} aliases={aliases:?}"));
        Ok(())
    }
    async fn start(&self, name: &str) -> anyhow::Result<()> {
        self.push(format!("start {name}"));
        Ok(())
    }
    async fn inspect(&self, name: &str) -> anyhow::Result<ContainerInspectResponse> {
        let mut c = container(&[], None);
        c.name = Some(format!("/{name}"));
        if matches!(self.readiness, Readiness::Healthcheck) {
            let status = if self.ready_now() { bollard::models::HealthStatusEnum::HEALTHY } else { bollard::models::HealthStatusEnum::STARTING };
            c.state.as_mut().unwrap().health = Some(bollard::models::Health { status: Some(status), ..Default::default() });
        }
        Ok(c)
    }
    async fn stop(&self, name: &str, timeout: Duration) -> anyhow::Result<()> {
        self.push(format!("stop {name} {}s", timeout.as_secs()));
        Ok(())
    }
    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        self.push(format!("remove {name}"));
        Ok(())
    }
    async fn rename(&self, name: &str, to: &str) -> anyhow::Result<()> {
        self.push(format!("rename {name} -> {to}"));
        Ok(())
    }
    async fn disconnect(&self, network: &str, container: &str) -> anyhow::Result<()> {
        self.push(format!("disconnect {container} from {network}"));
        Ok(())
    }
    async fn connect(&self, network: &str, container: &str, endpoint: EndpointSettings) -> anyhow::Result<()> {
        self.push(format!("connect {container} to {network} aliases={:?}", endpoint.aliases));
        Ok(())
    }
    async fn probe(&self, url: &str) -> bool {
        self.push(format!("probe {url}"));
        self.ready_now()
    }
    async fn sleep(&self, _d: Duration) {}
}

fn plan_for(readiness: Readiness) -> swap::SwapPlan {
    let mut p = swap::plan(&container(&[], Some(vec!["voxmin-backend-testing"])), 10).unwrap();
    p.readiness = readiness;
    p.health_timeout = Duration::from_secs(5);
    p
}

#[tokio::test]
async fn the_old_container_serves_until_the_new_one_is_ready_and_answering() {
    let readiness = Readiness::Http { port: 8073, path: "/health".into() };
    let fake = Fake::new(Some(2), readiness.clone());
    let old = container(&[], Some(vec!["voxmin-backend-testing"]));
    swap::swap(&fake, &old, "img:new", &plan_for(readiness)).await.expect("swapped");
    let calls = fake.calls();
    let t = "voxmin-backend-testing-updock-70592fbe";
    let expected = vec![
        format!("remove {t}"),
        format!("create {t} aliases=None"),
        format!("start {t}"),
        format!("probe http://172.18.0.9:8073/health"),
        format!("probe http://172.18.0.9:8073/health"),
        format!("probe http://172.18.0.9:8073/health"),
        format!("disconnect {t} from voxmin-net"),
        format!("connect {t} to voxmin-net aliases=Some([\"voxmin-backend-testing\"])"),
        format!("stop {OLD_ID} 30s"),
        format!("remove {OLD_ID}"),
        format!("rename {t} -> voxmin-backend-testing"),
    ];
    assert_eq!(calls, expected);
}

#[tokio::test]
async fn a_new_container_that_never_gets_ready_is_removed_and_the_old_one_is_untouched() {
    let readiness = Readiness::Http { port: 8073, path: "/".into() };
    let fake = Fake::new(None, readiness.clone());
    let old = container(&[], None);
    let err = swap::swap(&fake, &old, "img:new", &plan_for(readiness)).await.unwrap_err();
    assert!(err.to_string().contains("not ready"), "{err}");
    let calls = fake.calls();
    assert!(calls.iter().all(|c| !c.contains(OLD_ID)), "the old container is never stopped or removed: {calls:?}");
    assert!(calls.ends_with(&["stop voxmin-backend-testing-updock-70592fbe 5s".into(), "remove voxmin-backend-testing-updock-70592fbe".into()]));
}

#[tokio::test]
async fn a_healthcheck_is_waited_for() {
    let fake = Fake::new(Some(3), Readiness::Healthcheck);
    swap::swap(&fake, &container(&[], None), "img:new", &plan_for(Readiness::Healthcheck)).await.expect("swapped once healthy");
    assert!(fake.calls().last().unwrap().starts_with("rename"));
}
