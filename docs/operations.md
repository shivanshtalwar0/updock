# Operations Guide

This guide covers everything you need to run, manage, and monitor updock in
production: the CLI, daemon mode, logging, Prometheus metrics, and system
integration examples.

---

## Table of contents

- [CLI reference](#cli-reference)
- [Config file location](#config-file-location)
- [Foreground mode](#foreground-mode)
- [Daemon mode](#daemon-mode)
- [Stopping the daemon](#stopping-the-daemon)
- [Checking the daemon status](#checking-the-daemon-status)
- [Validating your configuration](#validating-your-configuration)
- [Logging](#logging)
- [Prometheus metrics](#prometheus-metrics)
- [Systemd service example](#systemd-service-example)
- [Graceful shutdown](#graceful-shutdown)
- [Troubleshooting](#troubleshooting)

---

## CLI reference

```
updock [OPTIONS] <COMMAND>
```

**Global options:**

| Flag | Env var | Default | Description |
|------|---------|---------|-------------|
| `-c`, `--config <PATH>` | `UPDOCK_CONFIG` | `updock.yaml` | Path to the configuration file |

**Commands:**

| Command | Description |
|---------|-------------|
| `start [--daemon]` | Start the webhook server (and watcher loop if enabled) |
| `stop` | Stop a running daemon by sending it `SIGTERM` |
| `check` | Validate the config file and print the parsed settings |
| `status` | Print whether the daemon is running or stopped |

---

## Config file location

By default updock looks for `updock.yaml` in the **current working directory**.
Override the path with `--config` or the `UPDOCK_CONFIG` environment variable:

```bash
# Explicit flag
updock --config /etc/updock/updock.yaml start

# Environment variable
export UPDOCK_CONFIG=/etc/updock/updock.yaml
updock start
```

---

## Foreground mode

Running without `--daemon` (and with `daemon.enabled: false` in the config)
keeps updock attached to the terminal. Logs go to stdout, and the process exits
when you press `Ctrl+C`.

```bash
updock start
```

This is the recommended mode for:
- Initial setup and testing
- Containerised deployments (Docker / Kubernetes manage process lifecycle)
- Seeing log output in real time

---

## Daemon mode

Daemon mode forks updock into the background, detaches it from the terminal,
and writes a PID file so you can stop it later.

Enable daemon mode in one of two ways:

**Option 1 — config file:**

```yaml
daemon:
  enabled: true
  pid_file: /var/run/updock.pid    # default
  log_dir: /var/log/updock         # default
```

**Option 2 — command-line flag (overrides config):**

```bash
updock start --daemon
```

When daemonising, updock:

1. `fork()`s a child process
2. The parent exits immediately (your shell prompt returns)
3. The child calls `setsid()` to create a new session, detaching from the
   controlling terminal
4. Creates the log directory if it does not exist
5. Writes its PID to `daemon.pid_file`

**Default paths:**

| Setting | Default |
|---------|---------|
| `daemon.pid_file` | `/var/run/updock.pid` |
| `daemon.log_dir` | `/var/log/updock` |

> **Tip:** Make sure the process has write permission to both the PID file
> directory and the log directory before starting in daemon mode. Running as
> `root` or adjusting directory ownership is the simplest solution.

---

## Stopping the daemon

```bash
updock stop
```

This reads the PID from `daemon.pid_file` and sends `SIGTERM` to the process.
updock handles `SIGTERM` gracefully (see [Graceful shutdown](#graceful-shutdown)).

If the PID file does not exist, updock prints a warning and exits without
error.

---

## Checking the daemon status

```bash
updock status
```

Reads the PID file and checks whether the process is actually running:

| Output | Meaning |
|--------|---------|
| `running (PID 12345)` | Daemon is alive |
| `stopped (no PID file at /var/run/updock.pid)` | No PID file found |
| `stale PID file (/var/run/updock.pid) – process not found` | PID file exists but the process is gone (crashed or was killed) |

A stale PID file can be safely deleted before restarting.

---

## Validating your configuration

Before starting updock for the first time (or after any config change), run:

```bash
updock check
```

This loads and validates the config file, then prints a human-readable summary:

```
Configuration is valid.

Server bind       : 0.0.0.0:8080
Webhook path      : /webhook
Secret            : <set>
Daemon enabled    : false
PID file          : /var/run/updock.pid

Watcher enabled   : true
Watcher schedule  : 0 0 4 * * *

Projects (2):
  [myapp]   harbor_repository="myproject/myapp"   tag_filter=Some("latest")   compose_dir=/srv/myapp   pull=true
  [worker]  harbor_repository="myproject/worker"  tag_filter=None             compose_dir=/srv/worker  pull=true
```

`updock check` exits with a non-zero status code if the config is invalid,
making it suitable for CI/CD pre-flight checks.

---

## Logging

updock emits **structured JSON logs** via the `tracing` crate. Every log line
is a single JSON object, suitable for ingestion by log aggregators (Loki,
Elasticsearch, Datadog, etc.).

**Example log line:**

```json
{"timestamp":"2026-03-06T10:00:00.123Z","level":"INFO","fields":{"message":"updock starting","version":"0.1.0","bind":"0.0.0.0:8080","path":"/webhook","daemon":false,"watcher":true}}
```

### Controlling log verbosity

Set the `RUST_LOG` environment variable:

| Value | What you see |
|-------|-------------|
| `error` | Only errors |
| `warn` | Errors and warnings |
| `info` | Normal operational messages (default) |
| `debug` | Detailed internal state (useful for troubleshooting) |
| `trace` | Very verbose; includes library internals |

```bash
RUST_LOG=debug updock start
```

You can also scope logging to a specific module:

```bash
# Debug only the webhook server, keep everything else at info
RUST_LOG=info,updock::server=debug updock start
```

If `RUST_LOG` is not set, updock defaults to `info`.

---

## Prometheus metrics

When `http_api_metrics: true` is set, updock exposes a `/v1/metrics` endpoint
in Prometheus text format.

See the [API reference](api.md#get-v1metrics) for the full endpoint
documentation. The metrics available are:

**Watcher metrics:**

| Metric | Type | Description |
|--------|------|-------------|
| `updock_containers_scanned` | Gauge | Containers examined in the last watcher scan |
| `updock_containers_updated` | Gauge | Containers updated in the last watcher scan |
| `updock_containers_failed` | Gauge | Containers that failed to update in the last watcher scan |
| `updock_scans_total` | Counter | Total watcher scans completed since process start |
| `updock_scans_skipped` | Counter | Total watcher scans skipped since process start |

**Webhook metrics:**

| Metric | Type | Description |
|--------|------|-------------|
| `updock_webhook_deployments_total` | Counter | Successful webhook-triggered deployments since process start |
| `updock_webhook_failures_total` | Counter | Failed webhook-triggered deployments since process start |
| `updock_webhook_ignored_total` | Counter | Webhook events ignored (non-push or no matching project) since process start |
| `updock_webhook_skipped_total` | Counter | Webhook deployments skipped (image unchanged after pull) since process start |

Gauges are **reset each watcher scan** (they always reflect the most recent run).
Counters are **cumulative** and never decrease.

**Prometheus scrape configuration:**

```yaml
scrape_configs:
  - job_name: updock
    static_configs:
      - targets: ["updock-host:8080"]
    metrics_path: /v1/metrics
    # Only needed if http_api_token is set:
    authorization:
      credentials: mysecrettoken
```

---

## Systemd service example

For servers that use systemd, here is a minimal unit file to manage updock as
a system service. Note that because systemd manages the process lifecycle,
you should use foreground mode (`daemon.enabled: false`) and let systemd handle
restart behaviour.

Save this as `/etc/systemd/system/updock.service`:

```ini
[Unit]
Description=updock - Harbor webhook daemon and container auto-updater
After=network.target docker.service
Wants=docker.service

[Service]
Type=simple
User=updock
Group=updock
ExecStart=/usr/local/bin/updock --config /etc/updock/updock.yaml start
ExecStop=/usr/local/bin/updock --config /etc/updock/updock.yaml stop
Restart=on-failure
RestartSec=5s

# Structured logging goes to the journal
StandardOutput=journal
StandardError=journal

# Allow the process to bind to port 8080 (not required for ports > 1024)
# AmbientCapabilities=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
```

**Setup steps:**

```bash
# Create a dedicated system user (no login shell, no home directory)
useradd --system --no-create-home --shell /usr/sbin/nologin updock

# Place the config file
mkdir -p /etc/updock
cp updock.yaml /etc/updock/updock.yaml
chown root:updock /etc/updock/updock.yaml
chmod 640 /etc/updock/updock.yaml

# Install the binary
install -o root -g root -m 755 target/release/updock /usr/local/bin/updock

# Enable and start the service
systemctl daemon-reload
systemctl enable updock
systemctl start updock

# Check that it's running
systemctl status updock
journalctl -u updock -f
```

**If updock needs to run docker commands** (for the watcher), the `updock` user
must be in the `docker` group:

```bash
usermod -aG docker updock
```

---

## Graceful shutdown

updock handles `SIGTERM` and `SIGINT` (Ctrl+C) gracefully:

1. The HTTP server stops accepting new connections
2. In-flight requests are allowed to complete
3. The PID file is removed
4. The process exits

This means that a `docker stop`, `systemctl stop updock`, or `updock stop`
command will all result in a clean shutdown without abruptly dropping active
webhook requests.

---

## Troubleshooting

### updock won't start: "cannot bind to 0.0.0.0:8080"

Another process is already using port 8080, or you lack permission to bind
to it.

```bash
# Find what's using the port
lsof -i :8080
# Or change the port in the config
server:
  bind: 0.0.0.0:9000
```

### updock won't start: "Cannot read config file updock.yaml"

The config file is not in the current directory. Either `cd` to the directory
that contains it, use `--config`, or set `UPDOCK_CONFIG`.

### Webhook returns 401

The `Authorization` header sent by Harbor does not match `server.secret`.
Double-check the secret configured in Harbor's webhook settings matches
`server.secret` in `updock.yaml`. Harbor can send the token as
`Authorization: Bearer <secret>`, `Authorization: <secret>`, or
`X-Harbor-Token: <secret>` — all are accepted.

### Daemon mode: "daemonize failed" / permission denied on PID file

The default PID path `/var/run/updock.pid` requires root or the `updock` user
to have write permission to `/var/run`. Override the path to somewhere writable:

```yaml
daemon:
  pid_file: /tmp/updock.pid
  log_dir: /tmp/updock-logs
```

### Watcher never runs

1. Check `watcher.enabled: true` is set.
2. Run `updock check` and confirm the schedule or poll interval looks correct.
3. Check that Docker is accessible:
   ```bash
   curl --unix-socket /var/run/docker.sock http://localhost/version
   ```
4. Look for `"failed to connect to Docker – watcher disabled"` in the logs.

### Stale PID file after a crash

```bash
rm /var/run/updock.pid
updock start
```

### How do I see what updock is doing right now?

Enable debug logging:

```bash
RUST_LOG=debug updock start
```

Or, if running as a daemon, check the journal (systemd):

```bash
journalctl -u updock -f
```
