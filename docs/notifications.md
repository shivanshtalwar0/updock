# Notifications

updock can alert you whenever containers are updated or fail to update.
Notifications are sent at the end of every watcher scan cycle — but **only when
there is something to report** (i.e. at least one container was updated or
failed). Scans where every container is already up-to-date produce no
notification.

---

## Table of contents

- [When notifications are sent](#when-notifications-are-sent)
- [Message format](#message-format)
- [Generic webhook URLs](#generic-webhook-urls)
- [Slack](#slack)
- [Gotify](#gotify)
- [Email (SMTP)](#email-smtp)
- [Using multiple channels at once](#using-multiple-channels-at-once)
- [Full example configuration](#full-example-configuration)
- [Troubleshooting](#troubleshooting)

---

## When notifications are sent

A notification is dispatched after each watcher scan **if and only if** at
least one container was updated or at least one container failed to update.
Scans that find nothing to do (every container is already running the latest
image) are silent.

The webhook server side of updock (Harbor webhook → `docker compose up`) does
**not** send notifications — notifications are a watcher-loop feature only.

---

## Message format

Every notification channel receives the same plain-text message, regardless of
channel type:

```
[updock] 5 scanned, 2 updated, 1 failed
  ✓ updated: my-app
  ✓ updated: worker
  ✗ failed:  sidecar
  - skipped: nginx
  - skipped: redis
```

| Symbol | Meaning |
|--------|---------|
| `✓ updated:` | Container was successfully updated to the new image |
| `✗ failed:` | Update was attempted but failed (pull error, restart failure, etc.) |
| `- skipped:` | Container was seen but skipped (e.g. pinned tag, excluded by label) |

The summary line always shows the **total** counts, even when individual lines
are absent.

---

## Generic webhook URLs

The simplest channel. updock will `POST` a JSON body to every URL you list.
This works with any service that accepts a `{"text": "..."}` payload —
Discord, Microsoft Teams, Mattermost, custom endpoints, etc.

```yaml
notifications:
  urls:
    - https://hooks.example.com/my-endpoint
    - https://discord.com/api/webhooks/1234/abcd
```

**Payload sent:**

```json
{
  "text": "[updock] 5 scanned, 2 updated, 1 failed\n  ✓ updated: my-app\n  ..."
}
```

**HTTP behaviour:**
- Request timeout: 15 seconds
- Non-2xx responses are logged as warnings; updock continues to the next channel
- Network errors are logged as errors; other channels are still attempted

> **Note:** For Slack, Discord, and Teams there are often service-specific
> fields (like `username` or `embeds`). Use the generic URL channel if you only
> need the basic message. Use the dedicated Slack channel (below) if you need
> username or channel overrides.

---

## Slack

Use the dedicated Slack channel if you want to set a custom bot name or post to
a specific channel.

```yaml
notifications:
  slack_webhook_url: https://hooks.slack.com/services/T.../B.../xxx
  slack_username: updock-bot      # optional — defaults to "updock"
  slack_channel: "#deployments"  # optional — omit to use the webhook's default channel
```

| Key | Required | Default | Description |
|-----|----------|---------|-------------|
| `slack_webhook_url` | Yes | — | Incoming Webhook URL from Slack app settings |
| `slack_username` | No | `"updock"` | Display name shown in Slack |
| `slack_channel` | No | webhook default | Channel to post to, e.g. `"#ops"` |

**Payload sent:**

```json
{
  "text": "[updock] ...",
  "username": "updock-bot",
  "channel": "#deployments"
}
```

When `slack_channel` is not set the `channel` key is **omitted entirely** from
the payload (not sent as `null`), so Slack routes the message to the channel
configured in the webhook app settings.

### How to get a Slack Incoming Webhook URL

1. Go to **api.slack.com/apps** → Your App → **Incoming Webhooks**
2. Toggle **Activate Incoming Webhooks** → On
3. Click **Add New Webhook to Workspace** and choose a channel
4. Copy the `https://hooks.slack.com/services/...` URL

---

## Gotify

[Gotify](https://gotify.net/) is a self-hosted push notification server.
Both `gotify_url` and `gotify_token` **must** be set; if either is missing
Gotify notifications are silently skipped.

```yaml
notifications:
  gotify_url: https://gotify.example.com
  gotify_token: AbCdEfGhIjKlMnOp
```

| Key | Required | Description |
|-----|----------|-------------|
| `gotify_url` | Yes | Base URL of your Gotify server (no trailing slash needed) |
| `gotify_token` | Yes | Application token from Gotify → Apps |

**Request sent:**

```
POST https://gotify.example.com/message?token=AbCdEfGhIjKlMnOp
```

**Payload:**

```json
{
  "title": "updock",
  "message": "[updock] 5 scanned, 2 updated, 1 failed\n  ...",
  "priority": 5
}
```

Priority `5` maps to "normal" in the default Gotify UI theme.

---

## Email (SMTP)

updock can send email via any SMTP server. STARTTLS is used by default.
One email is sent per recipient address.

```yaml
notifications:
  email:
    smtp_host: smtp.example.com
    smtp_port: 587          # default: 587
    username: alerts@example.com
    password: "s3cr3t"
    from: "updock <alerts@example.com>"
    to:
      - ops@example.com
      - admin@example.com
    tls_skip_verify: false  # default: false
```

| Key | Required | Default | Description |
|-----|----------|---------|-------------|
| `smtp_host` | Yes | — | SMTP server hostname |
| `smtp_port` | No | `587` | SMTP port |
| `username` | No | — | SMTP auth username (omit if server allows unauthenticated relay) |
| `password` | No | — | SMTP auth password |
| `from` | Yes | — | Sender address, e.g. `"updock <no-reply@example.com>"` |
| `to` | Yes | — | List of recipient addresses |
| `tls_skip_verify` | No | `false` | Skip TLS certificate verification (**development only**) |

**Subject line:**

```
[updock] Container update report
```

**Body:** the same plain-text message shown in [Message format](#message-format).

> **Security note:** `tls_skip_verify: true` disables certificate validation
> entirely. Only use it in a private lab environment. Never use it in
> production.

---

## Using multiple channels at once

All channels are independent and can be combined freely:

```yaml
notifications:
  # Generic webhooks
  urls:
    - https://hooks.example.com/my-channel

  # Slack
  slack_webhook_url: https://hooks.slack.com/services/T.../B.../xxx
  slack_channel: "#ops"

  # Gotify
  gotify_url: https://gotify.example.com
  gotify_token: AbCdEfGhIjKlMnOp

  # Email
  email:
    smtp_host: smtp.example.com
    from: updock@example.com
    to:
      - team@example.com
```

All configured channels are attempted on each notification. A failure in one
channel (e.g. network timeout) does not prevent the others from being tried.

---

## Full example configuration

```yaml
server:
  bind: 0.0.0.0:8080

watcher:
  enabled: true
  schedule: "0 0 4 * * *"   # run at 04:00 every day

notifications:
  urls:
    - https://discord.com/api/webhooks/1234/abcd
  slack_webhook_url: https://hooks.slack.com/services/T000/B000/xxxx
  slack_username: updock
  slack_channel: "#deployments"
  gotify_url: https://gotify.example.com
  gotify_token: MyGotifyToken
  email:
    smtp_host: smtp.gmail.com
    smtp_port: 587
    username: me@gmail.com
    password: "app-password"
    from: "updock <me@gmail.com>"
    to:
      - ops@example.com
    tls_skip_verify: false
```

---

## Troubleshooting

### No notifications are arriving

1. **Check that the watcher is enabled.** Notifications only fire from the
   watcher loop, not from the webhook server.
2. **Check whether there was anything to report.** If every container was
   already up to date, no notification is sent. Check the logs for
   `"watcher scan complete"` and look at the `updated` and `failed` counts.
3. **Enable debug logging** with `RUST_LOG=debug` to see per-channel
   send/error messages.

### Slack returns a non-2xx status

- Verify the webhook URL is correct (it expires if the Slack app is
  reinstalled).
- Check that the channel name uses the `#channel` format if overriding.

### Email fails with a TLS error

- Make sure `smtp_port` is correct for your server (`587` for STARTTLS,
  `465` for implicit TLS — note: implicit TLS is not supported; use `587`).
- If your server uses a self-signed certificate in a trusted lab, set
  `tls_skip_verify: true` temporarily to confirm connectivity.

### Gotify shows no messages

- Confirm **both** `gotify_url` and `gotify_token` are set. If either is
  missing, the Gotify channel is silently disabled.
- Verify the token belongs to an **application** (not a client) in Gotify.
