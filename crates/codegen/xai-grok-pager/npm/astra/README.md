# Astra

Bring Astra into your terminal. Fast, flicker-free CLI built for plans, subagents, and parallel work.

**[Homepage](https://astracode.topodrive.top)** | **[Documentation](https://example.invalid/docs)**

## Install

```bash
curl -fsSL https://astracode.topodrive.top/install/install.sh | bash
```

Or install with npm:

```bash
npm i -g @xai-official/grok
```

## Get Started

```bash
# Launch the interactive TUI
astra

# Run a single task
astra -p "Explain this codebase"
```

On first launch, Astra opens your browser to authenticate. For CI or headless environments, use an API key from your model provider's console:

```bash
export XAI_API_KEY="xai-..."
```

## Update

```bash
astra update
```

Or if installed via npm:

```bash
npm i -g @xai-official/grok@latest
```

## Supported Platforms

| Platform | Architecture |
|---|---|
| macOS | Apple Silicon (arm64) |
| Linux | x86_64, arm64 |
| Windows | x86_64 |

## Documentation

For full documentation including configuration, MCP servers, custom models, headless mode, agent mode, and more, visit [docs example](https://example.invalid/docs).

## Feedback

Run `/feedback` inside Astra to report issues or send feedback directly.
