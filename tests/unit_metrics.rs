/// Unit tests for Prometheus metrics: counters, gauges, and text rendering.
use updock::metrics::Metrics;

// ── construction ──────────────────────────────────────────────────────────────

#[test]
fn metrics_new_creates_instance() {
    let _m = Metrics::new();
}

// ── gauges ────────────────────────────────────────────────────────────────────

#[test]
fn set_scanned_reflects_value() {
    let m = Metrics::new();
    m.set_scanned(5);
    // Verify via the rendered text output
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_scanned 5"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn set_updated_reflects_value() {
    let m = Metrics::new();
    m.set_updated(3);
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_updated 3"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn set_failed_reflects_value() {
    let m = Metrics::new();
    m.set_failed(2);
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_failed 2"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn set_scanned_overwrites_previous_value() {
    let m = Metrics::new();
    m.set_scanned(10);
    m.set_scanned(7);
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_scanned 7"),
        "rendered:\n{rendered}"
    );
    assert!(!rendered.contains("updock_containers_scanned 10"));
}

// ── counters ──────────────────────────────────────────────────────────────────

#[test]
fn inc_scans_total_increments() {
    let m = Metrics::new();
    m.inc_scans_total();
    m.inc_scans_total();
    m.inc_scans_total();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_scans_total 3"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_scans_skipped_increments() {
    let m = Metrics::new();
    m.inc_scans_skipped();
    m.inc_scans_skipped();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_scans_skipped 2"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_containers_updated_increments() {
    let m = Metrics::new();
    m.inc_containers_updated();
    m.inc_containers_updated();
    m.inc_containers_updated();
    m.inc_containers_updated();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_updated 4"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_containers_failed_increments() {
    let m = Metrics::new();
    m.inc_containers_failed();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_failed 1"),
        "rendered:\n{rendered}"
    );
}

// ── webhook counters ──────────────────────────────────────────────────────────

#[test]
fn inc_webhook_deployments_increments() {
    let m = Metrics::new();
    m.inc_webhook_deployments();
    m.inc_webhook_deployments();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_deployments_total 2"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_webhook_failures_increments() {
    let m = Metrics::new();
    m.inc_webhook_failures();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_failures_total 1"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_webhook_ignored_increments() {
    let m = Metrics::new();
    m.inc_webhook_ignored();
    m.inc_webhook_ignored();
    m.inc_webhook_ignored();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_ignored_total 3"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn inc_webhook_skipped_increments() {
    let m = Metrics::new();
    m.inc_webhook_skipped();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_skipped_total 1"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn webhook_counters_start_at_zero() {
    let m = Metrics::new();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_deployments_total 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_failures_total 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_ignored_total 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_skipped_total 0"),
        "rendered:\n{rendered}"
    );
}

// ── defaults start at zero ────────────────────────────────────────────────────

#[test]
fn all_metrics_start_at_zero() {
    let m = Metrics::new();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_scanned 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_updated 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_failed 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_scans_total 0"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_scans_skipped 0"),
        "rendered:\n{rendered}"
    );
}

// ── render ────────────────────────────────────────────────────────────────────

#[test]
fn render_produces_prometheus_text_format() {
    let m = Metrics::new();
    let rendered = m.render();
    // Prometheus text format: lines with HELP and TYPE directives
    assert!(rendered.contains("# HELP"), "rendered:\n{rendered}");
    assert!(rendered.contains("# TYPE"), "rendered:\n{rendered}");
}

#[test]
fn render_contains_all_metric_names() {
    let m = Metrics::new();
    let rendered = m.render();
    assert!(
        rendered.contains("updock_containers_scanned"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_updated"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_failed"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_scans_total"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_scans_skipped"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_deployments_total"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_failures_total"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_ignored_total"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_skipped_total"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn render_is_valid_utf8() {
    let m = Metrics::new();
    let rendered = m.render();
    assert!(!rendered.is_empty());
    // All chars are valid UTF-8 by construction (String type guarantees it)
}

// ── independent instances do not share state ─────────────────────────────────

#[test]
fn two_metrics_instances_are_independent() {
    let m1 = Metrics::new();
    let m2 = Metrics::new();
    m1.inc_scans_total();
    m1.inc_scans_total();
    // m2 should still be at 0
    let r2 = m2.render();
    assert!(
        r2.contains("updock_scans_total 0"),
        "m2 should be independent:\n{r2}"
    );
    let r1 = m1.render();
    assert!(
        r1.contains("updock_scans_total 2"),
        "m1 should have 2:\n{r1}"
    );
}

// ── combined operations ───────────────────────────────────────────────────────

#[test]
fn full_cycle_updates_all_fields() {
    let m = Metrics::new();
    // Simulate a scan cycle
    m.inc_scans_total();
    m.set_scanned(10);
    m.inc_containers_updated();
    m.inc_containers_updated();
    m.inc_containers_failed();
    m.set_updated(2);
    m.set_failed(1);

    let rendered = m.render();
    assert!(
        rendered.contains("updock_scans_total 1"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_scanned 10"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_updated 2"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_containers_failed 1"),
        "rendered:\n{rendered}"
    );
}

#[test]
fn webhook_full_cycle() {
    let m = Metrics::new();
    // Simulate a webhook event cycle: 2 ignored, 1 deployed, 1 failed, 1 skipped
    m.inc_webhook_ignored();
    m.inc_webhook_ignored();
    m.inc_webhook_deployments();
    m.inc_webhook_failures();
    m.inc_webhook_skipped();

    let rendered = m.render();
    assert!(
        rendered.contains("updock_webhook_ignored_total 2"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_deployments_total 1"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_failures_total 1"),
        "rendered:\n{rendered}"
    );
    assert!(
        rendered.contains("updock_webhook_skipped_total 1"),
        "rendered:\n{rendered}"
    );
}
