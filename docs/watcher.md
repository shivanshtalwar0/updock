# Container watcher

The watcher is updock's Watchtower-compatible automatic update engine. It polls your Docker daemon on a schedule, detects containers whose images have newer versions, and seamlessly recreates them.

---

## How it works

```
Every N seconds (or on a cron schedule):

1. List running containers on the Docker daemon
2. For each container:
   a. Pull the latest image from the registry
   b. Compare the new image digest to the one the container is running
3. For containers with a newer image:
   a. Stop the old container
   b. Remove it
   c. Recreate it with identical configuration (env, mounts, ports, networks, labels)
   d. Start the new container
4. Send a notification report
```

The container's original configuration is captured before it is stopped, so the recreated container is indistinguishable from one you would start manually.

---

## Enabling the watcher

```yaml
watcher:
  enabled: true
  poll_interval: 3600   # check every hour
```

That is the minimum configuration. updock will start checking all running containers every hour and update any that have a newer image.

---

## Scheduling

### Fixed interval

```yaml
watcher:
  enabled: true
  poll_interval: 3600   # every hour (default is 86400 = 24 h)
```

### Cron schedule

For more control over when scans happen, use a 6-field cron expression instead:

```yaml
watcher:
  enabled: true
  schedule: "0 0 3 * * *"   # 3:00 AM every day
```

The 6 fields are: `seconds minutes hours day-of-month month day-of-week`

| Expression | Meaning |
|---|---|
| `0 0 3 * * *` | 3:00 AM every day |
| `0 */30 * * * *` | Every 30 minutes |
| `0 0 2 * * 1` | 2:00 AM every Monday |
| `0 0 4 1 * *` | 4:00 AM on the 1st of every month |

When `schedule` is set, `poll_interval` is ignored.

The cron expression is validated when updock starts. An invalid expression (like `"not-a-cron"`) prevents startup.

---

## Choosing which containers to watch

By default, the watcher watches **all running containers**. You can restrict or expand this.

### Watch only specific containers

```yaml
watcher:
  enabled: true
  watch_containers:
    - "my-api"
    - "my-worker"
```

Only `my-api` and `my-worker` will ever be updated. All other containers are ignored. Names are matched case-insensitively and the leading `/` Docker prepends to names is stripped before comparison.

### Exclude specific containers

```yaml
watcher:
  enabled: true
  disable_containers:
    - "my-database"
    - "my-reverse-proxy"
```

### Exclude Docker Compose-managed containers (default)

When you run both the watcher and the webhook path simultaneously, both would
try to manage the same Compose-stack containers and fight over them. By default,
updock prevents this automatically:

```yaml
watcher:
  enabled: true
  ignore_compose_managed: true   # default — already set, no need to specify
```

Any container with the `com.docker.compose.project` label — which Docker
Compose sets on every container it manages — is silently skipped by the
watcher. It will be managed via the webhook path instead.

To disable this protection (e.g. you are only using the watcher, with no
webhook deployments):

```yaml
watcher:
  enabled: true
  ignore_compose_managed: false
```

> **Note:** Docker Compose always sets `com.docker.compose.project` on every
> container it creates. You do not need to set any label manually.

### Opt-in via Docker label

When `label_enable: true`, updock only watches containers that have explicitly opted in:

```yaml
watcher:
  enabled: true
  label_enable: true
```

Then on your containers:

```yaml
# docker-compose.yml
services:
  api:
    image: registry.example.com/myorg/api:latest
    labels:
      com.centurylinklabs.watchtower.enable: "true"

  database:
    image: postgres:16
    # no label → watcher ignores this container
```

Any container without `com.centurylinklabs.watchtower.enable=true` is ignored.

> **Opt-out without `label_enable`:** Even without `label_enable: true`, you can exclude a single container by setting `com.centurylinklabs.watchtower.enable=false` on it.

### Watch by scope

Scopes let you run multiple updock instances (or watcher instances) on the same Docker daemon without them stepping on each other:

```yaml
# updock instance for production
watcher:
  enabled: true
  scope: "production"
```

```yaml
# docker-compose.yml — production stack
services:
  api:
    labels:
      com.centurylinklabs.watchtower.scope: "production"
```

Only containers whose `com.centurylinklabs.watchtower.scope` label exactly matches the configured scope are watched.

---

## Connecting to Docker

```yaml
watcher:
  enabled: true
  docker_host: "unix:///var/run/docker.sock"   # default
```

| Value | Connects via |
|---|---|
| `unix:///var/run/docker.sock` | Unix socket (default, local daemon) |
| `tcp://192.168.1.10:2375` | Unencrypted TCP (remote daemon) |
| `http://192.168.1.10:2375` | Same as `tcp://` |

---

## Private registries

By default, the watcher pulls images without any authentication. If your
containers run images from a private registry — including your own Harbor
instance — you need to supply credentials.

Credentials are configured under `watcher.registries`, keyed by the registry
**hostname** (and optional port):

```yaml
watcher:
  enabled: true
  registries:
    # Harbor — username + password (or robot account)
    registry.example.com:
      username: myuser
      password: mypassword

    # Harbor or any registry with a port
    registry.example.com:5000:
      username: myuser
      password: mypassword

    # GitHub Container Registry — personal access token
    ghcr.io:
      token: ghp_xxxxxxxxxxxxxxxxxxxx

    # Docker Hub — required when you hit anonymous pull limits
    index.docker.io:
      username: dockerhubuser
      password: dockerhubpassword
```

### How the registry is determined

updock parses the registry hostname from the container's image reference using
the same rules Docker uses:

| Image reference | Registry matched |
|-----------------|-----------------|
| `nginx` | `index.docker.io` |
| `nginx:latest` | `index.docker.io` |
| `myuser/myapp:v1` | `index.docker.io` |
| `registry.example.com/myapp:v1` | `registry.example.com` |
| `registry.example.com:5000/myapp` | `registry.example.com:5000` |
| `ghcr.io/org/repo:tag` | `ghcr.io` |
| `localhost:5000/myapp` | `localhost:5000` |
| `123456.dkr.ecr.us-east-1.amazonaws.com/app` | `123456.dkr.ecr.us-east-1.amazonaws.com` |

A first path component is treated as a registry host if it contains `.` or `:`,
or is exactly `localhost`. Everything else is treated as a Docker Hub short-form
name.

### Authentication methods

| Config fields | Used for |
|---------------|----------|
| `username` + `password` | HTTP Basic / token auth — Harbor robot accounts, Docker Hub, GHCR personal access tokens passed as passwords |
| `token` | Bearer / identity token — some registries return this from an OAuth exchange |

If both are set, `username`/`password` takes precedence. Matching is
case-insensitive (`Registry.Example.Com` matches `registry.example.com`).

### Harbor robot accounts (recommended)

For Harbor, create a **robot account** with pull permissions on the relevant
project, then use the robot's username and secret:

```yaml
watcher:
  registries:
    harbor.example.com:
      username: "robot$my-updock-robot"
      password: "the-robot-secret"
```

Robot accounts are preferable to personal accounts because:
- They can be scoped to specific projects
- They have their own lifecycle (expiry, revocation)
- They do not block the user's login if credentials are leaked

### With this feature, do you still need the Harbor webhook?

The watcher polling and the Harbor webhook serve different purposes:

| | Webhook mode | Watcher mode |
|---|---|---|
| **Trigger** | Immediate, on push | Scheduled (every N seconds or cron) |
| **Target** | `docker compose` stacks | Any Docker container |
| **Auth** | `docker compose pull` uses `~/.docker/config.json` | Built-in per-registry credentials |
| **Best for** | Compose-based deployments that should update instantly | Long-running containers you want updated on a schedule |

If you want **both** instant updates for your Compose stacks **and** scheduled
updates for standalone containers, run both modes simultaneously. The watcher
automatically skips Compose-managed containers by default
(`ignore_compose_managed: true`), so the two paths will not fight over the same
containers.

---

## Update behaviour options

### Monitor only (dry run)

Check for updates but never apply them. Useful for alerting without automated changes:

```yaml
watcher:
  enabled: true
  monitor_only: true
```

Per-container override (overrides the global setting):

```yaml
labels:
  com.centurylinklabs.watchtower.monitor-only: "true"
```

### Skip pulling from the registry

Compare local image IDs only. This is faster but will not detect new images that have not already been pulled:

```yaml
watcher:
  enabled: true
  no_pull: true
```

Per-container override:

```yaml
labels:
  com.centurylinklabs.watchtower.no-pull: "true"
```

### Clean up old images

Remove the previous image from disk after a container is updated:

```yaml
watcher:
  enabled: true
  cleanup: true
```

### Rolling restarts

Restart one container at a time, with a 2-second pause between each, instead of updating all matched containers simultaneously:

```yaml
watcher:
  enabled: true
  rolling_restart: true
```

### Graceful stop timeout

How long updock waits for a container to stop gracefully before force-killing it:

```yaml
watcher:
  enabled: true
  stop_timeout: 30   # seconds (default: 10)
```

Per-container override via the Docker stop timeout built into the container spec (not a label).

### Custom stop signal

Send a different signal to stop a container instead of `SIGTERM`:

```yaml
# docker-compose.yml
services:
  api:
    labels:
      com.centurylinklabs.watchtower.stop-signal: "SIGHUP"
```

---

## Stopped containers

By default the watcher only considers running containers. To also watch containers that are stopped (exited, paused, etc.):

```yaml
watcher:
  enabled: true
  include_stopped: true
```

To automatically **start** a stopped container after its image is updated:

```yaml
watcher:
  enabled: true
  include_stopped: true
  revive_stopped: true
```

Without `revive_stopped`, updated-but-stopped containers are recreated in the stopped state (image updated but not started).

---

## Lifecycle hooks

Lifecycle hooks let you run custom scripts inside containers at specific points during the update process. They are implemented as Docker labels whose values are shell commands.

Enable hooks globally:

```yaml
watcher:
  enabled: true
  lifecycle_hooks: true
```

Then set labels on your containers:

```yaml
# docker-compose.yml
services:
  api:
    image: registry.example.com/myorg/api:latest
    labels:
      com.centurylinklabs.watchtower.lifecycle.pre-check: "echo 'about to check for update'"
      com.centurylinklabs.watchtower.lifecycle.pre-update: "/app/scripts/drain.sh"
      com.centurylinklabs.watchtower.lifecycle.post-update: "/app/scripts/smoke-test.sh"
      com.centurylinklabs.watchtower.lifecycle.post-check: "echo 'check complete'"
```

| Label | When it runs |
|---|---|
| `pre-check` | Before checking any container for updates (beginning of scan) |
| `post-check` | After checking all containers (end of scan, before updates) |
| `pre-update` | Immediately before stopping this container |
| `post-update` | After the updated container is started |

Each hook is executed as `docker exec <container> sh -c "<label value>"`. The hook runs inside the container, so it has access to all the same tools and filesystem.

All four hooks share the label prefix `com.centurylinklabs.watchtower.lifecycle.`.

---

## Dependency ordering

If container B depends on container A (e.g. a worker that connects to a queue), you can tell updock to update A first:

```yaml
# docker-compose.yml
services:
  queue:
    image: registry.example.com/myorg/queue:latest

  worker:
    image: registry.example.com/myorg/worker:latest
    labels:
      com.centurylinklabs.watchtower.depends-on: "queue"
```

When both containers have updates, updock updates `queue` first, then `worker`. Multiple dependencies can be listed as comma-separated values: `"queue,cache,db"`. Circular dependencies are handled gracefully (updock detects cycles and breaks them).

---

## Run once

Run a single scan and exit — useful for cron-based invocation from CI/CD or a systemd timer:

```yaml
watcher:
  enabled: true
  run_once: true
```

---

## Full Docker label reference

All labels follow the Watchtower naming convention and are fully compatible with Watchtower.

| Label | Values | Description |
|---|---|---|
| `com.centurylinklabs.watchtower.enable` | `"true"` / `"false"` | Opt in or out of watching (see `label_enable` config) |
| `com.centurylinklabs.watchtower.monitor-only` | `"true"` | Watch but never update this container |
| `com.centurylinklabs.watchtower.no-pull` | `"true"` | Skip registry pull for this container |
| `com.centurylinklabs.watchtower.stop-signal` | e.g. `"SIGHUP"` | Signal to send instead of `SIGTERM` when stopping |
| `com.centurylinklabs.watchtower.scope` | any string | Scope identifier; only watched when the watcher's `scope` matches |
| `com.centurylinklabs.watchtower.depends-on` | comma-separated names | Update these containers before this one |
| `com.centurylinklabs.watchtower.lifecycle.pre-check` | shell command | Run before checking for updates (scan start) |
| `com.centurylinklabs.watchtower.lifecycle.pre-update` | shell command | Run before stopping this container |
| `com.centurylinklabs.watchtower.lifecycle.post-update` | shell command | Run after the updated container starts |
| `com.centurylinklabs.watchtower.lifecycle.post-check` | shell command | Run after all checks complete (scan end) |

---

## What the scan report looks like

After each scan, updock sends a notification (if configured) with a summary:

```
[updock] 12 scanned, 2 updated, 0 failed
  ✓ updated: my-api
  ✓ updated: my-worker
  - skipped: my-database (monitor-only)
```

A notification is only sent when at least one container was **updated** or **failed**. Fresh scans (nothing to update) are silent.

See the [Notifications guide](notifications.md) for how to configure where this report is sent.

---

## Example: full watcher configuration

```yaml
watcher:
  enabled: true
  schedule: "0 0 3 * * *"     # 3 AM every day
  docker_host: "unix:///var/run/docker.sock"
  cleanup: true                # remove old images
  stop_timeout: 30             # wait up to 30 s for graceful stop
  rolling_restart: true        # one at a time
  label_enable: true           # only watch opted-in containers
  scope: "production"          # only watch production-scoped containers
  lifecycle_hooks: true        # run pre/post scripts
  ignore_compose_managed: true # don't touch Compose stacks (default)
  disable_containers:
    - "my-database"            # never touch this
```
