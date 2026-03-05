/// Prometheus metrics for the watcher loop.
use prometheus::{IntCounter, IntGauge, Registry};
use std::sync::Arc;

pub struct Metrics {
    pub containers_scanned: IntGauge,
    pub containers_updated: IntGauge,
    pub containers_failed: IntGauge,
    pub scans_total: IntCounter,
    pub scans_skipped: IntCounter,
    registry: Registry,
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        let registry = Registry::new();

        let containers_scanned = IntGauge::new(
            "updock_containers_scanned",
            "Number of containers scanned for changes during the last scan",
        )
        .unwrap();
        let containers_updated = IntGauge::new(
            "updock_containers_updated",
            "Number of containers updated during the last scan",
        )
        .unwrap();
        let containers_failed = IntGauge::new(
            "updock_containers_failed",
            "Number of containers where update failed during the last scan",
        )
        .unwrap();
        let scans_total =
            IntCounter::new("updock_scans_total", "Number of scans since updock started").unwrap();
        let scans_skipped = IntCounter::new(
            "updock_scans_skipped",
            "Number of skipped scans since updock started",
        )
        .unwrap();

        registry
            .register(Box::new(containers_scanned.clone()))
            .unwrap();
        registry
            .register(Box::new(containers_updated.clone()))
            .unwrap();
        registry
            .register(Box::new(containers_failed.clone()))
            .unwrap();
        registry.register(Box::new(scans_total.clone())).unwrap();
        registry.register(Box::new(scans_skipped.clone())).unwrap();

        Arc::new(Self {
            containers_scanned,
            containers_updated,
            containers_failed,
            scans_total,
            scans_skipped,
            registry,
        })
    }

    pub fn set_scanned(&self, n: usize) {
        self.containers_scanned.set(n as i64);
    }
    pub fn set_updated(&self, n: usize) {
        self.containers_updated.set(n as i64);
    }
    pub fn set_failed(&self, n: usize) {
        self.containers_failed.set(n as i64);
    }
    pub fn inc_scans_total(&self) {
        self.scans_total.inc();
    }
    pub fn inc_scans_skipped(&self) {
        self.scans_skipped.inc();
    }
    pub fn inc_containers_updated(&self) {
        self.containers_updated.inc();
    }
    pub fn inc_containers_failed(&self) {
        self.containers_failed.inc();
    }

    /// Render all metrics as a Prometheus text exposition.
    pub fn render(&self) -> String {
        use prometheus::Encoder;
        let mut buf = Vec::new();
        let encoder = prometheus::TextEncoder::new();
        encoder
            .encode(&self.registry.gather(), &mut buf)
            .unwrap_or(());
        String::from_utf8_lossy(&buf).into_owned()
    }
}
