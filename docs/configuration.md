# Configuration reference

updock is configured through a single YAML file. By default it looks for `updock.yaml` in the current directory. You can specify a different path with:

```bash
updock --config /etc/updock/updock.yaml start
# or via environment variable
UPDOCK_CONFIG=/etc/updock/updock.yaml updock start
```

---

## File structure

```yaml
server:       # required — HTTP server settings
  ...

daemon:       # optional — background process settings
  ...

projects:     # optional — webhook → Compose project mappings
  - ...

watcher:      # optional — automatic container update poller
  ...

notifications: # optional — where to send update reports
  ...
```

Only `server:` is required (it can be empty: `server: {}`). All other sections are optional and default to safe, disabled values.

---

## `server` section

Controls the HTTP server that receives Harbor webhooks and optional API endpoints.

```yaml
server:
  bind: "0.0.0.0:8080"
  webhook_path: "/webhook"
  secret: "your-shared-secret"
  http_api_update: false
  http_api_metrics: false
  http_api_token: "your-api-token"
```

| Field | Type | Default | Description |
|---|---|---|---|
| `bind` | string | `"0.0.0.0:8080"` | TCP address and port to listen on |
| `webhook_path` | string | `"/webhook"` | URL path Harbor sends `POST` requests to |
| `secret` | string | _(none)_ | Shared secret used to authenticate Harbor webhook calls. When absent, **no authentication is performed** on the webhook — only use this on trusted networks |
| `http_api_update` | bool | `false` | Expose `POST /v1/update` — lets CI/CD pipelines trigger an immediate container scan |
| `http_api_metrics` | bool | `false` | Expose `GET /v1/metrics` — Prometheus metrics endpoint |
| `http_api_token` | string | _(none)_ | Bearer token required for `/v1/update` and `/v1/metrics`. When absent, those endpoints are open to anyone who can reach the server |

### Choosing a bind address

- `"0.0.0.0:8080"` — listen on all interfaces (default; Harbor can reach it from outside)
- `"127.0.0.1:8080"` — listen only on localhost (put a reverse proxy in front)
- `"[::]:8080"` — dual-stack IPv4 + IPv6

### Securing the webhook

Set `secret` to the same value you configure in Harbor's webhook "Auth header" field. updock accepts it in any of these forms:

```
Authorization: Bearer your-secret
Authorization: your-secret
X-Harbor-Token: your-secret
```

The comparison is **constant-time** — it does not leak the secret via timing differences.

---

## `daemon` section

Controls whether updock forks into the background and where it writes its PID file and logs.

```yaml
daemon:
  enabled: false
  pid_file: "/var/run/updock.pid"
  log_dir: "/var/log/updock"
```

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Fork into the background on `updock start` |
| `pid_file` | path | `"/var/run/updock.pid"` | Where to write the process ID. The `updock stop` and `updock status` commands read this file |
| `log_dir` | path | `"/var/log/updock"` | Directory for log output when running as a daemon. Created automatically if it does not exist |

> **Note:** The PID file is written even when `enabled: false` (i.e. running in the foreground), so `updock stop` and `updock status` always work.

---

## `projects` section

Maps Harbor repository pushes to local Docker Compose stacks. This is the core of the webhook mode.

```yaml
projects:
  - name: my-api
    harbor_repository: "myproject/my-api"
    compose_dir: "/srv/my-api"

  - name: frontend
    harbor_repository: "myproject/frontend"
    tag_filter: "v*"
    compose_dir: "/srv/frontend"
    compose_file: "docker-compose.prod.yml"
    pull_before_up: true
    env:
      DEPLOY_ENV: production
      REGISTRY: registry.example.com
```

Each item in the list is one project. You can have as many as you need, including multiple projects watching the same repository (e.g. one for production tags, another for staging).

| Field | Type | Default | Required | Description |
|---|---|---|---|---|
| `name` | string | — | yes | A label used in logs and notifications. Can be anything. |
| `harbor_repository` | string | — | yes | The full repository path as shown in Harbor, e.g. `"myproject/myimage"`. Must not be empty. Case-insensitive when matching. |
| `compose_dir` | path | — | yes | Absolute path to the directory containing your `docker-compose.yml`. **Must exist** when updock starts. |
| `tag_filter` | string | _(none)_ | no | Glob pattern. Only trigger a redeploy when the pushed tag matches. See [Tag filters](#tag-filters) below. |
| `compose_file` | string | _(none)_ | no | Override the compose file name, e.g. `"docker-compose.prod.yml"`. When absent, Docker Compose uses its default lookup rules. |
| `pull_before_up` | bool | `true` | no | Run `docker compose pull` before `docker compose up -d`. Disable if you use a private registry that requires separate authentication or if you push images directly to the daemon. |
| `env` | map | `{}` | no | Extra environment variables injected into every `docker compose` command for this project. Useful for passing secrets or per-environment settings. |

### Tag filters

The `tag_filter` field is a glob pattern matched against the image tag that Harbor reports in the push event.

| Pattern | Matches | Does not match |
|---|---|---|
| _(absent)_ | Everything | — |
| `latest` | `latest` only | `v1.0`, `dev` |
| `v*` | `v1.0`, `v2.3.1`, `v10` | `latest`, `dev` |
| `*-rc*` | `v1.0-rc1`, `2.0-rc2` | `v1.0`, `latest` |
| `**` | Everything, including tags with `/` | — |
| `**/stable` | `stable`, `us-east/stable`, `prod/v2/stable` | `unstable` |

Rules:
- `*` matches any characters **except** `/`
- `**` matches any characters **including** `/`
- `**/foo` matches both `foo` and `anything/foo` — the `**/` prefix is optional
- All other characters are matched literally (case-sensitive)

### How deployment works

When a match is found, updock runs these commands in `compose_dir`:

```bash
# Step 1 — only when pull_before_up: true (the default)
docker compose [-f compose_file] pull

# Step 2 — image digest check (automatic)
# Compare image IDs before and after pull; if nothing changed, skip step 3

# Step 3 — only when images actually changed (or pull_before_up: false)
docker compose [-f compose_file] up -d
```

Both commands receive the `env` map as extra environment variables. If `pull` fails, a warning is logged but `up -d` still runs. If `up -d` fails, an error is logged.

After each webhook event, updock sends a notification (if configured) summarising which projects were **deployed**, **skipped** (image unchanged), or **failed**. Silent events (all projects skipped) do not send a notification.

Multiple matching projects are deployed **in parallel**.

---

## `watcher` section

The watcher polls your Docker daemon on a regular schedule and updates any containers whose images have changed.

```yaml
watcher:
  enabled: true
  poll_interval: 3600     # seconds; default 86400 (24 h)
  schedule: "0 0 4 * * *" # cron; overrides poll_interval when set
  cleanup: true
  label_enable: false
```

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | bool | `false` | Enable the watcher. Nothing happens unless this is `true`. |
| `docker_host` | string | `"unix:///var/run/docker.sock"` | Docker socket or TCP address. Supports `unix://`, `tcp://`, `http://`. |
| `poll_interval` | number | `86400` | Seconds between scans. Ignored when `schedule` is also set. |
| `schedule` | string | _(none)_ | 6-field cron expression (`seconds minutes hours day month weekday`). Overrides `poll_interval`. Validated at startup — an invalid expression prevents updock from starting. |
| `cleanup` | bool | `false` | Remove the old image from disk after a container is updated. |
| `remove_volumes` | bool | `false` | Remove anonymous volumes when recreating a container. |
| `monitor_only` | bool | `false` | Detect updates but do **not** apply them. Useful for auditing. Per-container override available via Docker label. |
| `no_pull` | bool | `false` | Skip pulling from the registry — compare local image IDs only. Per-container override available via Docker label. |
| `no_restart` | bool | `false` | Stop and remove the container but do not recreate or start it. |
| `include_stopped` | bool | `false` | Also consider containers in `exited`, `created`, `paused`, or `restarting` states. |
| `revive_stopped` | bool | `false` | Start a stopped container after updating its image (requires `include_stopped: true`). |
| `rolling_restart` | bool | `false` | Restart one container at a time, with a 2-second pause between each. Reduces simultaneous downtime. |
| `label_enable` | bool | `false` | When `true`, only watch containers that have the label `com.centurylinklabs.watchtower.enable=true`. |
| `ignore_compose_managed` | bool | `true` | When `true`, skip any container that has the `com.docker.compose.project` label. This prevents the watcher from fighting with the webhook path over the same Compose-managed containers. Disable only if you deliberately want both paths to manage the same containers. |
| `disable_containers` | list | `[]` | Container names to never update, regardless of other settings. |
| `watch_containers` | list | `[]` | Restrict watching to only these container names. Empty means watch all. |
| `scope` | string | _(none)_ | Only watch containers whose `com.centurylinklabs.watchtower.scope` label matches this value exactly. |
| `stop_timeout` | number | `10` | Seconds to wait for a graceful stop before force-killing a container. |
| `lifecycle_hooks` | bool | `false` | Execute pre/post-check and pre/post-update scripts defined as Docker labels on each container. |
| `run_once` | bool | `false` | Run one scan and then exit. Useful for cron-based invocation from CI. |
| `registries` | map | `{}` | Per-registry credentials for private registries. See below. |

### `watcher.registries`

A map from registry hostname (with optional port) to credentials. updock
automatically matches each container's image reference against the map and
passes the credentials to Docker when pulling.

```yaml
watcher:
  registries:
    # Harbor or any registry using username + password
    registry.example.com:
      username: myuser
      password: mypassword

    # Registry on a non-standard port
    registry.example.com:5000:
      username: myuser
      password: mypassword

    # Bearer / identity token (e.g. from an OAuth exchange)
    ghcr.io:
      token: ghp_xxxxxxxxxxxxxxxxxxxx

    # Docker Hub (to avoid anonymous rate limits)
    index.docker.io:
      username: hubuser
      password: hubpass
```

| Field | Type | Description |
|---|---|---|
| `username` | string | Registry username or robot account name |
| `password` | string | Registry password or robot account secret |
| `token` | string | Bearer / identity token (used when `username`/`password` are absent) |

Images without a registry prefix (e.g. `nginx:latest`) are treated as Docker
Hub (`index.docker.io`). See the [Watcher guide](watcher.md#private-registries)
for full details on host matching rules and Harbor robot account setup.

See the [Watcher guide](watcher.md) for detailed usage, label reference, and examples.

---

## `notifications` section

updock sends a report after each watcher scan that updated or failed at least one container. It also forwards the same message to all configured channels.

```yaml
notifications:
  urls:
    - "https://discord.com/api/webhooks/..."
  slack_webhook_url: "https://hooks.slack.com/services/..."
  slack_channel: "#deployments"
  slack_username: "updock"
  gotify_url: "https://gotify.example.com"
  gotify_token: "your-gotify-app-token"
  email:
    smtp_host: "smtp.example.com"
    smtp_port: 587
    username: "updock@example.com"
    password: "smtp-password"
    from: "updock@example.com"
    to:
      - "ops@example.com"
```

| Field | Type | Default | Description |
|---|---|---|---|
| `urls` | list | `[]` | Generic JSON webhook URLs. Sends `{"text": "..."}` to each. Works with Discord, Mattermost, Teams, etc. |
| `slack_webhook_url` | string | _(none)_ | Slack incoming-webhook URL |
| `slack_channel` | string | _(none)_ | Override the channel in the Slack payload, e.g. `"#ops"` |
| `slack_username` | string | `"updock"` | Display name for the Slack bot |
| `gotify_url` | string | _(none)_ | Gotify server base URL |
| `gotify_token` | string | _(none)_ | Gotify app token. Both URL and token are required for Gotify to work. |
| `email` | object | _(none)_ | SMTP email config (see below) |

### `notifications.email` fields

| Field | Type | Default | Description |
|---|---|---|---|
| `smtp_host` | string | — | SMTP server hostname |
| `smtp_port` | number | `587` | SMTP server port |
| `username` | string | _(none)_ | SMTP username for authentication |
| `password` | string | _(none)_ | SMTP password for authentication |
| `from` | string | — | Sender address |
| `to` | list | — | List of recipient addresses |
| `tls_skip_verify` | bool | `false` | Disable TLS certificate validation. **Only use this in test environments.** |

See the [Notifications guide](notifications.md) for message format and per-channel examples.

---

## Complete example

```yaml
server:
  bind: "0.0.0.0:8080"
  webhook_path: "/webhook"
  secret: "change-me-to-something-random"
  http_api_metrics: true
  http_api_update: true
  http_api_token: "another-random-secret"

daemon:
  enabled: true
  pid_file: "/var/run/updock.pid"
  log_dir: "/var/log/updock"

projects:
  - name: backend-production
    harbor_repository: "myorg/backend"
    tag_filter: "v*"
    compose_dir: "/srv/backend"
    compose_file: "docker-compose.prod.yml"
    pull_before_up: true
    env:
      APP_ENV: production

  - name: backend-staging
    harbor_repository: "myorg/backend"
    tag_filter: "dev"
    compose_dir: "/srv/backend-staging"
    env:
      APP_ENV: staging

  - name: frontend
    harbor_repository: "myorg/frontend"
    compose_dir: "/srv/frontend"

watcher:
  enabled: true
  schedule: "0 0 3 * * *"   # 3 AM every day
  cleanup: true
  stop_timeout: 30
  scope: "production"        # only update containers with this scope label
  ignore_compose_managed: true  # default — don't fight with the webhook path

notifications:
  slack_webhook_url: "https://hooks.slack.com/services/YOUR/SLACK/WEBHOOK"
  slack_channel: "#deployments"
  urls:
    - "https://discord.com/api/webhooks/YOUR/DISCORD/WEBHOOK"
```

---

## Validation errors at startup

updock validates the config when it loads. If anything is wrong, it exits with a clear error message:

| Problem | Error message |
|---|---|
| File does not exist | `Cannot read config file <path>: ...` |
| Invalid YAML syntax | `Config parse error in <path>: ...` |
| `harbor_repository` is empty | `Project '<name>': harbor_repository must not be empty` |
| `compose_dir` does not exist | `Project '<name>': compose_dir '<path>' does not exist` |
| Invalid cron expression | `Invalid watcher.schedule '<expr>': ...` |
