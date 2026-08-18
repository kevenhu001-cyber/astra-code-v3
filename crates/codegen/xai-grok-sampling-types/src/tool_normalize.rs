//! Protocol-agnostic tool-call normalization for cross-protocol compatibility.
//!
//! Three inference backends carry the same logical concept — a tool definition
//! advertised to the model, and a tool call returned by the model — but with
//! different wire encodings:
//!
//! | Backend                          | Tool definition     | Tool call id      | Tool arguments          |
//! |----------------------------------|---------------------|-------------------|-------------------------|
//! | `ApiBackend::ChatCompletions`    | `ToolDefinition`    | caller-defined    | JSON-encoded `String`   |
//! | `ApiBackend::Responses`          | `responses::Tool`   | caller-defined    | JSON-encoded `String`   |
//! | `ApiBackend::Messages` (Anthropic)| `ToolParam`        | `toolu_*`         | parsed `serde_json::Value` |
//!
//! Without a shared normalization layer, each `build_*_request` re-implements
//! the protocol-specific mapping, which is where subtle drift lives:
//!
//! - Tool names that exceed OpenAI's strict 64-character limit get rejected
//!   by the upstream API; only Anthropic accepts arbitrary names.
//! - Tool-call `id` prefixes differ (`call_*` vs `toolu_*`); replayed history
//!   must round-trip through any backend without losing correlation.
//! - Chat Completions and Responses expect `arguments` as a JSON string,
//!   while Anthropic wants the parsed `serde_json::Value`; both must survive
//!   invalid JSON without panicking.
//!
//! This module is the single source of truth for those mappings. The three
//! `build_*_request` adapters in `conversation/` delegate here instead of
//! inlining their own copies.

use serde_json::Value;
use std::borrow::Cow;

use crate::conversation::{ConversationToolChoice, HostedTool, ToolSpec};
use crate::messages::{ToolChoiceParam, ToolParam};
use crate::{ApiBackend, ToolChoice, ToolDefinition, rs};

/// Maximum length OpenAI's Chat Completions and Responses APIs accept for a
/// tool/function name. The Anthropic Messages API has no comparable limit,
/// but we cap everything here so a single name works across all three.
pub const TOOL_NAME_MAX_LEN: usize = 64;

/// Maximum length for a tool-call identifier. Anthropic limits `tool_use_id`
/// to 64 characters and OpenAI tolerates similar lengths; we cap uniformly.
pub const TOOL_CALL_ID_MAX_LEN: usize = 64;

/// Protocol-agnostic tool definitions, post-normalization, in the wire shape
/// expected by each backend. Use [`normalize_tool_definitions_for`] to build.
#[derive(Debug)]
pub enum BackendTools<'a> {
    Chat(Cow<'a, [ToolDefinition]>),
    Responses(Cow<'a, [rs::Tool]>),
    Anthropic(Cow<'a, [ToolParam]>),
}

impl<'a> BackendTools<'a> {
    pub fn is_empty(&self) -> bool {
        match self {
            BackendTools::Chat(v) => v.is_empty(),
            BackendTools::Responses(v) => v.is_empty(),
            BackendTools::Anthropic(v) => v.is_empty(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            BackendTools::Chat(v) => v.len(),
            BackendTools::Responses(v) => v.len(),
            BackendTools::Anthropic(v) => v.len(),
        }
    }
}

/// Protocol-agnostic tool choice, in the wire shape expected by each backend.
#[derive(Debug, Clone)]
pub enum BackendToolChoice {
    Chat(ToolChoice),
    Responses(rs::ToolChoiceParam),
    Anthropic(ToolChoiceParam),
}

/// Build wire-shaped tool definitions for the given backend.
///
/// `hosted_tools` carries the names of any backend-hosted tools already on the
/// request; a client-side tool whose name collides with a hosted tool is
/// dropped on backends that share a single `tools` array, to avoid the API
/// rejecting the request as a duplicate. Anthropic and Chat Completions both
/// use one `tools` array; Responses too, but its serializer tolerates
/// duplicates as long as names differ — we drop uniformly for consistency.
pub fn normalize_tool_definitions_for(
    backend: ApiBackend,
    tools: &[ToolSpec],
    hosted_tools: &[HostedTool],
) -> BackendTools<'static> {
    match backend {
        ApiBackend::ChatCompletions => {
            let chat: Vec<ToolDefinition> = tools
                .iter()
                .filter(|t| !collides_with_hosted(&t.name, hosted_tools))
                .map(|t| {
                    ToolDefinition::function(
                        sanitize_tool_name(&t.name),
                        t.description.as_deref(),
                        t.parameters.clone(),
                    )
                })
                .collect();
            BackendTools::Chat(Cow::Owned(chat))
        }
        ApiBackend::Responses => {
            let resp: Vec<rs::Tool> = tools
                .iter()
                .filter(|t| !collides_with_hosted(&t.name, hosted_tools))
                .map(|t| {
                    rs::Tool::Function(rs::FunctionTool {
                        name: sanitize_tool_name(&t.name),
                        description: t.description.clone(),
                        parameters: Some(t.parameters.clone()),
                        strict: None,
                    })
                })
                .collect();
            BackendTools::Responses(Cow::Owned(resp))
        }
        ApiBackend::Messages => {
            let anth: Vec<ToolParam> = tools
                .iter()
                .filter(|t| !collides_with_hosted(&t.name, hosted_tools))
                .map(|t| ToolParam {
                    name: sanitize_tool_name(&t.name),
                    description: t.description.clone(),
                    input_schema: t.parameters.clone(),
                })
                .collect();
            BackendTools::Anthropic(Cow::Owned(anth))
        }
    }
}

fn collides_with_hosted(name: &str, hosted_tools: &[HostedTool]) -> bool {
    hosted_tools.iter().any(|h| h.wire_name() == name)
}

/// Build wire-shaped tool choice for the given backend. Pass `None` for
/// "no tool choice" — most backends default to `Auto` already, so the wire
/// shape only needs to be set when the user picked something explicit.
pub fn normalize_tool_choice_for(
    backend: ApiBackend,
    choice: ConversationToolChoice,
) -> BackendToolChoice {
    match backend {
        ApiBackend::ChatCompletions => {
            let tc = match choice {
                ConversationToolChoice::Auto => ToolChoice::auto(),
                ConversationToolChoice::None => ToolChoice::none(),
                ConversationToolChoice::Required => ToolChoice::required(),
                ConversationToolChoice::Function(name) => {
                    ToolChoice::function(sanitize_tool_name(&name))
                }
            };
            BackendToolChoice::Chat(tc)
        }
        ApiBackend::Responses => {
            let tc = match choice {
                ConversationToolChoice::Auto => {
                    rs::ToolChoiceParam::Mode(rs::ToolChoiceOptions::Auto)
                }
                ConversationToolChoice::None => {
                    rs::ToolChoiceParam::Mode(rs::ToolChoiceOptions::None)
                }
                ConversationToolChoice::Required => {
                    rs::ToolChoiceParam::Mode(rs::ToolChoiceOptions::Required)
                }
                ConversationToolChoice::Function(name) => {
                    rs::ToolChoiceParam::Function(rs::ToolChoiceFunction {
                        name: sanitize_tool_name(&name),
                    })
                }
            };
            BackendToolChoice::Responses(tc)
        }
        ApiBackend::Messages => {
            let tc = match choice {
                ConversationToolChoice::Auto => ToolChoiceParam::Auto,
                // Anthropic has no `None` for tool_choice; map to Auto and let
                // `tools` being empty handle suppression.
                ConversationToolChoice::None => ToolChoiceParam::Auto,
                // Anthropic calls `Required` "any" (any of the advertised tools).
                ConversationToolChoice::Required => ToolChoiceParam::Any,
                ConversationToolChoice::Function(name) => ToolChoiceParam::Tool {
                    name: sanitize_tool_name(&name),
                },
            };
            BackendToolChoice::Anthropic(tc)
        }
    }
}

/// Make a tool name safe for all three backends.
///
/// OpenAI rejects names that are not `[a-zA-Z0-9_-]{1,64}`. Anthropic accepts
/// more characters but the same cap keeps cross-protocol replay stable. Long
/// names are truncated to `TOOL_NAME_MAX_LEN - 8` characters and suffixed with
/// a short hash to keep distinct names from colliding.
pub fn sanitize_tool_name(name: &str) -> String {
    if name.is_empty() {
        return "_".to_string();
    }
    let mut out = String::with_capacity(name.len().min(TOOL_NAME_MAX_LEN));
    for c in name.chars() {
        let is_safe = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        out.push(if is_safe { c } else { '_' });
        if out.len() >= TOOL_NAME_MAX_LEN {
            break;
        }
    }
    if name.len() <= TOOL_NAME_MAX_LEN && out == name {
        return out;
    }
    // Truncated or substituted — append a stable 7-hex-char suffix to keep
    // distinct names distinct after the cap. Avoids `name_a` colliding with
    // `name_b` when both exceed 64 chars.
    let suffix = short_hash(name);
    let keep = TOOL_NAME_MAX_LEN.saturating_sub(suffix.len() + 1);
    out.truncate(keep);
    out.push('_');
    out.push_str(&suffix);
    out
}

/// Make a tool-call id safe for all three backends. The `toolu_*` and
/// `call_*` prefixes Anthropic and OpenAI use are preserved when present so
/// replayed history stays byte-identical to what the original model emitted.
pub fn sanitize_tool_call_id(id: &str) -> String {
    if id.is_empty() {
        return format!("toolu_{}", short_hash("empty"));
    }
    let mut out = String::with_capacity(id.len().min(TOOL_CALL_ID_MAX_LEN));
    for c in id.chars() {
        let is_safe = c.is_ascii_alphanumeric() || c == '_' || c == '-';
        out.push(if is_safe { c } else { '_' });
        if out.len() >= TOOL_CALL_ID_MAX_LEN {
            break;
        }
    }
    if id.len() <= TOOL_CALL_ID_MAX_LEN && out == id {
        return out;
    }
    let suffix = short_hash(id);
    let keep = TOOL_CALL_ID_MAX_LEN.saturating_sub(suffix.len() + 1);
    out.truncate(keep);
    out.push('_');
    out.push_str(&suffix);
    out
}

/// Parse a tool-call's `arguments` payload into a `serde_json::Value`. Chat
/// Completions and Responses ship `arguments` as a JSON-encoded string;
/// Anthropic ships it pre-parsed. This function handles both shapes and never
/// panics on malformed input — a parse failure yields `Value::Null` and a
/// warning, so the upstream call still has *something* to log.
pub fn normalize_tool_call_arguments(arguments: &str) -> Value {
    if arguments.is_empty() {
        return Value::Null;
    }
    match serde_json::from_str(arguments) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                error = %err,
                "tool_call_arguments: invalid JSON, substituting null"
            );
            Value::Null
        }
    }
}

/// Stable, short hash of an arbitrary string. Used for suffixing names/ids
/// that hit the per-backend length cap. 7 hex chars = 28 bits = enough to
/// avoid collisions on the small tool inventories this app ships.
fn short_hash(input: &str) -> String {
    // FNV-1a 32-bit; cheap, deterministic, dependency-free.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in input.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:07x}", hash & 0x0fff_ffff)
}

#[cfg(test)]
#[path = "tool_normalize_tests.rs"]
mod tests;
