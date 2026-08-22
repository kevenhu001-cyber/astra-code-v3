//! Round-trip tests for `tool_normalize`. Each test pins a contract that
//! holds across the three backends the agent supports.
//!
//! Test fixtures use only fields the wire types expose; `serde_json::json!`
//! macros keep schema authoring compact.

use serde_json::{Value, json};

use super::*;
use crate::conversation::{ConversationToolChoice, HostedTool, ToolSpec};
use crate::messages::{ContentBlock, ToolParam, ToolResultContent};
use crate::types::ToolChoiceFunction;

fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        description: Some(format!("test tool {name}")),
        parameters: json!({
            "type": "object",
            "properties": {"x": {"type": "integer"}},
            "required": ["x"],
            "additionalProperties": false
        }),
    }
}

fn chat_name_of(bt: &BackendTools<'_>) -> String {
    match bt {
        BackendTools::Chat(v) => v[0].function.name.clone(),
        _ => unreachable!("expected Chat variant"),
    }
}

fn responses_name_of(bt: &BackendTools<'_>) -> String {
    match bt {
        BackendTools::Responses(v) => match &v[0] {
            rs::Tool::Function(f) => f.name.clone(),
            _ => unreachable!(),
        },
        _ => unreachable!("expected Responses variant"),
    }
}

fn anth_name_of(bt: &BackendTools<'_>) -> String {
    match bt {
        BackendTools::Anthropic(v) => v[0].name.clone(),
        _ => unreachable!("expected Anthropic variant"),
    }
}

#[test]
fn long_tool_name_is_capped_on_all_three_backends() {
    let long_name: String = "a".repeat(200);
    let tools = vec![spec(&long_name)];

    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &[]);
    let resp = normalize_tool_definitions_for(ApiBackend::Responses, &tools, &[]);
    let anth = normalize_tool_definitions_for(ApiBackend::Messages, &tools, &[]);

    let chat_name = chat_name_of(&chat);
    let resp_name = responses_name_of(&resp);
    let anth_name = anth_name_of(&anth);

    for n in [&chat_name, &resp_name, &anth_name] {
        assert!(n.len() <= TOOL_NAME_MAX_LEN, "name too long: {n:?}");
        // The cap suffix is a stable hash of the original; pin its shape so
        // the algorithm doesn't silently regress.
        assert!(
            n.ends_with(|c: char| c.is_ascii_hexdigit()),
            "no hash suffix: {n:?}"
        );
    }
}

#[test]
fn distinct_long_tool_names_remain_distinct_after_truncation() {
    let a: String = "alpha_".to_string() + &"x".repeat(100);
    let b: String = "beta_".to_string() + &"x".repeat(100);
    let tools = vec![spec(&a), spec(&b)];

    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &[]);
    let names: Vec<String> = match chat {
        BackendTools::Chat(v) => v.iter().map(|t| t.function.name.clone()).collect(),
        _ => unreachable!(),
    };
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1], "collision after truncation: {names:?}");
}

#[test]
fn unsafe_characters_in_tool_name_are_substituted() {
    let tools = vec![spec("a b/c?d")];
    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &[]);
    let name = chat_name_of(&chat);
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    );
}

#[test]
fn hosted_tool_collision_is_dropped_on_all_backends() {
    let tools = vec![spec("web_search"), spec("safe_tool")];
    let hosted = vec![HostedTool::WebSearch { options: None }];

    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &hosted);
    let resp = normalize_tool_definitions_for(ApiBackend::Responses, &tools, &hosted);
    let anth = normalize_tool_definitions_for(ApiBackend::Messages, &tools, &hosted);

    let chat_len = match chat {
        BackendTools::Chat(v) => v.len(),
        _ => unreachable!(),
    };
    let resp_len = match resp {
        BackendTools::Responses(v) => v.len(),
        _ => unreachable!(),
    };
    let anth_len = match anth {
        BackendTools::Anthropic(v) => v.len(),
        _ => unreachable!(),
    };
    assert_eq!(chat_len, 1);
    assert_eq!(resp_len, 1);
    assert_eq!(anth_len, 1);
}

#[test]
fn tool_choice_required_maps_correctly_per_backend() {
    let required = ConversationToolChoice::Required;

    let chat = normalize_tool_choice_for(ApiBackend::ChatCompletions, required.clone());
    let resp = normalize_tool_choice_for(ApiBackend::Responses, required.clone());
    let anth = normalize_tool_choice_for(ApiBackend::Messages, required);

    match chat {
        BackendToolChoice::Chat(ToolChoice::Preset(s)) => assert_eq!(s, "required"),
        other => panic!("chat shape wrong: {other:?}"),
    }
    match resp {
        BackendToolChoice::Responses(rs::ToolChoiceParam::Mode(
            rs::ToolChoiceOptions::Required,
        )) => {}
        other => panic!("responses shape wrong: {other:?}"),
    }
    match anth {
        BackendToolChoice::Anthropic(ToolChoiceParam::Any) => {}
        other => panic!("anthropic shape wrong: {other:?}"),
    }
}

#[test]
fn tool_choice_function_picks_correct_backend_variant() {
    let f = ConversationToolChoice::Function("foo".to_string());

    let chat = normalize_tool_choice_for(ApiBackend::ChatCompletions, f.clone());
    let resp = normalize_tool_choice_for(ApiBackend::Responses, f.clone());
    let anth = normalize_tool_choice_for(ApiBackend::Messages, f);

    match chat {
        BackendToolChoice::Chat(ToolChoice::Function {
            function: ToolChoiceFunction { name },
            ..
        }) => assert_eq!(name, "foo"),
        other => panic!("chat shape wrong: {other:?}"),
    }
    match resp {
        BackendToolChoice::Responses(rs::ToolChoiceParam::Function(rs::ToolChoiceFunction {
            name,
        })) => assert_eq!(name, "foo"),
        other => panic!("responses shape wrong: {other:?}"),
    }
    match anth {
        BackendToolChoice::Anthropic(ToolChoiceParam::Tool { name }) => assert_eq!(name, "foo"),
        other => panic!("anthropic shape wrong: {other:?}"),
    }
}

#[test]
fn tool_choice_function_name_also_gets_sanitized() {
    let f = ConversationToolChoice::Function("with spaces and / chars".to_string());
    let chat = normalize_tool_choice_for(ApiBackend::ChatCompletions, f);
    match chat {
        BackendToolChoice::Chat(ToolChoice::Function {
            function: ToolChoiceFunction { name },
            ..
        }) => {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            )
        }
        other => panic!("chat shape wrong: {other:?}"),
    }
}

#[test]
fn arguments_invalid_json_yields_null_not_panic() {
    let v = normalize_tool_call_arguments("not json at all");
    assert_eq!(v, Value::Null);
}

#[test]
fn arguments_empty_string_yields_null() {
    assert_eq!(normalize_tool_call_arguments(""), Value::Null);
}

#[test]
fn arguments_object_round_trips() {
    let raw = r#"{"x":1,"y":"two"}"#;
    let v = normalize_tool_call_arguments(raw);
    assert_eq!(v, json!({"x": 1, "y": "two"}));
}

#[test]
fn tool_call_id_anthropic_prefix_is_preserved() {
    let id = "toolu_01HFAKE00000000000000000";
    let out = sanitize_tool_call_id(id);
    assert_eq!(out, id);
}

#[test]
fn tool_call_id_openai_prefix_is_preserved() {
    let id = "call_abc123def456";
    let out = sanitize_tool_call_id(id);
    assert_eq!(out, id);
}

#[test]
fn tool_call_id_unsafe_chars_are_substituted() {
    let id = "toolu_abc def!";
    let out = sanitize_tool_call_id(id);
    assert!(
        out.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    );
}

#[test]
fn tool_call_id_long_is_capped_with_distinct_suffix() {
    let a: String = "toolu_".to_string() + &"a".repeat(100);
    let b: String = "toolu_".to_string() + &"b".repeat(100);
    let sa = sanitize_tool_call_id(&a);
    let sb = sanitize_tool_call_id(&b);
    assert_ne!(sa, sb);
    assert!(sa.len() <= TOOL_CALL_ID_MAX_LEN);
    assert!(sb.len() <= TOOL_CALL_ID_MAX_LEN);
}

#[test]
fn empty_tool_name_becomes_underscore() {
    let tools = vec![spec("")];
    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &[]);
    let name = chat_name_of(&chat);
    assert_eq!(name, "_");
}

#[test]
fn schema_with_additional_properties_false_is_preserved() {
    // The strict-mode schema travels byte-identical to every backend so the
    // model sees the same constraints regardless of provider.
    let tools = vec![spec("foo")];
    let chat = normalize_tool_definitions_for(ApiBackend::ChatCompletions, &tools, &[]);
    let anth = normalize_tool_definitions_for(ApiBackend::Messages, &tools, &[]);

    let chat_params = match chat {
        BackendTools::Chat(v) => v[0].function.parameters.clone(),
        _ => unreachable!(),
    };
    let anth_schema = match anth {
        BackendTools::Anthropic(v) => v[0].input_schema.clone(),
        _ => unreachable!(),
    };
    assert_eq!(
        chat_params.get("additionalProperties"),
        Some(&json!(false)),
        "additionalProperties must round-trip through Chat"
    );
    assert_eq!(
        anth_schema.get("additionalProperties"),
        Some(&json!(false)),
        "additionalProperties must round-trip through Anthropic"
    );
}

#[test]
fn tool_use_input_is_a_value_not_a_string() {
    // Anthropic's `tool_use.input` is a parsed JSON Value, while OpenAI's
    // `tool_calls[i].function.arguments` is a JSON-encoded String. Verify the
    // wire shape difference is preserved here, so downstream callers can
    // rely on `input` already being deserialized.
    let anth = ToolParam {
        name: "do_thing".into(),
        description: Some("do a thing".into()),
        input_schema: json!({"type": "object", "properties": {"x": {"type": "integer"}}}),
    };
    let block: ContentBlock = serde_json::from_value(json!({
        "type": "tool_use",
        "id": "toolu_1",
        "name": anth.name,
        "input": anth.input_schema,
    }))
    .expect("fixture deserializes");
    match block {
        ContentBlock::ToolUse { input, .. } => {
            assert_eq!(
                input,
                json!({"type": "object", "properties": {"x": {"type": "integer"}}})
            );
        }
        other => panic!("expected tool_use, got {other:?}"),
    }
}

#[test]
fn tool_result_content_text_round_trips() {
    let id = sanitize_tool_call_id("toolu_xyz");
    let block: ContentBlock = serde_json::from_value(json!({
        "type": "tool_result",
        "tool_use_id": id,
        "content": "ok",
    }))
    .expect("fixture deserializes");
    match block {
        ContentBlock::ToolResult {
            tool_use_id,
            content: ToolResultContent::Text(text),
            ..
        } => {
            assert_eq!(tool_use_id, "toolu_xyz");
            assert_eq!(text, "ok");
        }
        other => panic!("expected tool_result, got {other:?}"),
    }
}
