# Authentication

Astra has exactly one sign-in method: the browser login to your Astra deployment. API keys belong to custom model providers (see [Custom Models](11-custom-models.md)); they do not sign you in.

---

## Sign in (the only method)

On first launch, Astra prints the address of the authorization page:

```bash
astra
```

Open `https://astracode.topodrive.top/authorize`, sign in, and the site generates a CLI authorization code. Paste that code into the waiting terminal prompt and press Enter. The terminal consumes that same code to receive its token, so the code shown on the website and the one the CLI needs are always the same.

Astra stores credentials in `~/.astra/auth.json` and reuses them across sessions. Astra refreshes access tokens automatically in the background. When a token can't be refreshed, Astra prompts you to sign in again. Credentials without a server-provided expiry fall back to a 30-day lifetime.

### Credential storage

Tokens in `~/.astra/auth.json` (and MCP OAuth tokens in `~/.astra/mcp_credentials.json`) are written with owner-only permissions (`0600` on Unix). Anyone with filesystem access to those paths can use the credentials, so:

- Prefer full-disk encryption (FileVault, BitLocker, LUKS, or equivalent).
- Do not copy `auth.json` or `mcp_credentials.json` into shared directories, tickets, or chat.
- On multi-user hosts, keep `$HOME` / `$ASTRA_HOME` private to your account.

### Re-authenticate

To switch accounts or resolve an authentication problem, run:

```bash
astra login
```

Running `astra login` starts the same flow again, replacing your cached session: it prints `https://astracode.topodrive.top/authorize`, you generate a code there, and you paste it into the terminal.

There is one sign-in flow, so `astra login` needs no flag. The legacy `--oauth`, `--device-auth`, and `--device-code` flags are still accepted so existing scripts keep running; all three select the same device-code flow.

To sign out, run `astra logout`. It takes no flags and clears your cached credentials.

---

## API Key (custom providers)

A model-provider API key is not a login: it authenticates one `[model.*]`
endpoint, typically a self-hosted or third-party provider. `/connect` writes it
into your config for you. When you configure a provider by hand:

```bash
export ASTRA_API_KEY="sk-..."
```

`XAI_API_KEY` and `GROK_CODE_XAI_API_KEY` are still read as legacy fallbacks.
For per-model keys, prefer `api_key` / `env_key` inside that model's table in
`~/.astra/config.toml`.

---

---

## External Auth Provider

When browser-based login isn't possible -- for example, on sandboxed VMs, CI runners, or air-gapped networks -- delegate authentication to an external binary or script.

### How It Works

```
+--------------+     sh -c     +------------------------+
|     Astra     |-------------->|  your auth binary      |
|              |               |                        |
|  reads       |<-- stdout ----|  prints token          |
|  auth.json   |               |                        |
|              |   (stderr)    |  prints status/URLs    |--> surfaced to user
+--------------+               +------------------------+
```

1. Astra runs your command via `sh -c "<command>"`
2. Your binary runs whatever auth flow it needs (SSO, device code, certificate exchange)
3. **stderr** carries human-readable output, such as login URLs and status messages. Astra reads stderr and surfaces it to the user; in the TUI, it turns the first `https://` URL into a clickable sign-in link.
4. **stdout** is captured by Astra and saved as the access token
5. Exit 0 = success; exit non-zero = Astra falls back to interactive login

### The stdout / stderr Contract

| Stream | What to print | Who sees it |
|--------|---------------|-------------|
| **stdout** | The token -- nothing else | Astra (parsed and stored in auth.json) |
| **stderr** | Login URLs, status messages, errors | The user (Astra reads stderr and shows the sign-in URL as a clickable link in the TUI) |

**Do not print anything to stdout except the token.** No progress messages, no debug output. Astra reads stdout, trims surrounding whitespace, and parses the result as a token.

### stdout Token Format

**Bare string** -- just the raw token:

```
eyJhbGciOiJSUzI1NiIs...
```

**JSON** -- with optional refresh token, expiry, and issuer:

```json
{"access_token": "eyJhbGciOi...", "refresh_token": "ref-tok", "expires_in": 3600, "issuer": "https://idp.example.com"}
```

Use JSON if your tokens expire and you want Astra to automatically re-run the binary before expiry.

JSON fields:

| Field | Required | Meaning |
|-------|----------|---------|
| `access_token` | yes | Bearer token Astra sends to the Astra API |
| `refresh_token` | no | Stored for reference. Astra refreshes by re-running your binary, not with an OAuth refresh grant |
| `expires_in` | no | Token lifetime in seconds; enables proactive refresh before expiry |
| `issuer` | no | Identifies the token's issuer |

### Configuration

Via config file:

```toml
# ~/.astra/config.toml
[auth]
auth_provider_command = "/usr/local/bin/my-auth-provider"
auth_provider_label = "Acme Corp"   # optional -- customizes the TUI login button
auth_token_ttl = 3600               # optional -- token lifetime in seconds
```

Or via environment variables:

```bash
export ASTRA_AUTH_PROVIDER_COMMAND="/usr/local/bin/my-auth-provider"
export ASTRA_AUTH_PROVIDER_LABEL="Acme Corp"
export ASTRA_AUTH_TOKEN_TTL=3600
```

### Token Refresh

Astra runs your binary on two different contracts, and `ASTRA_AUTH_EXPIRED` is how
it tells them apart. Each run fully replaces the stored credential, so emit the
same JSON fields (such as `issuer`) on every invocation, including refreshes.

- **`ASTRA_AUTH_EXPIRED=1` — a headless refresh.** Astra is re-minting over a
  credential it already holds: a near-expiry rotation, or a token the server
  rejected. Nobody is watching. stdin is closed, your stderr is swallowed, and
  the binary is given a few seconds before it is killed. Mint silently or exit
  non-zero — never block.
- **Unset — a sign-in.** `astra login`, the sign-in screen, or the escalation
  Astra performs when a headless run couldn't mint. A user is waiting, your
  stderr reaches them, and you have 300 seconds — enough for a browser round
  trip or a device code.

```bash
#!/bin/sh
if [ "$ASTRA_AUTH_EXPIRED" = "1" ]; then
    # Headless: silent refresh only. Declining is the fast, correct answer
    # when your SSO session has lapsed and only the user can renew it.
    echo "Refreshing token..." >&2
    TOKEN=$(my-company-auth --refresh --silent) || exit 1
else
    echo "Authenticating via Acme Corp SSO..." >&2
    TOKEN=$(my-company-auth --login --interactive)
fi

if [ -z "$TOKEN" ]; then
    echo "Authentication failed" >&2
    exit 1
fi

echo "{\"access_token\": \"$TOKEN\", \"expires_in\": 3600}"
```

When the headless run can't produce a token, Astra stops treating the stored
credential as usable and starts the sign-in flow instead — the same one you get
on a machine that has never signed in, with your binary's stderr shown, so a
device-code URL or a browser prompt reaches you. Exiting promptly on
`ASTRA_AUTH_EXPIRED=1` is what makes that handover fast; a binary that blocks
instead makes you wait out the refresh timeout on every start. Mid-session, the
turn fails with a re-auth prompt and `/login` re-runs the binary interactively.

One case stays ambiguous, and only in **leader mode** (`--leader`, or
`[cli] use_leader = true`; off by default): with no credential at all, the
leader makes one extra attempt in the background just after startup, and that
run has the variable unset, like a sign-in. A binary that mints without help
(service account, keytab, mounted token) succeeds there and the session heals
itself. One that must prompt just sits, up to the 300s sign-in ceiling —
nothing waits on it, the sign-in screen is already up, and that run's stderr
goes to `~/.astra/leader.log` rather than to you.

### Environment Variables

| Variable | Description |
|----------|-------------|
| `ASTRA_AUTH_PROVIDER_COMMAND` | Path to your auth binary |
| `ASTRA_AUTH_PROVIDER_LABEL` | Display name on the TUI login screen (e.g., "Acme Corp") |
| `ASTRA_AUTH_TOKEN_TTL` | Token lifetime in seconds (for bare-string tokens without `expires_in`) |
| `ASTRA_AUTH_EXPIRED` | Set to `1` on a headless refresh: don't prompt, and don't hand back a cached token. Unset on a sign-in, where a user is attached |
| `ASTRA_AUTH_EARLY_INVALIDATION_SECS` | Seconds before expiry to proactively refresh (default: 300) |

---

## Device Code Flow (the only sign-in flow)

Every `astra login` runs this flow, locally or over SSH; there is no second flow to choose:

1. The terminal prints `https://astracode.topodrive.top/authorize` and waits for a code.
2. Open that URL on any device, sign in, and the site generates a CLI authorization code.
3. Paste the code into the terminal and press Enter. The terminal consumes that same code at `/api/auth/device/consume` and stores the returned token in `~/.astra/auth.json`.

The code is single-use, expires after 10 minutes, and is accepted regardless of case or surrounding spaces when pasted. `astra login --device-auth` / `--device-code` are accepted but change nothing; they are kept so existing scripts keep working.

You can also implement the device-code flow through an [External Auth Provider](#external-auth-provider) for full control.

---

## Automatic Credential Refresh

Astra automatically refreshes expired credentials:

- **Before expiry:** If your auth provider returned `expires_in` (JSON output) or you set `auth_token_ttl`, Astra re-runs the auth binary ~5 minutes before expiry.
- **On auth error:** If the server returns 401 Unauthorized, Astra refreshes the credentials and retries the request.
- **Session refresh:** when the stored credential carries a `refresh_token`, Astra refreshes it silently without another sign-in.

Tune the refresh buffer:

```bash
# Refresh 5 minutes before expiry (default)
export ASTRA_AUTH_EARLY_INVALIDATION_SECS=300

# Disable the proactive buffer: refresh at expiry or on a 401 (set to 0)
export ASTRA_AUTH_EARLY_INVALIDATION_SECS=0
```

---

## Hot Reload

Astra picks up changes to `~/.astra/auth.json` automatically. If you update credentials externally (for example, with a script that writes new tokens), Astra uses the new credentials on the next API call without a restart.

---

## Auth Precedence

Astra resolves credentials for each request in this order, highest to lowest:

1. **Per-model `api_key` or `env_key`** -- set under `[model.<name>]` in `config.toml`. Wins whenever present.
2. **Active session token** -- obtained from the Astra sign-in flow (or an external auth provider) and stored in `~/.astra/auth.json`.
3. **`XAI_API_KEY`** -- fallback when no session token is active.

When more than one login flow is configured, Astra populates the session token from the first available source, highest to lowest:

1. **External auth provider** (`auth_provider_command`)
2. **No other sign-in method** -- enterprise IdP support was removed; `[grok_com_config.oidc]` and `ASTRA_OIDC_*` are ignored, so there is no SSO alternative to the Astra sign-in flow.
3. **Astra OAuth2 browser login** -- the default

During a session, the active method handles all mid-session refreshes.

---

## Grove Git credentials (not this page's `grok login`)

Everything on this page authenticates Grok to the model API. Git remotes behind a Grove mount -- what [`grok clone`](27-grok-clone.md) fetches from -- are a separate world, owned by the Grove daemon.

**`~/.grok/auth.json` is never read for Git.** `grok login` does not create a Git credential and `grok logout` does not revoke one; the daemon builds its own credential cell from `auth_mode` in Grove config. Those credentials are managed with `grove status` and `grove reload-credentials` -- see [grok clone](27-grok-clone.md#authentication) for the failure classes and their next steps.

---

## Related settings

Coding-data sharing — **Coding data, retention, and training** in Settings,
which `/privacy` opens — does not change these config knobs:

| Setting | How to set it |
|---------|---------------|
| `[features] telemetry` | `config.toml` or `ASTRA_TELEMETRY_ENABLED` |
| `[telemetry] trace_upload` | `config.toml` or `ASTRA_TELEMETRY_TRACE_UPLOAD` |
| External OpenTelemetry | `ASTRA_EXTERNAL_OTEL` / `[telemetry] otel_*`. See [Monitoring Usage](24-monitoring-usage.md). |

On team accounts, only a team admin can change coding-data sharing.
Team admins can also enable or disable Zero Data Retention (ZDR) for their team.
See [How to enable ZDR](https://astracode.topodrive.top/docs/faq/security#how-to-enable-zdr).
When ZDR is on, coding-data sharing cannot be changed at all — the settings
row shows `ZDR` in place of the value. ZDR does not turn off external OTEL
or `user.email` — see [ZDR and this stream](24-monitoring-usage.md#zdr-and-this-stream).

See [Monitoring Usage](24-monitoring-usage.md#related-settings) and [Configuration](05-configuration.md#telemetry).

---

## Troubleshooting

### Debug logging

Set `RUST_LOG` to control the verbosity of the file log and headless stderr output. (The TUI's on-screen tracing pane uses a fixed filter and ignores `RUST_LOG`.) In the TUI, file logging defaults to `DEBUG`; in headless mode (`-p`), `RUST_LOG` defaults to `off` so only the answer is printed — set `RUST_LOG=error` (or broader) to see logs on stderr.

In the TUI, set `ASTRA_LOG_FILE` to an absolute path to write logs to that file:

```bash
ASTRA_LOG_FILE=/tmp/grok.log RUST_LOG=debug astra
tail -f /tmp/grok.log
```

`ASTRA_LOG_FILE` is treated as a literal file path. A relative value such as `1` writes a file named `1` in the current directory.

In headless mode, logs go to stderr. Redirect them to a file:

```bash
RUST_LOG=debug astra -p "hello" 2> /tmp/grok.log
```

### Common log messages

| Log message | What it means |
|-------------|---------------|
| `auth: running external auth provider (headless refresh)` / `(interactive login)` | Astra is running your binary, and on which contract |
| `auth: external auth provider returned fresh token` | Astra parsed and stored the token |
| `auth: external auth provider failed` | Binary exited non-zero or stdout was empty |
| `auth: external auth provider timed out (likely needs interactive auth), killing` | Binary did not exit before the timeout and was killed |
| `auth: failed to start external auth provider` | Command could not be spawned (binary not found) |

### Common fixes

- **"Authentication failed"** -- Run `astra logout` to clear cached credentials, then `astra login` to sign in again.
- **Token expires too quickly** -- Set `auth_token_ttl` or return `expires_in` in your auth provider's JSON output.
- **External auth provider not found** -- Check that the `auth_provider_command` path is correct and the binary is executable.
