# updock

A Rust daemon that combines two complementary container automation tools:

1. **Harbor webhook receiver** — listens for Harbor artifact-push events and runs `docker compose up -d` for matching projects.
2. **Watchtower-style auto-updater** — polls running Docker containers on a schedule, detects updated images, recreates containers, runs lifecycle hooks, sends notifications, and exposes Prometheus metrics.

## Features

### Harbor webhook receiver
- Configurable HTTP server (host, port, webhook path)
- Shared-secret authentication matching Harbor's `Authorization` header
- Project matching by Harbor repository name (`<project>/<image>`)
- Optional tag filtering with `*` / `**` wildcard support
- Optional `docker compose pull` before `up`
- Parallel deployments when multiple projects match

### Watchtower-style watcher
- Polling interval (seconds) **or** 6-field cron schedule
- Container label filtering (`com.centurylinklabs.watchtower.*`)
- Lifecycle hooks: pre-check, pre-update, post-update, post-check via `docker exec`
- Cleanup old images after update
- Remove anonymous volumes on recreation
- Monitor-only mode (detect without updating)
- No-pull mode (local digest comparison only)
- No-restart mode (pull new image but don't recreate)
- Include stopped/exited containers
- Rolling restart (one at a time, with a gap)
- Run-once mode (single scan then exit)
- Container name allow / deny lists
- Scope label filtering for multi-instance deployments
- Configurable stop timeout per container

### HTTP API
- `GET /health` — liveness probe
- `GET /v1/metrics` — Prometheus text exposition
- `POST /v1/update` — manual trigger (Bearer token auth)

### Notifications
- Generic JSON webhook (Slack, Discord, Teams, Mattermost, …)
- Slack incoming webhook (with channel / username overrides)
- Gotify push notifications
- SMTP email

### Operational
- Daemon mode (`fork`, PID file, SIGTERM-based stop)
- Structured JSON logs via `tracing` + `tracing-subscriber`
- Graceful shutdown on SIGINT / SIGTERM
- `check`, `status`, `stop` sub-commands

## Installation

```bash
cargo build --release
sudo cp target/release/updock /usr/local/bin/
```

## Quick start

```bash
cp updock.yaml.example updock.yaml
# Edit updock.yaml to match your setup
updock -c updock.yaml check          # validate config
updock -c updock.yaml start          # foreground
updock -c updock.yaml start --daemon # background
```

## Configuration (`updock.yaml`)

See [`updock.yaml.example`](updock.yaml.example) for a fully-annotated reference.

### `server`

| Key | Default | Description |
|-----|---------|-------------|
| `bind` | `0.0.0.0:8080` | TCP address to listen on |
| `webhook_path` | `/webhook` | URL path Harbor posts to |
| `secret` | _(none)_ | Shared secret validated against the `Authorization` / `X-Harbor-Token` header |
| `http_api_metrics` | `false` | Enable `GET /v1/metrics` |
| `http_api_update` | `false` | Enable `POST /v1/update` |
| `http_api_token` | _(none)_ | Bearer token protecting `/v1/metrics` and `/v1/update` |

### `daemon`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Fork into background on `start` |
| `pid_file` | `/var/run/updock.pid` | PID file location |
| `log_dir` | `/var/log/updock` | Log directory (daemon mode) |

### `projects[]`

| Key | Required | Description |
|-----|----------|-------------|
| `name` | yes | Human-readable label used in log output |
| `harbor_repository` | yes | Harbor repository in `<project>/<image>` form |
| `compose_dir` | yes | Absolute path to the directory containing `docker-compose.yml` |
| `tag_filter` | no | Only deploy if pushed tag matches; supports `*` (no `/`) and `**` (any) wildcards |
| `compose_file` | no | Override the compose filename (default: auto-detect) |
| `env` | no | Extra environment variables injected into the `docker compose` call |
| `pull_before_up` | `true` | Run `docker compose pull` before `up -d` |

### `watcher`

| Key | Default | Description |
|-----|---------|-------------|
| `enabled` | `false` | Enable the polling/update loop |
| `docker_host` | `unix:///var/run/docker.sock` | Docker socket or TCP endpoint |
| `poll_interval` | `86400` | Poll interval in seconds; ignored when `schedule` is set |
| `schedule` | _(none)_ | 6-field cron expression overriding `poll_interval` |
| `cleanup` | `false` | Remove old images after updating a container |
| `remove_volumes` | `false` | Remove anonymous volumes on container recreation |
| `monitor_only` | `false` | Detect updates only; do not apply them |
| `no_pull` | `false` | Compare local digests; never pull from registry |
| `no_restart` | `false` | Pull new image but do not recreate the container |
| `include_stopped` | `false` | Also watch stopped / exited containers |
| `rolling_restart` | `false` | Restart one container at a time (2-second gap) |
| `label_enable` | `false` | Only watch containers with `…watchtower.enable=true` |
| `disable_containers` | `[]` | Container names to explicitly exclude |
| `watch_containers` | `[]` | Restrict watching to these names (empty = watch all) |
| `scope` | _(none)_ | Only watch containers whose `…watchtower.scope` label matches |
| `stop_timeout` | `10` | Seconds before force-stopping a container |
| `lifecycle_hooks` | `false` | Execute pre/post-check and pre/post-update hook labels |
| `run_once` | `false` | Run one scan then exit |

#### Container labels (Watchtower-compatible)

| Label | Description |
|-------|-------------|
| `com.centurylinklabs.watchtower.enable` | `true` / `false` — include or exclude this container |
| `com.centurylinklabs.watchtower.monitor-only` | `true` — never update this container |
| `com.centurylinklabs.watchtower.no-pull` | `true` — skip registry pull for this container |
| `com.centurylinklabs.watchtower.stop-signal` | Signal name to send when stopping (e.g. `SIGQUIT`) |
| `com.centurylinklabs.watchtower.scope` | Scope value for multi-instance filtering |
| `com.centurylinklabs.watchtower.lifecycle.pre-check` | Shell command run before the scan |
| `com.centurylinklabs.watchtower.lifecycle.pre-update` | Shell command run before stopping the container |
| `com.centurylinklabs.watchtower.lifecycle.post-update` | Shell command run after the new container starts |
| `com.centurylinklabs.watchtower.lifecycle.post-check` | Shell command run after the scan completes |

### `notifications`

| Key | Description |
|-----|-------------|
| `urls` | List of generic webhook URLs; POST `{"text": "..."}` |
| `slack_webhook_url` | Slack incoming-webhook URL |
| `slack_channel` | Slack channel override, e.g. `#deployments` |
| `slack_username` | Slack username override (default: `updock`) |
| `gotify_url` | Gotify server base URL |
| `gotify_token` | Gotify application token |
| `email.smtp_host` | SMTP server hostname |
| `email.smtp_port` | SMTP port (default: `587`) |
| `email.username` / `email.password` | SMTP credentials |
| `email.from` | Sender address |
| `email.to` | List of recipient addresses |

## Harbor webhook setup

1. Go to **Harbor → your project → Webhooks → New Webhook**.
2. Set **Endpoint URL** to `http://<your-host>:8080/webhook`.
3. Set **Auth Header** to the value of `server.secret` in your config.
4. Select event type **Artifact pushed**.
5. Save.

## CLI reference

```
updock [OPTIONS] <COMMAND>

Options:
  -c, --config <FILE>   Config file path [default: updock.yaml]
                        Also reads UPDOCK_CONFIG env var

Commands:
  start   Start the webhook server (and watcher loop if enabled)
  stop    Send SIGTERM to a running daemon via the PID file
  check   Parse and validate the config, then print a summary
  status  Show whether the daemon is running
```

### Environment variables

| Variable | Description |
|----------|-------------|
| `UPDOCK_CONFIG` | Override the default config file path |
| `RUST_LOG` | Tracing filter, e.g. `RUST_LOG=debug` |

## Prometheus metrics

When `server.http_api_metrics: true`, `GET /v1/metrics` exposes:

| Metric | Type | Description |
|--------|------|-------------|
| `updock_containers_scanned` | gauge | Containers scanned in the last cycle |
| `updock_containers_updated` | gauge | Containers updated in the last cycle |
| `updock_containers_failed` | gauge | Containers where update failed in the last cycle |
| `updock_scans_total` | counter | Total scan cycles since start |
| `updock_scans_skipped` | counter | Scan cycles skipped (no containers to watch) |

## Log output

All logs are emitted as newline-delimited JSON to stdout:

```json
{"timestamp":"2024-01-01T00:00:00Z","level":"INFO","fields":{"message":"updock starting","version":"0.1.0","bind":"0.0.0.0:8080","watcher":true},"target":"updock"}
{"timestamp":"2024-01-01T04:00:00Z","level":"INFO","fields":{"message":"container updated successfully","container":"/my-app"},"target":"updock::watcher"}
{"timestamp":"2024-01-01T04:00:01Z","level":"INFO","fields":{"message":"received Harbor webhook","event_type":"PUSH_ARTIFACT","repository":"myproject/myapi"},"target":"updock::server"}
```

## Systemd unit

```ini
[Unit]
Description=updock Harbor webhook daemon + container auto-updater
After=network.target docker.service

[Service]
Type=simple
ExecStart=/usr/local/bin/updock -c /etc/updock/updock.yaml start
ExecStop=/usr/local/bin/updock -c /etc/updock/updock.yaml stop
Restart=on-failure
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=multi-user.target
```

## License

MIT
