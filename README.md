# updock

**updock** is a self-hosted daemon that connects your [Harbor](https://goharbor.io/) container registry to your Docker Compose stacks. Push an image to Harbor and your server redeploys itself — automatically, securely, and without a Kubernetes cluster.

It also ships a built-in **container watcher** that polls your running containers on a schedule and updates them when their images change, in the same spirit as [Watchtower](https://containrrr.dev/watchtower/).

---

## What it does

```
                 ┌─────────────────────────────────────────────┐
                 │                   updock                     │
                 │                                              │
Harbor  ──push──▶│  POST /webhook   ──match──▶  docker compose │
registry         │                              up -d           │
                 │                                              │
Docker  ◀──poll──│  Watcher loop    ──update──▶  docker stop   │
daemon           │  (cron / interval)             docker start  │
                 │                                              │
Grafana ◀──GET───│  /v1/metrics     (Prometheus)               │
                 │                                              │
CI/CD   ──POST──▶│  /v1/update      (manual scan trigger)      │
                 └─────────────────────────────────────────────┘
```

**Harbor webhook mode** — When Harbor reports a new image push, updock checks whether any of your configured Compose projects use that repository. If there is a match, it runs `docker compose pull` and `docker compose up -d` in the right directory. Multiple projects can react to the same push.

**Container watcher mode** — On a configurable interval (or cron schedule), updock asks the Docker daemon for all running containers, checks whether their images have newer digests in the registry, and recreates any that are stale — preserving all original configuration (environment, mounts, networks, labels).

Both modes are optional and can run together in the same process.

---

## Quick start

### 1. Install

```bash
cargo install --path .
```

Or build a release binary:

```bash
cargo build --release
cp target/release/updock /usr/local/bin/
```

### 2. Write a minimal config

```yaml
# updock.yaml
server:
  bind: "0.0.0.0:8080"
  webhook_path: "/webhook"
  secret: "your-harbor-shared-secret"

projects:
  - name: my-api
    harbor_repository: "myproject/my-api"
    compose_dir: "/srv/my-api"
```

### 3. Start

```bash
updock start
```

### 4. Register the webhook in Harbor

In your Harbor project: **Webhooks → New Webhook**

| Field | Value |
|---|---|
| Endpoint URL | `http://your-server:8080/webhook` |
| Auth header | `your-harbor-shared-secret` |
| Event types | `Push Artifact` |

Push an image and watch the containers redeploy.

---

## Documentation

| Guide | What it covers |
|---|---|
| [Configuration reference](docs/configuration.md) | Every config field, with defaults and examples |
| [Harbor webhook setup](docs/webhook.md) | How to configure Harbor and map repos to Compose projects |
| [Container watcher](docs/watcher.md) | Polling loop, Docker labels, rolling restarts, lifecycle hooks |
| [Notifications](docs/notifications.md) | Slack, Gotify, generic webhooks, email |
| [HTTP API reference](docs/api.md) | All endpoints: health, metrics, update trigger, webhook |
| [Operations guide](docs/operations.md) | CLI commands, daemon mode, Prometheus metrics, troubleshooting |

---

## Example: full config

```yaml
server:
  bind: "0.0.0.0:8080"
  webhook_path: "/webhook"
  secret: "my-harbor-secret"
  http_api_metrics: true
  http_api_update: true
  http_api_token: "my-api-token"

daemon:
  enabled: true
  pid_file: "/var/run/updock.pid"
  log_dir: "/var/log/updock"

projects:
  - name: frontend
    harbor_repository: "myproject/frontend"
    tag_filter: "v*"
    compose_dir: "/srv/frontend"
    pull_before_up: true

  - name: backend
    harbor_repository: "myproject/backend"
    compose_dir: "/srv/backend"
    compose_file: "docker-compose.prod.yml"
    env:
      APP_ENV: production
      DATABASE_URL: "postgres://db:5432/app"

watcher:
  enabled: true
  poll_interval: 3600   # check every hour
  cleanup: true          # remove old images after update

notifications:
  slack_webhook_url: "https://hooks.slack.com/services/..."
  urls:
    - "https://your-discord-webhook-url"
```

---

## Requirements

- Docker with Compose v2 (`docker compose` plugin, not `docker-compose`)
- Rust 1.80+ (for building from source)
- Network access from Harbor to the machine running updock

---

## License

MIT
