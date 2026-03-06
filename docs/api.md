# HTTP API Reference

updock exposes a small HTTP API on the same port as the webhook receiver.
Some endpoints are always available; others must be explicitly enabled in the
configuration file.

---

## Table of contents

- [Summary table](#summary-table)
- [Authentication](#authentication)
- [GET /health](#get-health)
- [POST \<webhook\_path\>](#post-webhook_path)
- [GET /v1/metrics](#get-v1metrics)
- [POST /v1/update](#post-v1update)
- [Error responses](#error-responses)

---

## Summary table

| Method | Path | Auth | Enabled by |
|--------|------|------|-----------|
| `GET` | `/health` | None | Always |
| `POST` | `<webhook_path>` (default `/webhook`) | Optional shared secret | Always |
| `GET` | `/v1/metrics` | Optional Bearer token | `http_api_metrics: true` |
| `POST` | `/v1/update` | Optional Bearer token | `http_api_update: true` |

---

## Authentication

updock uses two different authentication mechanisms depending on the endpoint.

### Webhook secret (Harbor)

The webhook endpoint accepts the secret in any of the following forms:

```
Authorization: Bearer <secret>
Authorization: <secret>
X-Harbor-Token: <secret>
```

All comparisons are **constant-time** to prevent timing attacks.

### Bearer token (API endpoints)

`/v1/metrics` and `/v1/update` use a standard Bearer token:

```
Authorization: Bearer <token>
```

The token is set via `server.http_api_token` in the config. If no token is
configured, the endpoints are accessible without authentication. Raw tokens and
`X-Harbor-Token` are **not** accepted here — only `Authorization: Bearer`.

---

## GET /health

A simple liveness check. Always returns `200 OK` regardless of configuration.
No authentication required.

**Request:**

```bash
curl http://localhost:8080/health
```

**Response:**

```
HTTP/1.1 200 OK

updock ok
```

Use this endpoint to verify that the process is running, in health checks, or
in load balancer probes.

---

## POST \<webhook_path\>

Receives a Harbor push event and triggers `docker compose up -d` for any
projects whose `harbor_repository` and `tag_filter` match the pushed image.

The default path is `/webhook`. Override it with `server.webhook_path` in the
config.

**Request:**

```bash
curl -X POST http://localhost:8080/webhook \
  -H "Authorization: Bearer mysecret" \
  -H "Content-Type: application/json" \
  -d @harbor_event.json
```

**Success responses:**

| Status | Body | Meaning |
|--------|------|---------|
| `200` | `deployment triggered\n` | At least one project matched; deployment(s) started |
| `200` | `ignored\n` | Event was not a `PUSH_ARTIFACT` event |
| `200` | `no match\n` | Push event was valid but no project matched the repository/tag |

**Error responses:**

| Status | Body | Meaning |
|--------|------|---------|
| `400` | `Bad Request: invalid JSON\n` | Request body is not valid JSON or not a Harbor event |
| `401` | `Unauthorized\n` | Secret configured but not provided / incorrect |

### Only `PUSH_ARTIFACT` events are acted on

Harbor sends events for many actions (pull, delete, scan, etc.). updock ignores
everything except `PUSH_ARTIFACT`. Non-push events return `200 ignored`.

### Deployments run in parallel

If multiple projects match a single push event, all matching `docker compose
up -d` commands are launched concurrently. The HTTP response is returned
immediately — it does not wait for the deployments to finish.

### Expected Harbor payload (reference)

Harbor's webhook payload looks like this:

```json
{
  "type": "PUSH_ARTIFACT",
  "operator": "admin",
  "event_data": {
    "repository": {
      "name": "myapp",
      "namespace": "myproject",
      "full_name": "myproject/myapp",
      "type": "private"
    },
    "resources": [
      {
        "tag": "latest",
        "resource_url": "registry.example.com/myproject/myapp:latest"
      }
    ]
  }
}
```

updock matches against `full_name` (e.g. `myproject/myapp`) and the tag
value(s) in `resources[*].tag`.

---

## GET /v1/metrics

Exposes Prometheus-format metrics for both the watcher loop and webhook deployments.

Must be enabled with `http_api_metrics: true` in the config. If the endpoint
is not enabled, the server returns `404 Not Found`.

**Config:**

```yaml
server:
  http_api_metrics: true
  http_api_token: mysecrettoken   # optional
```

**Request:**

```bash
# Without auth
curl http://localhost:8080/v1/metrics

# With Bearer token
curl -H "Authorization: Bearer mysecrettoken" \
     http://localhost:8080/v1/metrics
```

**Response:**

```
HTTP/1.1 200 OK
Content-Type: text/plain; version=0.0.4

# HELP updock_containers_scanned Number of containers scanned for changes during the last scan
# TYPE updock_containers_scanned gauge
updock_containers_scanned 12
# HELP updock_containers_updated Number of containers updated during the last scan
# TYPE updock_containers_updated gauge
updock_containers_updated 2
# HELP updock_containers_failed Number of containers where update failed during the last scan
# TYPE updock_containers_failed gauge
updock_containers_failed 0
# HELP updock_scans_total Number of scans since updock started
# TYPE updock_scans_total counter
updock_scans_total 7
# HELP updock_scans_skipped Number of skipped scans since updock started
# TYPE updock_scans_skipped counter
updock_scans_skipped 0
# HELP updock_webhook_deployments_total Total webhook-triggered deployments since updock started
# TYPE updock_webhook_deployments_total counter
updock_webhook_deployments_total 14
# HELP updock_webhook_failures_total Total webhook-triggered deployments that failed since updock started
# TYPE updock_webhook_failures_total counter
updock_webhook_failures_total 1
# HELP updock_webhook_ignored_total Total webhook events ignored (non-push or no matching project)
# TYPE updock_webhook_ignored_total counter
updock_webhook_ignored_total 3
# HELP updock_webhook_skipped_total Total webhook deployments skipped (image unchanged after pull)
# TYPE updock_webhook_skipped_total counter
updock_webhook_skipped_total 5
```

The `Content-Type: text/plain; version=0.0.4` header conforms to the
Prometheus text exposition format and is compatible with standard Prometheus
scrapers.

### Available metrics

| Metric | Type | Description |
|--------|------|-------------|
| `updock_containers_scanned` | Gauge | Containers examined in the **most recent** watcher scan |
| `updock_containers_updated` | Gauge | Containers successfully updated in the **most recent** watcher scan |
| `updock_containers_failed` | Gauge | Containers that failed to update in the **most recent** watcher scan |
| `updock_scans_total` | Counter | Total number of watcher scans since updock started |
| `updock_scans_skipped` | Counter | Total number of watcher scans that were skipped (e.g. Docker unreachable) |
| `updock_webhook_deployments_total` | Counter | Total webhook-triggered `docker compose up -d` calls that succeeded |
| `updock_webhook_failures_total` | Counter | Total webhook-triggered deployments that failed |
| `updock_webhook_ignored_total` | Counter | Total webhook events ignored (non-`PUSH_ARTIFACT` events, or no project matched) |
| `updock_webhook_skipped_total` | Counter | Total webhook deployments skipped because the image digest was unchanged after pull |

Gauges reflect the **last watcher scan only** — they are reset and rewritten at the
start of each new scan. Counters are cumulative and never reset.

**Prometheus scrape config example:**

```yaml
scrape_configs:
  - job_name: updock
    static_configs:
      - targets: ["updock-host:8080"]
    metrics_path: /v1/metrics
    authorization:
      credentials: mysecrettoken
```

---

## POST /v1/update

Triggers an immediate watcher scan without waiting for the next scheduled
interval. Useful for CI/CD pipelines or manual refreshes.

Must be enabled with `http_api_update: true` in the config. If the watcher is
not enabled, returns `503 Service Unavailable`.

**Config:**

```yaml
server:
  http_api_update: true
  http_api_token: mysecrettoken   # optional

watcher:
  enabled: true
```

**Request:**

```bash
curl -X POST http://localhost:8080/v1/update \
  -H "Authorization: Bearer mysecrettoken"
```

**Responses:**

| Status | Body | Meaning |
|--------|------|---------|
| `202` | `update triggered\n` | Scan queued; the watcher will run shortly |
| `202` | `update already pending\n` | A scan was already queued; nothing more needed |
| `401` | `Unauthorized\n` | Token configured but missing or incorrect |
| `503` | `watcher not enabled\n` | `watcher.enabled` is `false` in config |
| `503` | `watcher unavailable\n` | Internal channel error (should not occur normally) |

The response is returned immediately — it does not wait for the scan to
complete. Check logs or the `/v1/metrics` endpoint to see the scan result.

**CI/CD example:**

```bash
# After pushing an image, trigger updock to check for updates immediately
curl -s -o /dev/null -w "%{http_code}" \
  -X POST http://updock.internal:8080/v1/update \
  -H "Authorization: Bearer $UPDOCK_TOKEN"
```

---

## Error responses

All API errors return plain text bodies (not JSON).

| Status | Meaning |
|--------|---------|
| `400 Bad Request` | Invalid JSON payload (webhook only) |
| `401 Unauthorized` | Authentication required but token missing or wrong |
| `404 Not Found` | Endpoint not enabled in config |
| `503 Service Unavailable` | Watcher not running (update endpoint only) |
