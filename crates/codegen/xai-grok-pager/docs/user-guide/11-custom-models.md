# Custom Models

Astra connects to custom model endpoints for alternative providers, self-hosted models, and overriding built-in settings. This guide explains how to select models, configure endpoints, and integrate third-party providers.

---

## Default Models

By default, Astra uses models hosted by Astra, and new sessions start with `grok-4.5`. Default models require no configuration. Authenticate with `astra login` or an API key, then start a session.

List all available models:

```bash
astra models
```

---

## Connecting a Custom Model with `/connect`

The `/connect` slash command is a guided shortcut for writing the
`[model.astra-custom]` block described below. It knows the preset vendors and
pre-fills the protocol + base URL so you only supply a model id and key.

```text
/connect                              # open the guided preset picker
/connect <preset> <model_id> <key>    # one-shot connect (URL omitted for presets)
/connect custom <model_id> <key> <base_url>   # explicit endpoint
```

Supported presets:

| Preset | Protocol | Base URL |
|--------|----------|----------|
| `openai` | Chat Completions | `https://api.openai.com/v1` |
| `openai_responses` | Responses | `https://api.openai.com/v1` |
| `anthropic` | Messages | `https://api.anthropic.com/v1` |
| `xai` | Chat Completions | `https://api.x.ai/v1` |
| `deepseek` | Chat Completions | `https://api.deepseek.com/v1` |
| `zhipu` | Chat Completions | `https://open.bigmodel.cn/api/paas/v4` |
| `xiaomi` | Chat Completions | `https://api.xiaomimimo.com/v1` |
| `minimax_cn` | Chat Completions | `https://api.minimaxi.com/v1` |
| `zai` | Chat Completions | `https://api.z.ai/api/paas/v4` |
| `custom` | Chat Completions | (you provide it) |

For every preset the **model id is supplied by you** at connect time (the
preset only fixes the protocol + base URL). The guided help (`/connect` with
no arguments) advertises example model IDs per preset — for example
`gpt-5.6-luna`, `gpt-5.6-terra`, `gpt-5.6-sol` (GPT 5.6 Luna/Terra/Sol) for
OpenAI, and `claude-fable-5`, `claude-opus-5`, `claude-sonnet-5` (Claude
Fable 5/Opus 5/Sonnet 5) for Anthropic. **These are suggestions only** — the
model ID field is free-form, so any model ID the provider serves is accepted,
including ones not listed. Model IDs change frequently; **always verify
against the vendor's live `/v1/models` endpoint**.

| Preset | How to discover current model IDs |
|--------|-----------------------------------|
| `openai` | `GET https://api.openai.com/v1/models` |
| `openai_responses` | `GET https://api.openai.com/v1/models` |
| `anthropic` | `GET https://api.anthropic.com/v1/models` |
| `xai` | `GET https://api.x.ai/v1/models` |
| `deepseek` | `GET https://api.deepseek.com/v1/models` |
| `zhipu` | `GET https://open.bigmodel.cn/api/paas/v4/models` |
| `xiaomi` | `GET https://api.xiaomimimo.com/v1/models` |
| `minimax_cn` | `GET https://api.minimaxi.com/v1/models` |
| `zai` | `GET https://api.z.ai/api/paas/v4/models` |

These official endpoints expose a **native** reasoning channel
(`reasoning_content` / Anthropic `thinking`), so the preset vendors do **not**
set `injects_think_tags_in_content`. Some OpenAI-compatible gateways and
self-hosted models instead embed reasoning as `<think>…</think>` tags inside
the `content` field; for those, connect with `custom` and the thinking-tag
parser (gated by `injects_think_tags_in_content`) will strip the tags and
route the inner text to the thinking/reasoning panel automatically.

A restart is required after `/connect` for the new endpoint to take effect; it
becomes the default model for new sessions.

---


### CLI Flag

```bash
astra -p "Hello" -m grok-build
```

### Slash Command

In the TUI, switch models during a session:

```
/model grok-build
```

Or use the alias:

```
/m grok-build
```

### Model Picker (Ctrl+M)

Press `Ctrl+M` from the scrollback pane to open the model picker. It lists all available models, both built-in and custom, and lets you switch with a single keystroke. With the prompt focused, `Ctrl+M` toggles multiline input instead -- use `/model` to switch without leaving the prompt.

### Config Default

Set a persistent default in `~/.astra/config.toml`:

```toml
[models]
default = "grok-4.5"
```

---

## Supported API Backends

Astra supports three API backends. Set `api_backend` in your `[model.*]` config to choose which protocol the model uses:

| Value | API | Default |
|-------|-----|---------|
| `"chat_completions"` | OpenAI Chat Completions (`/v1/chat/completions`) | Yes |
| `"responses"` | OpenAI Responses (`/v1/responses`) | |
| `"messages"` | Anthropic Messages (`/v1/messages`) | |

When you omit `api_backend`, Astra uses `chat_completions`.

To send provider-specific authentication or version headers -- for example, Anthropic's `x-api-key` -- use the `extra_headers` field described below. Astra sends those headers verbatim with every request to the endpoint.

---

## Configuring Custom Models

Add custom model endpoints in `~/.astra/config.toml` under `[model.<name>]` sections:

```toml
[model.my-model]
model = "model-id"                        # Model identifier sent to the API
base_url = "https://api.example.com/v1"   # OpenAI-compatible endpoint
name = "Display Name"                     # Shown in the model picker
description = "Model description"          # Optional description
api_key = "sk-..."                        # API key for this provider (optional)
env_key = "XAI_API_KEY"                   # Env var holding the API key (optional; string or array)
api_backend = "chat_completions"          # "chat_completions", "responses", or "messages"
temperature = 0.7                         # Sampling temperature
top_p = 0.95                              # Nucleus sampling parameter
max_completion_tokens = 8192              # Maximum tokens per response
context_window = 128000                   # Total context window in tokens
extra_headers = { "x-api-key" = "sk-..." } # Extra request headers, sent verbatim (optional)
query_params = { api-version = "2026-07-22" } # Query params appended to every request URL (optional)
env_http_headers = { "X-Tenant" = "TENANT_TOKEN" }    # Headers from env vars, resolved at client build (optional)
```

### Credential Resolution

Astra resolves the API key in this order:

1. The `api_key` field in the model config
2. The environment variable(s) named by `env_key` — a single string or an array of names. The first set, non-empty value wins (for example `env_key = ["ANTHROPIC_AUTH_TOKEN", "LC_ANTHROPIC_AUTH_TOKEN"]` for SSH `LC_*` forwarding)
3. Your signed-in session token (from `astra login`), for a model with no `api_key`/`env_key` of its own
4. The `XAI_API_KEY` environment variable (global fallback; Astra also accepts `ASTRA_CODE_XAI_API_KEY` for backward compatibility)

### Context Window

The `context_window` value tells Astra when to trigger auto-compaction. When you override a known model, Astra inherits that model's context window. When you define a new model and omit `context_window`, Astra defaults to 200,000 tokens, so set it explicitly to match your provider.

### Global Default Headers

To apply the same headers to *every* model in the catalog -- built-in, prefetched from `/v1/models`, or custom -- set them once under the global `[models]` section instead of repeating them per model:

```toml
[models]
extra_headers = { "X-Request-Tags" = "team=example,env=prod" }
```

These act as a base for each model's inference requests. A per-model `[model.<id>].extra_headers` entry overrides the global default **per key** (matched case-insensitively): a key set on the model wins, while any global-only keys are still inherited by that model. Like the per-model field, they ride on that model's inference calls -- not on separate services such as image generation or video generation -- which makes them handy for attribution tags (for example, cost tracking) without re-declaring them whenever a new model appears.

### Global Default Values

A few common per-model settings can also be set once under `[models]` as a default for *every* model. A per-model `[model.<id>]` value always wins; the global only fills in where a model (or the server's model list) left the field unset:

```toml
[models]
temperature                 = 0.7
top_p                       = 0.95
max_completion_tokens       = 8192
max_retries                 = 8
inference_idle_timeout_secs = 600
stream_tool_calls           = true
```

This is a small, fixed set of environment-wide knobs. Settings that identify a specific model (`model`, `base_url`, `api_key`, `context_window`, ...) cannot be defaulted this way, and a few settings with their own dedicated configuration -- auto-compaction (`[session]`), the system-prompt label (`[agent]`), and reasoning effort (`[models].default_reasoning_effort`) -- keep their existing homes.

> **Note on `stream_tool_calls`:** this one affects request *shape*, not just sampling. A few endpoints (some BYOK providers) expect it left unset; if a global `stream_tool_calls = true` causes problems for such a model, opt that model out with `stream_tool_calls = false` in its `[model.<id>]` block.

### Request Query Parameters

Some gateways route or version on the query string. `query_params` appends percent-encoded query parameters to every request Astra makes for a model. For example, a gateway that selects an API version this way:

```toml
[model.my-gateway]
model = "my-model"
base_url = "https://gateway.example/v1"
api_backend = "responses"
env_key = "GATEWAY_API_KEY"
query_params = { api-version = "2026-07-22" }
```

A key that also appears in the `base_url` query string is overridden (last value wins) rather than duplicated. Query parameters are saved in the session, so do not put secrets in them: use `env_http_headers` for a secret.

### Environment-Variable Headers

`env_http_headers` maps a request header to the name of an environment variable that supplies its value, so a per-request secret never has to be written into `config.toml`:

```toml
[model.gateway]
model = "my-model"
base_url = "https://gateway.example/v1"
env_http_headers = { "X-Tenant-Token" = "GATEWAY_TENANT_TOKEN" }
```

Astra reads each variable when it builds the client for a session and places the value in the request headers only, never on disk. A header is skipped when its variable is unset or blank, and a resolved value overrides an `extra_headers` entry of the same name. Use `extra_headers` for a static value and `env_http_headers` for one that comes from the environment.

Both fields also work on a shared `[model_providers.<id>]` block. A model that points at a provider with `model_provider = "<id>"` inherits the provider's `query_params` and `env_http_headers` when it sets none of its own, matching how `extra_headers` is inherited.

---

## Overriding Built-in Models

You can override specific fields of built-in models without redefining everything. Only specify the fields you want to change:

```toml
# Override only the API key for a default model
[model.astra-build]
api_key = "my-api-key"

# Override temperature and add a custom API key
[model.astra-build]
temperature = 0.5
api_key = "sk-custom"
```

When you override a built-in model, Astra starts with the default configuration (including the correct `base_url`), then applies only the fields you specify. Unspecified fields inherit from the default.

### Priority Order

1. Your config (`[model.*]`) -- highest priority
2. Prefetched models from remote `/v1/models`
3. Hardcoded defaults -- lowest priority

---

## Provider Examples

### Anthropic (Claude)

Use Claude models directly via the Anthropic Messages API:

```toml
[model.claude-opus]
model = "claude-opus-5"
base_url = "https://api.anthropic.com/v1"
name = "Claude Opus 5"
api_backend = "messages"
context_window = 200000
extra_headers = { "x-api-key" = "sk-ant-...", "anthropic-version" = "2023-06-01" }
```

The `messages` backend uses the Anthropic Messages protocol. Anthropic authenticates with an `x-api-key` header rather than `Authorization: Bearer`, so pass your key through `extra_headers`, which Astra sends verbatim.

### OpenAI (Chat Completions)

```toml
[model.gpt-5.6-luna]
model = "gpt-5.6-luna"
base_url = "https://api.openai.com/v1"
name = "GPT 5.6 Luna"
env_key = "OPENAI_API_KEY"
```

`api_backend` defaults to `"chat_completions"`, so you don't need to set it explicitly for OpenAI.

### OpenAI (Responses API)

If your provider supports the newer Responses API:

```toml
[model.gpt-5.6-luna-responses]
model = "gpt-5.6-luna"
base_url = "https://api.openai.com/v1"
name = "GPT 5.6 Luna (Responses)"
api_backend = "responses"
env_key = "OPENAI_API_KEY"
```

### Ollama (Local Models)

Run models locally with [Ollama](https://ollama.ai):

```toml
[model.ollama-codellama]
model = "codellama"
base_url = "http://localhost:11434/v1"
name = "CodeLlama (Ollama)"
```

Make sure Ollama is running (`ollama serve`) and the model is pulled (`ollama pull codellama`).

### Together AI

```toml
[model.together-mixtral]
model = "mistralai/Mixtral-8x7B-Instruct-v0.1"
base_url = "https://api.together.xyz/v1"
name = "Mixtral 8x7B"
env_key = "TOGETHER_API_KEY"
```

### Local OpenAI-Compatible Server

Any server that implements the OpenAI Chat Completions or Responses API:

```toml
[model.local-llama]
model = "llama-3.1-70b"
base_url = "http://localhost:8080/v1"
name = "Local Llama"
temperature = 0.8
```

---

## Custom Models Endpoint

Point Astra at a custom OpenAI-compatible `/v1/models` endpoint instead of the default. Use this when your models sit behind a corporate gateway or a self-hosted inference service.

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `ASTRA_MODELS_BASE_URL` | Yes | Base URL for inference. Astra fetches the model list from `{base_url}/models`. |
| `XAI_API_KEY` | Yes | API key sent as `Authorization: Bearer`. Astra also accepts `ASTRA_CODE_XAI_API_KEY`. |
| `ASTRA_MODELS_LIST_URL` | No | Override the model-list URL when it differs from `{base_url}/models`. |

### Setup

```bash
export ASTRA_MODELS_BASE_URL="https://api.acme.com/v1"
export XAI_API_KEY="xai-..."
astra
```

### Config File Alternative

```toml
[endpoints]
models_base_url = "https://api.acme.com/v1"

# Override only the API key for a specific model
[model.astra-build]
api_key = "my-api-key"
```

When you use `[endpoints]` with partial model overrides, Astra inherits the `base_url` from the endpoints config, so you do not need to specify it in each `[model.*]` section.

### Auth Behavior

When you set `models_base_url`, Astra uses API key auth (`Authorization: Bearer`) instead of session auth. You do not need `astra login` -- the API key is enough.

---

## Web Search Model

The `web_search` tool uses a separate model. Configure it with:

```toml
[models]
web_search = "grok-4.5"
```

Or via environment variable:

```bash
export ASTRA_WEB_SEARCH_MODEL="grok-4.5"
```

If you point web search at a custom model, you also need a `[model.*]` entry so Astra can reach it. Server-side ("backend") web search runs only when the model sets `supports_backend_search = true` (and the build enables backend search); it does not depend on `api_backend`:

```toml
[models]
web_search = "my-custom-model"

[model.my-custom-model]
model = "my-custom-model"
supports_backend_search = true
```

---

## Using Custom Models

```bash
# List available models (including custom)
astra models

# Use in the TUI via slash command
/model my-model

# Use in headless mode
astra -p "Hello" -m my-model

# Set as default in config.toml:
[models]
default = "my-model"
```

---

## Enterprise Deployment

A complete config for an enterprise deployment with custom models:

```toml
[cli]
auto_update = false

[auth]
auth_provider_command = "/usr/local/bin/my-company-auth-provider"
auth_provider_label = "Acme Corp"
auth_token_ttl = 3600

[models]
default = "company-astra"

[model.company-astra]
model = "grok-build"
base_url = "https://grok-proxy.acme.com/"
name = "Astra Latest (Proxy)"
context_window = 128000

[features]
telemetry = false
```

---

## Troubleshooting

### Model Not Found

```bash
# List available models
astra models

# Check config.toml for typos in [model.*] sections
```

### Connection Errors

Verify the endpoint is reachable:

```bash
curl -s https://api.example.com/v1/models \
  -H "Authorization: Bearer $XAI_API_KEY"
```

### Debug Logging

```bash
RUST_LOG=debug ASTRA_LOG_FILE=/tmp/grok.log astra
tail -f /tmp/grok.log
```

Look for log entries containing `model` or `sampling` to trace model selection and API calls.
