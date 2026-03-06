# Harbor webhook setup

This guide walks you through connecting Harbor to updock so that pushing an image automatically redeploys your Docker Compose stack.

---

## How it works

```
You push an image
       │
       ▼
Harbor registry
       │  sends POST /webhook
       ▼
updock (running on your server)
       │  matches repository + tag
       ▼
docker compose pull
       │  image digest check
       │  (skip up -d if nothing changed)
       ▼
docker compose up -d
       │
       ▼
Your stack is running the new image
       │
       ▼
Notification sent (if deployed or failed)
```

1. Harbor sends a JSON payload to the URL you configure as a webhook.
2. updock verifies the shared secret (if configured), then parses the payload.
3. If the event is a `PUSH_ARTIFACT`, updock looks for projects in your config whose `harbor_repository` matches the pushed repository, and whose `tag_filter` (if set) matches the pushed tag.
4. Every matching project runs `docker compose pull` (if `pull_before_up: true`), then checks whether any image ID actually changed.
5. If images changed, `docker compose up -d` is run. If nothing changed, the project is marked **skipped**.
6. After all projects are processed, a notification is sent (if configured) summarising deployed, skipped, and failed projects. Silent events (all skipped) do not send a notification.

---

## Step 1 — Configure updock

Create or edit `updock.yaml`:

```yaml
server:
  bind: "0.0.0.0:8080"
  webhook_path: "/webhook"
  secret: "a-random-shared-secret"   # you choose this value

projects:
  - name: my-app
    harbor_repository: "myproject/my-app"
    compose_dir: "/srv/my-app"
```

Start updock:

```bash
updock start
```

Verify it is listening:

```bash
curl http://localhost:8080/health
# → updock ok
```

---

## Step 2 — Configure Harbor

1. Log in to your Harbor UI.
2. Navigate to your **Project** → **Webhooks**.
3. Click **New Webhook**.

Fill in the form:

| Field | Value |
|---|---|
| **Notify On** | Push Artifact |
| **Endpoint URL** | `http://your-server:8080/webhook` |
| **Auth Header** | `a-random-shared-secret` (the same value as `server.secret`) |
| **Verify Remote Certificate** | On (unless you are using a self-signed cert) |

Click **Test** to send a test event. The test event has type `PING`, not `PUSH_ARTIFACT`, so updock will return `200 ignored` — this is correct.

---

## Step 3 — Test with a real push

```bash
docker build -t registry.example.com/myproject/my-app:latest .
docker push registry.example.com/myproject/my-app:latest
```

In the updock logs you should see:

```json
{"level":"INFO","message":"received Harbor webhook","event_type":"PUSH_ARTIFACT","repository":"myproject/my-app","tags":["latest"]}
{"level":"INFO","message":"deployment triggered","project":"my-app"}
```

---

## Mapping multiple projects

You can have as many `projects` entries as you need. They are all checked on every push:

```yaml
projects:
  # Different repositories → different stacks
  - name: frontend
    harbor_repository: "myorg/frontend"
    compose_dir: "/srv/frontend"

  - name: backend
    harbor_repository: "myorg/backend"
    compose_dir: "/srv/backend"

  # Same repository, different tags → different environments
  - name: backend-production
    harbor_repository: "myorg/backend"
    tag_filter: "v*"
    compose_dir: "/srv/backend-prod"

  - name: backend-staging
    harbor_repository: "myorg/backend"
    tag_filter: "dev"
    compose_dir: "/srv/backend-staging"
```

When `myorg/backend:v1.2` is pushed, only `backend-production` is deployed.
When `myorg/backend:dev` is pushed, only `backend-staging` is deployed.
When `myorg/backend:latest` is pushed (no filter matches), neither is deployed.

All matching projects are deployed **in parallel**.

---

## Tag filters

`tag_filter` is a glob pattern. Leave it out to redeploy on every push to that repository.

| Config | Triggers on | Ignores |
|---|---|---|
| _(absent)_ | every tag | nothing |
| `latest` | `latest` | `v1.0`, `dev` |
| `v*` | `v1.0`, `v2.3.1`, `v10` | `latest`, `dev` |
| `*-rc*` | `v1.0-rc1`, `2.0-rc2` | `v1.0`, `latest` |
| `**` | every tag, including tags with `/` | nothing |
| `**/stable` | `stable`, `us-east/stable` | `unstable` |

Rules:
- `*` matches anything except `/`
- `**` matches anything including `/`
- `**/foo` matches `foo` directly (no prefix needed) and also `any/path/foo`
- Matching is **case-sensitive** (`Latest` ≠ `latest`)

---

## Custom compose file

If your directory contains more than one compose file, or the file is not named `docker-compose.yml`:

```yaml
projects:
  - name: my-app
    harbor_repository: "myorg/my-app"
    compose_dir: "/srv/my-app"
    compose_file: "docker-compose.prod.yml"   # passed as: docker compose -f docker-compose.prod.yml up -d
```

---

## Injecting environment variables

Pass extra environment variables into every `docker compose` command for a project:

```yaml
projects:
  - name: my-app
    harbor_repository: "myorg/my-app"
    compose_dir: "/srv/my-app"
    env:
      REGISTRY: registry.example.com
      APP_ENV: production
      DB_HOST: db.internal
```

These are set as process environment variables for the `docker compose` subprocess. This means your `docker-compose.yml` can reference them with `${REGISTRY}`, `${APP_ENV}`, etc.

---

## Skipping the pull step

By default, updock runs `docker compose pull` before `docker compose up -d`. This ensures the Compose services use the newly pushed image rather than a cached layer.

If you need to skip the pull (for example, your registry requires special auth, or you push images directly to the Docker daemon), set:

```yaml
projects:
  - name: my-app
    harbor_repository: "myorg/my-app"
    compose_dir: "/srv/my-app"
    pull_before_up: false
```

---

## Image digest check (automatic skip)

When `pull_before_up: true` (the default), updock checks whether any image
actually changed after the pull before running `docker compose up -d`.

It does this by recording the set of image IDs that `docker compose images`
reports **before** the pull, pulling, then checking again **after**. If the
set of image IDs is identical, the deployment is skipped and reported as
`skipped` rather than `deployed`.

This prevents unnecessary container restarts when Harbor fires a webhook for a
push that did not change the image content (e.g. a re-tag of an existing
digest).

When `pull_before_up: false`, the digest check is not performed and
`docker compose up -d` always runs.

---

## Deployment notifications

After each webhook event, updock sends a notification (if configured) with a
summary of what happened. The notification is only sent when at least one
project was **deployed** or **failed** — events where everything was skipped
(image unchanged) are silent.

Example notification:

```
[updock] Webhook deployment report
  ✓ deployed: backend-production
  ✓ deployed: frontend
  ↷ skipped:  backend-staging (image unchanged)
```

Example with a failure:

```
[updock] Webhook deployment report
  ✓ deployed: frontend
  ✗ failed:   backend-production
```

Notifications use the same channels configured under `notifications:` in your
config (Slack, Gotify, generic webhooks, email). See the
[Notifications guide](notifications.md) for setup details.

---

## Changing the webhook path

If you put updock behind a reverse proxy or want to keep the webhook URL unpredictable:

```yaml
server:
  webhook_path: "/harbor/hooks/a8f3c9d2"
```

Update your Harbor webhook URL accordingly.

---

## Security considerations

### Always use a secret

Without `server.secret`, anyone who can reach your server can trigger deployments. Set a strong random secret:

```bash
# Generate a secret
openssl rand -hex 32
```

```yaml
server:
  secret: "the-output-of-the-command-above"
```

### Restrict network access

Consider binding updock to a specific interface or placing it behind a firewall or reverse proxy that only allows traffic from your Harbor instance's IP address.

### Reverse proxy with TLS

Harbor's webhook UI has an option to verify the remote certificate. If you terminate TLS with nginx or Caddy:

```nginx
server {
    listen 443 ssl;
    server_name updock.example.com;

    location /webhook {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
    }
}
```

Bind updock to localhost only:

```yaml
server:
  bind: "127.0.0.1:8080"
```

---

## Troubleshooting

### Harbor test sends a PING, not a PUSH

This is expected. The Harbor "Test" button sends a `PING` event, not `PUSH_ARTIFACT`. updock responds `200 ignored`. A real push will trigger the deployment.

### updock responds `401 Unauthorized`

The secret in your Harbor webhook configuration does not match `server.secret` in updock. They must be identical. Harbor can send the token as a raw value or with `Bearer ` prefix — updock accepts both.

### updock responds `no match`

None of your configured projects matched the pushed repository or tag. Check:
- `harbor_repository` spelling (case-insensitive, but the path must otherwise be exact)
- `tag_filter` pattern — try removing it temporarily to verify the repository matches first
- The logs will show the exact `repository` and `tags` values from the incoming event

### updock responds `400 Bad Request`

The body Harbor sent was not valid JSON. This should not happen with a real Harbor webhook but could indicate a proxy stripping or modifying the body.

### `compose_dir does not exist` error at startup

updock checks that every `compose_dir` path exists when it loads the config. Create the directory or fix the path before starting.

---

## Harbor webhook payload format

For reference, here is the JSON structure Harbor sends for a `PUSH_ARTIFACT` event:

```json
{
  "type": "PUSH_ARTIFACT",
  "occur_at": 1700000000,
  "operator": "admin",
  "event_data": {
    "resources": [
      {
        "digest": "sha256:abc123...",
        "tag": "v1.2.3",
        "resource_url": "registry.example.com/myproject/my-app:v1.2.3"
      }
    ],
    "repository": {
      "name": "my-app",
      "namespace": "myproject",
      "full_name": "myproject/my-app",
      "type": "private"
    }
  }
}
```

updock uses:
- `type` — must be `"PUSH_ARTIFACT"` to trigger anything
- `event_data.repository.full_name` — matched against `harbor_repository` in your config
- `event_data.resources[].tag` — matched against `tag_filter` in your config
