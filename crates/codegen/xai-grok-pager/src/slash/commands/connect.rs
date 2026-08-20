//! `/connect` — configure a custom model provider (URL, Model ID, API key).
//!
//! Guides the user through connecting a BYO-model endpoint. Preset vendors
//! (OpenAI, Anthropic, xAI, DeepSeek, 智谱/Zhipu, 小米/Xiaomi, MiniMax CN,
//! zAI, …) pre-fill the protocol + base URL; `custom` takes an explicit URL.
//!
//! Usage:
//!   /connect                              open the guided preset picker
//!   /connect <preset> <model_id> <key>    one-shot connect (URL omitted for presets)
//!   /connect <preset> <model_id> <key> <base_url>   with explicit endpoint
//!
//! The guided help advertises example model IDs per preset
//! ([`PRESET_EXAMPLE_MODELS`]) — suggestions only. The model ID is free-form:
//! any ID the provider serves is accepted, so preset vendors always allow
//! custom model IDs (e.g. `/connect openai gpt-5.6-luna sk-…`).
//!
//! The command writes the `[model.astra-custom]` block via the shell's config
//! persist path and pins it as `[models].default`. A restart is required for
//! the new endpoint to take effect.

use crate::acp::model_state::ModelState;
use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Canonical preset ids advertised by `/connect`. Mirrors the shell's
/// `CUSTOM_MODEL_PROVIDER_PRESETS` (kept inline so the pager dropdown needs
/// no extra dependency on shell config internals).
const CONNECT_PRESETS: &[(&str, &str)] = &[
    ("openai", "OpenAI (Chat Completions)"),
    ("openai_responses", "OpenAI (Responses)"),
    ("anthropic", "Anthropic (Messages)"),
    ("xai", "xAI (Grok)"),
    ("deepseek", "DeepSeek"),
    ("zhipu", "智谱 Zhipu AI"),
    ("xiaomi", "小米 Xiaomi"),
    ("minimax_cn", "MiniMax CN"),
    ("zai", "zAI"),
    ("custom", "Other (custom URL)"),
];

/// Example model IDs advertised by `/connect` for preset vendors. Suggestions
/// only — the model ID field stays free-form, so any ID the provider serves
/// is accepted (including IDs not listed here).
const PRESET_EXAMPLE_MODELS: &[(&str, &[&str])] = &[
    (
        "openai",
        &["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"],
    ),
    (
        "openai_responses",
        &["gpt-5.6-luna", "gpt-5.6-terra", "gpt-5.6-sol"],
    ),
    (
        "anthropic",
        &["claude-fable-5", "claude-opus-5", "claude-sonnet-5"],
    ),
];

/// Example model IDs for a preset vendor, if any.
fn preset_example_models(provider: &str) -> &'static [&'static str] {
    PRESET_EXAMPLE_MODELS
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, models)| *models)
        .unwrap_or(&[])
}

/// Presets that embed reasoning as `<think>…</think>` tags in `content`.
/// The official endpoints of the listed preset vendors all expose a native
/// reasoning channel, so this list is intentionally empty — the think-tag
/// splitter is opt-in for endpoints (custom proxies / self-hosted) that don't
/// surface a native reasoning field.
/// `minimax_cn` is the exception: its OpenAI-compatible /v1/chat/completions
/// endpoint does not expose a native reasoning_content delta - MiniMax
/// embeds thinking in content as <thinking>...</thinking> tags. The splitter
/// is a no-op when the stream carries no such tags, so it is safe to enable.
const THINK_TAG_PRESETS: &[&str] = &["minimax_cn"];

pub struct ConnectCommand;

impl SlashCommand for ConnectCommand {
    fn name(&self) -> &str {
        "connect"
    }

    fn description(&self) -> &str {
        "Connect a custom model (URL, Model ID, API key)"
    }

    fn usage(&self) -> &str {
        "/connect [preset] [model_id] [api_key] [base_url]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<preset> [model_id] [api_key] [base_url]")
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(
            CONNECT_PRESETS
                .iter()
                .map(|(id, label)| {
                    let examples = preset_example_models(id);
                    let description = if examples.is_empty() {
                        "Preset vendor — select to configure".to_string()
                    } else {
                        format!("Preset vendor — e.g. {}", examples.join(", "))
                    };
                    ArgItem {
                        display: label.to_string(),
                        match_text: id.to_string(),
                        insert_text: format!("{id} "),
                        description,
                    }
                })
                .collect(),
        )
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            // No args -> open the guided wizard instead of dumping help
            // text. The wizard walks URL -> model id -> API key, and
            // submits through the same `Action::ConnectCustomModel` path
            // the one-shot command would have used. Re-running `/connect`
            // with no args is now the canonical "I want to set up a model"
            // entry point.
            return CommandResult::Action(Action::OpenConnectWizard);
        }

        // Split into at most 4 whitespace-delimited tokens. The base URL may
        // itself contain no spaces (http(s)://…), so simple splitting is safe.
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let (provider, rest) = parts.split_first().expect("non-empty");

        let provider_lc = provider.to_ascii_lowercase();
        let preset = CONNECT_PRESETS
            .iter()
            .find(|(id, _)| *id == provider_lc)
            .map(|(id, _)| *id)
            .or_else(|| {
                // `custom` accepts any provider spelling; everything else must
                // match a known preset, but we also tolerate an explicit URL as
                // the first token for power users.
                CONNECT_PRESETS
                    .iter()
                    .find(|(id, _)| id.eq_ignore_ascii_case(provider))
                    .map(|(id, _)| *id)
            });

        let provider = match preset {
            Some(p) => p,
            None => {
                return CommandResult::Error(format!(
                    "Unknown provider '{provider}'. Run /connect to see presets."
                ));
            }
        };

        // Resolve model id / key / optional base url from the remaining tokens.
        let mut model_id = String::new();
        let mut api_key = String::new();
        let mut base_url = String::new();

        if let Some((mid, tail)) = rest.split_first() {
            model_id = mid.to_string();
            if let Some((key, tail)) = tail.split_first() {
                api_key = key.to_string();
                if let Some((url, _)) = tail.split_first() {
                    base_url = url.to_string();
                }
            }
        }

        if model_id.is_empty() {
            return CommandResult::Action(Action::OpenConnectWizard);
        }

        let injects_think_tags = THINK_TAG_PRESETS.contains(&provider);
        let is_custom = provider == "custom";

        // For preset vendors, default the base URL from the canonical preset so
        // the user only needs to supply model id + key. For `custom`, the URL
        // is required.
        let resolved_base_url = if !base_url.is_empty() {
            base_url.clone()
        } else if is_custom {
            return CommandResult::Error(format!(
                "The 'custom' preset requires a base URL: /connect custom {model_id} <api_key> <base_url>"
            ));
        } else {
            preset_base_url(provider).to_string()
        };

        let display_name = display_name_for(provider, &model_id);

        // API key is optional for providers that use session/network auth; warn
        // but proceed so a URL-only endpoint still connects.
        if api_key.is_empty() {
            return CommandResult::Message(format!(
                "No API key supplied for {provider}. Re-run with a key:\n\
                 /connect {provider} {model_id} <api_key>{0}",
                if is_custom {
                    format!(" {resolved_base_url}")
                } else {
                    String::new()
                }
            ));
        }

        CommandResult::Action(Action::ConnectCustomModel {
            provider: provider.to_string(),
            model_id,
            display_name,
            api_key,
            base_url: resolved_base_url,
            injects_think_tags,
        })
    }
}

/// Baked-in base URL for a preset vendor (mirrors the shell's presets).
fn preset_base_url(provider: &str) -> &'static str {
    match provider {
        "openai" => "https://api.openai.com/v1",
        "openai_responses" => "https://api.openai.com/v1",
        "anthropic" => "https://api.anthropic.com/v1",
        "xai" => "https://api.x.ai/v1",
        "deepseek" => "https://api.deepseek.com/v1",
        "zhipu" => "https://open.bigmodel.cn/api/paas/v4",
        "xiaomi" => "https://api.xiaomimimo.com/v1",
        "minimax_cn" => "https://api.minimaxi.com/v1",
        "zai" => "https://api.z.ai/api/paas/v4",
        _ => "",
    }
}

/// Human-readable default display name for a connected model.
fn display_name_for(provider: &str, model_id: &str) -> String {
    let vendor = CONNECT_PRESETS
        .iter()
        .find(|(id, _)| *id == provider)
        .map(|(_, label)| *label)
        .unwrap_or("Custom");
    format!("{vendor} · {model_id}")
}

/// Help text shown when `/connect` is run with no arguments.
fn guided_help() -> String {
    let mut s = String::from(
        "Connect a custom model. Pick a preset, then supply model id + key:\n\n",
    );
    for (id, label) in CONNECT_PRESETS {
        s.push_str(&format!("  /connect {id} <model_id> <api_key>\n    {label}\n"));
        let examples = preset_example_models(id);
        if !examples.is_empty() {
            s.push_str(&format!("    e.g. {}\n", examples.join(", ")));
        }
    }
    s.push_str(
        "\nExamples are suggestions only — any model ID the provider serves works.\n\
         For 'custom', append the base URL:\n  /connect custom <model_id> <api_key> <base_url>\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::actions::Action;
    use crate::slash::command::{AppCtx, CommandExecCtx, CommandResult};
    use std::sync::OnceLock;

    static EMPTY_BUNDLE: crate::app::bundle::BundleState = crate::app::bundle::BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    /// Shared empty `ModelState` for tests. `Default` is not const, so the
    /// instance is lazily initialized once and borrowed for `'static`.
    fn empty_models() -> &'static ModelState {
        static MODELS: OnceLock<ModelState> = OnceLock::new();
        MODELS.get_or_init(ModelState::default)
    }

    fn ctx() -> CommandExecCtx<'static> {
        CommandExecCtx {
            models: empty_models(),
            session_id: None,
            bundle_state: &EMPTY_BUNDLE,
            screen_mode: crate::app::ScreenMode::Inline,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        }
    }

    fn run(args: &str) -> CommandResult {
        let mut c = ctx();
        ConnectCommand.run(&mut c, args)
    }

    #[test]
    fn no_args_opens_wizard() {
        // No args -> the guided wizard, not a help message.
        assert!(matches!(
            run(""),
            CommandResult::Action(Action::OpenConnectWizard)
        ));
    }

    #[test]
    fn preset_only_opens_wizard() {
        // `/connect openai` alone (no model id, no key) also routes to the
        // wizard — it's the typed shortcut to "open the form pre-loaded
        // with this preset".
        assert!(matches!(
            run("openai"),
            CommandResult::Action(Action::OpenConnectWizard)
        ));
    }

    #[test]
    fn unknown_provider_errors() {
        assert!(matches!(run("bogus gpt-5.6-luna x"), CommandResult::Error(_)));
    }

    #[test]
    fn unknown_provider_with_no_args_opens_wizard() {
        // Empty args or single-token unknown names both end up in the
        // wizard; the wizard then validates the preset on its own.
        assert!(matches!(
            run(""),
            CommandResult::Action(Action::OpenConnectWizard)
        ));
    }

    #[test]
    fn openai_preset_connects_without_url() {
        let res = run("openai gpt-5.6-luna sk-test123");
        match res {
            CommandResult::Action(Action::ConnectCustomModel {
                provider,
                model_id,
                display_name,
                api_key,
                base_url,
                injects_think_tags,
            }) => {
                assert_eq!(provider, "openai");
                assert_eq!(model_id, "gpt-5.6-luna");
                assert_eq!(api_key, "sk-test123");
                assert_eq!(base_url, "https://api.openai.com/v1");
                assert!(!injects_think_tags);
                assert!(display_name.contains("gpt-5.6-luna"));
            }
            other => panic!("expected ConnectCustomModel, got {other:?}"),
        }
    }

    #[test]
    fn deepseek_preset_does_not_flag_think_tags() {
        // Official DeepSeek exposes a native reasoning channel, so the preset
        // must NOT enable the `<think>`-tag splitter.
        let res = run("deepseek deepseek-v4-flash sk-ds");
        match res {
            CommandResult::Action(Action::ConnectCustomModel {
                provider,
                base_url,
                injects_think_tags,
                model_id,
                ..
            }) => {
                assert_eq!(provider, "deepseek");
                assert_eq!(model_id, "deepseek-v4-flash");
                assert_eq!(base_url, "https://api.deepseek.com/v1");
                assert!(!injects_think_tags);
            }
            other => panic!("expected ConnectCustomModel, got {other:?}"),
        }
    }

    #[test]
    fn minimax_preset_enables_think_tags() {
        // MiniMax's OpenAI-compatible endpoint embeds thinking in `content` as
        // `<thinking>…</thinking>` tags (no native reasoning_content delta), so
        // the preset must enable the tag splitter.
        let res = run("minimax_cn MiniMax-M3 sk-mm");
        match res {
            CommandResult::Action(Action::ConnectCustomModel {
                provider,
                base_url,
                injects_think_tags,
                model_id,
                ..
            }) => {
                assert_eq!(provider, "minimax_cn");
                assert_eq!(model_id, "MiniMax-M3");
                assert_eq!(base_url, "https://api.minimaxi.com/v1");
                assert!(injects_think_tags);
            }
            other => panic!("expected ConnectCustomModel, got {other:?}"),
        }
    }

    #[test]
    fn custom_requires_base_url() {
        // Missing URL → error, since the preset has no baked-in endpoint.
        assert!(matches!(
            run("custom my-model sk-key"),
            CommandResult::Error(_)
        ));
    }

    #[test]
    fn custom_with_explicit_url_connects() {
        let res = run("custom my-model sk-key https://example.com/v1");
        match res {
            CommandResult::Action(Action::ConnectCustomModel {
                provider,
                base_url,
                injects_think_tags,
                ..
            }) => {
                assert_eq!(provider, "custom");
                assert_eq!(base_url, "https://example.com/v1");
                assert!(!injects_think_tags);
            }
            other => panic!("expected ConnectCustomModel, got {other:?}"),
        }
    }

    #[test]
    fn missing_api_key_returns_guidance() {
        assert!(matches!(
            run("openai gpt-5.6-luna"),
            CommandResult::Message(_)
        ));
    }

    #[test]
    fn guided_help_advertises_example_model_ids() {
        // Help text now lives behind an internal-only call (no slash command
        // path triggers it). The wizard surfaces the same info via preset
        // labels instead. This test stays as a regression check for the
        // underlying `guided_help()` formatter so the strings it composes
        // stay in sync if anyone wires a non-wizard help path back in.
        let msg = guided_help();
        // OpenAI preset examples.
        assert!(msg.contains("gpt-5.6-luna"), "{msg}");
        assert!(msg.contains("gpt-5.6-terra"), "{msg}");
        assert!(msg.contains("gpt-5.6-sol"), "{msg}");
        // Anthropic preset examples.
        assert!(msg.contains("claude-fable-5"), "{msg}");
        assert!(msg.contains("claude-opus-5"), "{msg}");
        assert!(msg.contains("claude-sonnet-5"), "{msg}");
        // Examples are suggestions; custom IDs remain accepted.
        assert!(msg.contains("any model ID the provider serves works"), "{msg}");
    }

    #[test]
    fn preset_accepts_custom_model_id() {
        // Preset vendors accept any model ID, not just the advertised examples.
        let res = run("openai my-custom-model-id sk-test123");
        match res {
            CommandResult::Action(Action::ConnectCustomModel { model_id, .. }) => {
                assert_eq!(model_id, "my-custom-model-id");
            }
            other => panic!("expected ConnectCustomModel, got {other:?}"),
        }
    }

    #[test]
    fn suggest_args_lists_all_presets() {
        let c = ctx();
        let items = ConnectCommand
            .suggest_args(&crate::slash::command::AppCtx {
                models: empty_models(),
                cwd: std::path::Path::new("."),
                has_session_announcements: false,
                billing_surface_visible: true,
                usage_command_visible: true,
                workflows_available: true,
                screen_mode: crate::app::ScreenMode::Inline,
                current_title: None,
            }, "")
            .expect("suggestions");
        assert!(items.iter().any(|i| i.match_text == "openai"));
        assert!(items.iter().any(|i| i.match_text == "anthropic"));
        assert!(items.iter().any(|i| i.match_text == "xai"));
        assert!(items.iter().any(|i| i.match_text == "deepseek"));
        assert!(items.iter().any(|i| i.match_text == "zhipu"));
        assert!(items.iter().any(|i| i.match_text == "xiaomi"));
        assert!(items.iter().any(|i| i.match_text == "minimax_cn"));
        assert!(items.iter().any(|i| i.match_text == "zai"));
        assert!(items.iter().any(|i| i.match_text == "custom"));
        // Presets with example model IDs advertise them in the description.
        let openai = items.iter().find(|i| i.match_text == "openai").unwrap();
        assert!(openai.description.contains("gpt-5.6-luna"), "{}", openai.description);
        let anthropic = items.iter().find(|i| i.match_text == "anthropic").unwrap();
        assert!(anthropic.description.contains("claude-sonnet-5"), "{}", anthropic.description);
    }
}

