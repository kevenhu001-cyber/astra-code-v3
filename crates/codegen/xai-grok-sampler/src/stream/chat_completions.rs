//! Layer-2 stream transform for the Chat Completions API.
//!
//! Consumes a raw `ChatCompletionChunk` stream and produces
//! [`SamplingEvent`]s. Pure: no I/O, no shell coupling.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::{BoxStream, Stream};

use xai_grok_sampling_types::{
    AssistantItem, ChatCompletionChunk, ConversationItem, ConversationResponse,
    ResponseModelMetadata, SamplingError, StopReason, TokenUsage, ToolCall,
};

use crate::events::{SamplingChannel, SamplingErrorInfo, SamplingEvent};
use crate::metrics::InferenceLatencyStats;
use crate::types::RequestId;

/// Transform a raw Chat Completions chunk stream into a stream of
/// [`SamplingEvent`]s.
///
/// The output stream emits exactly one terminal event per request:
/// [`SamplingEvent::Completed`] on normal stream end, or
/// [`SamplingEvent::Failed`] on error / idle timeout. Callers must not
/// consume past the terminal event (the implementation `return`s after
/// yielding it).
///
/// `idle_timeout` covers two cases:
/// 1. The transport stops yielding chunks at all (`tokio::time::timeout`).
/// 2. The transport keeps yielding empty / keepalive chunks but no
///    meaningful content (separate `last_content_chunk_at` timer).
///
/// Both produce `SamplingEvent::Failed { kind: IdleTimeout }`.
pub fn stream_chat_completions<'a>(
    raw_stream: BoxStream<'a, Result<ChatCompletionChunk, SamplingError>>,
    model_metadata: Option<ResponseModelMetadata>,
    request_id: RequestId,
    idle_timeout: Duration,
    // When true, scan `delta.content` for `<think>…</think>` tags and route
    // inside-tag text to the reasoning channel while outside-tag text goes to
    // the regular text channel. Used for OpenAI-compatible providers
    // (DeepSeek, Qwen, Zhipu, zAI, …) that don't expose a native
    // `reasoning_content` delta and instead embed thinking as `<think>` tags
    // in the content stream. When false, the splitter is a no-op passthrough.
    injects_think_tags_in_content: bool,
) -> impl Stream<Item = SamplingEvent> + Send + 'a {
    async_stream::stream! {
        let stream_start = Instant::now();
        let mut chunk_timestamps: Vec<Instant> = Vec::new();

        // Emit StreamStarted before reading any chunks so subscribers
        // can record TTFB / TTLB baselines.
        yield SamplingEvent::StreamStarted {
            request_id: request_id.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        };

        if let Some(metadata) = model_metadata {
            yield SamplingEvent::ModelMetadata {
                request_id: request_id.clone(),
                metadata,
            };
        }

        // Per-response accumulators
        let mut first_chunk_seen = false;
        let mut first_choice_seen = false;
        let mut first_token_emitted = false;
        let mut model: String = String::new();
        let mut model_fingerprint: Option<String> = None;
        let mut usage: Option<TokenUsage> = None;
        let mut cost_usd_ticks: Option<i64> = None;
        let mut finish_reason: Option<StopReason> = None;

        let mut content_acc = String::new();
        let mut reasoning_acc = String::new();
        // Tool call deltas keyed by positional index. Each entry is
        // (id, name, arguments_buffer); the first chunk for an index
        // carries id+name and starts the arguments buffer, subsequent
        // chunks append to arguments only.
        let mut tool_call_acc: BTreeMap<u32, (String, String, String)> = BTreeMap::new();

        // Index counter spanning text + reasoning chunks (matches the
        // shell's chunk_index used for notification correlation).
        let mut chunk_index: u64 = 0;
        // Separate counter for AgentMessageChunk (text-only) emissions;
        // mirrored onto ConversationResponse.message_chunks_emitted so
        // downstream can detect lost-streaming-events scenarios.
        let mut message_chunk_count: u64 = 0;

        // Stateful `<think>` splitter: tracks whether we're currently inside a
        // thinking block. Content deltas are incrementally parsed so a tag can
        // straddle two chunks.
        let mut in_think = false;
        // Pending buffer holding a partial (not-yet-closed) `<think>`/`<\/think>`
        // tag opener across chunk boundaries.
        let mut tag_buf = String::new();

        // Content-aware idle timer: the outer
        // `tokio::time::timeout(idle_timeout, stream.next())` already
        // catches "transport stops yielding chunks". This second timer
        // catches the more subtle case where the model keeps emitting
        // keepalive / empty-delta SSE events that satisfy the outer
        // timer but make no real progress -- some inference engines
        // do exactly that.
        let mut last_content_chunk_at = Instant::now();

        let mut stream = raw_stream;
        loop {
            let next = match tokio::time::timeout(idle_timeout, stream.next()).await {
                Ok(Some(next)) => next,
                Ok(None) => break, // stream ended normally
                Err(_elapsed) => {
                    let err = SamplingError::IdleTimeout {
                        elapsed_secs: idle_timeout.as_secs(),
                    };
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
            };
            let chunk = match next {
                Ok(chunk) => chunk,
                Err(err) => {
                    yield SamplingEvent::Failed {
                        request_id: request_id.clone(),
                        error: SamplingErrorInfo::from(&err),
                    };
                    return;
                }
            };

            if !first_chunk_seen {
                model = chunk.model.clone();
                model_fingerprint = chunk
                    .system_fingerprint
                    .clone()
                    .filter(|s| !s.is_empty());
                first_chunk_seen = true;
            }

            if let Some(u) = chunk.usage.clone() {
                // Wire cost is cumulative for the response, so last-write-wins.
                // Never clobber a known cost with missing/unreported.
                let chunk_cost = xai_grok_sampling_types::reported_cost_ticks(u.cost_in_usd_ticks);
                cost_usd_ticks = match (cost_usd_ticks, chunk_cost) {
                    (_, Some(n)) => Some(n),
                    (prev, None) => prev,
                };
                usage = Some(u.into());
            }

            // Track whether this chunk carried meaningful content.
            // Set inside the choices loop and checked at the end.
            let mut chunk_has_content = false;

            for choice in chunk.choices.into_iter() {
                first_choice_seen = true;
                if let Some(fr) = choice.finish_reason {
                    finish_reason = Some(fr.into());
                    chunk_has_content = true;
                }

                let delta = choice.delta;

                if let Some(text) = delta.content
                    && !text.is_empty()
                {
                    if injects_think_tags_in_content {
                        // Split content into reasoning (inside `<think>…</think>`)
                        // vs regular text, emitting a channel token for each run.
                        for run in split_think_runs(&text, &mut in_think, &mut tag_buf) {
                            let (channel, payload) = match &run {
                                ThinkRun::Reasoning(t) => (SamplingChannel::Reasoning, t),
                                ThinkRun::Text(t) => (SamplingChannel::Text, t),
                            };
                            if !first_token_emitted {
                                first_token_emitted = true;
                                yield SamplingEvent::FirstToken {
                                    request_id: request_id.clone(),
                                };
                            }
                            chunk_has_content = true;
                            chunk_timestamps.push(Instant::now());
                            chunk_index += 1;
                            message_chunk_count += 1;
                            match &run {
                                ThinkRun::Reasoning(t) => reasoning_acc.push_str(t),
                                ThinkRun::Text(t) => content_acc.push_str(t),
                            }
                            yield SamplingEvent::ChannelToken {
                                request_id: request_id.clone(),
                                channel,
                                text: payload.clone(),
                                chunk_index,
                            };
                        }
                    } else {
                        // Plain text path (no think-tag splitter). Emit
                        // FirstToken on the first content chunk; emit a
                        // ChannelToken for EVERY content chunk so the
                        // downstream sees the full text, not just the
                        // first one. The earlier `else if !first_token_emitted`
                        // guard wrongly dropped every chunk past the first.
                        if !first_token_emitted {
                            first_token_emitted = true;
                            yield SamplingEvent::FirstToken {
                                request_id: request_id.clone(),
                            };
                        }
                        chunk_has_content = true;
                        chunk_timestamps.push(Instant::now());
                        chunk_index += 1;
                        message_chunk_count += 1;
                        content_acc.push_str(&text);
                        yield SamplingEvent::ChannelToken {
                            request_id: request_id.clone(),
                            channel: SamplingChannel::Text,
                            text,
                            chunk_index,
                        };
                    }
                }

                if let Some(thought) = delta.reasoning_content
                    && !thought.is_empty()
                {
                    if !first_token_emitted {
                        first_token_emitted = true;
                        yield SamplingEvent::FirstToken {
                            request_id: request_id.clone(),
                        };
                    }
                    chunk_has_content = true;
                    chunk_index += 1;
                    reasoning_acc.push_str(&thought);
                    yield SamplingEvent::ChannelToken {
                        request_id: request_id.clone(),
                        channel: SamplingChannel::Reasoning,
                        text: thought,
                        chunk_index,
                    };
                }

                for tc_delta in delta.tool_calls.into_iter() {
                    chunk_has_content = true;

                    let entry = tool_call_acc
                        .entry(tc_delta.index)
                        .or_insert_with(|| (String::new(), String::new(), String::new()));

                    let mut id_for_event: Option<String> = None;
                    let mut name_for_event: Option<String> = None;
                    let mut args_for_event: Option<String> = None;

                    if let Some(id) = tc_delta.id {
                        entry.0 = id.clone();
                        id_for_event = Some(id);
                    }
                    if let Some(func) = tc_delta.function {
                        if let Some(name) = func.name {
                            entry.1 = name.clone();
                            name_for_event = Some(name);
                        }
                        if let Some(args) = func.arguments {
                            entry.2.push_str(&args);
                            args_for_event = Some(args);
                        }
                    }

                    yield SamplingEvent::ToolCallDelta {
                        request_id: request_id.clone(),
                        tool_index: tc_delta.index,
                        id: id_for_event,
                        name: name_for_event,
                        arguments_delta: args_for_event,
                    };
                }
            }

            if chunk_has_content {
                last_content_chunk_at = Instant::now();
            } else if last_content_chunk_at.elapsed() > idle_timeout {
                let err = SamplingError::IdleTimeout {
                    elapsed_secs: idle_timeout.as_secs(),
                };
                yield SamplingEvent::Failed {
                    request_id: request_id.clone(),
                    error: SamplingErrorInfo::from(&err),
                };
                return;
            }
        }

        // ── Build the final response ─────────────────────────────────
        let tool_calls: Vec<ToolCall> = tool_call_acc
            .into_values()
            .map(|(id, name, arguments)| ToolCall {
                id: std::sync::Arc::<str>::from(id),
                name,
                arguments: std::sync::Arc::<str>::from(arguments),
            })
            .collect();

        // Honor tool calls by overriding the stop reason if the model
        // forgot to set it (mirrors the shell's behavior).
        if !tool_calls.is_empty() {
            finish_reason = Some(StopReason::ToolCalls);
        }

        // Build the trailing Assistant + any reasoning sibling.
        let mut items: Vec<ConversationItem> = Vec::new();
        if first_choice_seen {
            if !reasoning_acc.is_empty() {
                items.push(ConversationItem::Reasoning(
                    xai_grok_sampling_types::synthesized_reasoning_item(reasoning_acc),
                ));
            }
            items.push(ConversationItem::Assistant(AssistantItem {
                content: std::sync::Arc::<str>::from(content_acc),
                tool_calls,
                model_id: Some(model),
                model_fingerprint,
                // Chat Completions does not echo the applied reasoning effort.
                reasoning_effort: None,
            }));
        } else {
            items.push(ConversationItem::assistant(""));
        }

        let stream_end = Instant::now();
        let metrics =
            InferenceLatencyStats::from_timestamps(stream_start, &chunk_timestamps, stream_end);

        let response = ConversationResponse {
            items,
            stop_reason: finish_reason,
            usage,
            cost_usd_ticks,
            message_chunks_emitted: message_chunk_count,
            doom_loop_signals: Vec::new(),
            stop_message: None,
            message_id: None,
            raw_stop_reason: None,
            stop_sequence: None,
        };

        yield SamplingEvent::Completed {
            request_id: request_id.clone(),
            response: Box::new(response),
            metrics,
        };
    }
}

/// A contiguous run of content that belongs to either the reasoning or the
/// regular text channel, after `<think>…</think>` tag extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ThinkRun {
    Reasoning(String),
    Text(String),
}

const OPEN_TAG: &str = "<think>";
const CLOSE_TAG: &str = "</think>";

/// Parse a chunk of `delta.content` into reasoning/text runs, honoring tags
/// that may straddle chunk boundaries. `in_think` and `tag_buf` carry the
/// splitter state across successive calls.
///
/// The model emits the literal tags `<think>` and `</think>`. A partial opener
/// (e.g. `<thi` at a chunk edge) is buffered in `tag_buf` and only flushed
/// once it is proven not to be a tag.
fn split_think_runs(chunk: &str, in_think: &mut bool, tag_buf: &mut String) -> Vec<ThinkRun> {
    let mut runs: Vec<ThinkRun> = Vec::new();
    let mut cur = String::new();

    fn flush(cur: &mut String, in_think: bool, runs: &mut Vec<ThinkRun>) {
        if !cur.is_empty() {
            runs.push(if in_think {
                ThinkRun::Reasoning(std::mem::take(cur))
            } else {
                ThinkRun::Text(std::mem::take(cur))
            });
        }
    }

    // Try to complete any partial tag buffered from a previous chunk against
    // the start of this chunk before scanning fresh content. `tag_buf` holds
    // whole chars, so the candidate stays a valid UTF-8 prefix.
    let mut chars = chunk.char_indices().peekable();
    if !tag_buf.is_empty() {
        while let Some((_, c)) = chars.peek() {
            let candidate = format!("{tag_buf}{c}");
            if OPEN_TAG.starts_with(candidate.as_str()) || CLOSE_TAG.starts_with(candidate.as_str())
            {
                tag_buf.push(*c);
                chars.next();
                if *tag_buf == OPEN_TAG {
                    flush(&mut cur, *in_think, &mut runs);
                    *in_think = true;
                    tag_buf.clear();
                    break;
                }
                if *tag_buf == CLOSE_TAG {
                    flush(&mut cur, *in_think, &mut runs);
                    *in_think = false;
                    tag_buf.clear();
                    break;
                }
            } else {
                break;
            }
        }
        // Buffered chars that can no longer form a tag become plain content.
        if !tag_buf.is_empty()
            && !OPEN_TAG.starts_with(tag_buf.as_str())
            && !CLOSE_TAG.starts_with(tag_buf.as_str())
        {
            cur.push_str(tag_buf);
            flush(&mut cur, *in_think, &mut runs);
            tag_buf.clear();
        }
    }

    // Main scan over the remaining chars, honouring char (not byte) boundaries
    // so multi-byte UTF-8 content is preserved verbatim.
    for (_, c) in chars {
        // Extend a pending partial tag (or start one on '<').
        if !tag_buf.is_empty() || c == '<' {
            let candidate = format!("{tag_buf}{c}");
            if OPEN_TAG.starts_with(candidate.as_str()) || CLOSE_TAG.starts_with(candidate.as_str())
            {
                tag_buf.push(c);
                if *tag_buf == OPEN_TAG {
                    flush(&mut cur, *in_think, &mut runs);
                    *in_think = true;
                    tag_buf.clear();
                } else if *tag_buf == CLOSE_TAG {
                    flush(&mut cur, *in_think, &mut runs);
                    *in_think = false;
                    tag_buf.clear();
                }
                continue;
            }
            // Not a tag: emit the buffered partial chars as content first.
            cur.push_str(tag_buf);
            flush(&mut cur, *in_think, &mut runs);
            tag_buf.clear();
        }
        cur.push(c);
    }

    // Any trailing partial tag stays buffered for the next chunk.
    flush(&mut cur, *in_think, &mut runs);
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use std::pin::pin;
    use xai_grok_sampling_types::{
        ChatChunkChoice, ChatChunkDelta, FinishReason, Role, ToolCallDelta as ChunkToolCallDelta,
        ToolCallFunctionDelta, Usage, rs,
    };

    fn rid() -> RequestId {
        RequestId::from("test-req")
    }

    fn make_chunk(deltas: Vec<ChatChunkDelta>) -> ChatCompletionChunk {
        ChatCompletionChunk {
            id: "chunk-1".into(),
            object: "chat.completion.chunk".into(),
            created: 0,
            model: "test-model".into(),
            choices: deltas
                .into_iter()
                .enumerate()
                .map(|(i, delta)| ChatChunkChoice {
                    index: i as u32,
                    delta,
                    finish_reason: None,
                })
                .collect(),
            usage: None,
            system_fingerprint: None,
        }
    }

    fn text_chunk(text: &str) -> ChatCompletionChunk {
        make_chunk(vec![ChatChunkDelta {
            role: Some(Role::Assistant),
            content: Some(text.to_string()),
            reasoning_content: None,
            tool_calls: vec![],
            tool_call_id: None,
        }])
    }

    fn final_chunk(reason: FinishReason) -> ChatCompletionChunk {
        let mut chunk = make_chunk(vec![ChatChunkDelta::default()]);
        chunk.choices[0].finish_reason = Some(reason);
        chunk
    }

    async fn collect(s: impl Stream<Item = SamplingEvent>) -> Vec<SamplingEvent> {
        let mut out = Vec::new();
        let mut s = pin!(s);
        while let Some(ev) = s.next().await {
            out.push(ev);
        }
        out
    }

    #[tokio::test]
    async fn empty_stream_yields_started_then_completed() {
        let raw = stream::iter(Vec::<Result<ChatCompletionChunk, SamplingError>>::new()).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        match &events[1] {
            SamplingEvent::Completed { response, .. } => {
                assert!(response.is_empty());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn text_only_stream_emits_first_token_then_channel_tokens_then_completed() {
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("Hello, ")),
            Ok(text_chunk("world!")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        // Expected sequence: StreamStarted, FirstToken, ChannelToken(Text)
        // x 2, Completed.
        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        assert!(matches!(events[1], SamplingEvent::FirstToken { .. }));

        let text_tokens: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                SamplingEvent::ChannelToken {
                    channel: SamplingChannel::Text,
                    text,
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_tokens, vec!["Hello, ", "world!"]);

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let a = response.assistant().expect("assistant item present");
                assert_eq!(a.content.as_ref(), "Hello, world!");
                assert_eq!(response.stop_reason, Some(StopReason::Stop));
                assert_eq!(response.message_chunks_emitted, 2);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reasoning_chunk_emits_reasoning_channel_and_first_token_once() {
        let mut reasoning_chunk = make_chunk(vec![ChatChunkDelta {
            role: Some(Role::Assistant),
            content: None,
            reasoning_content: Some("thinking...".into()),
            tool_calls: vec![],
            tool_call_id: None,
        }]);
        reasoning_chunk.choices[0].finish_reason = None;

        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(reasoning_chunk),
            Ok(text_chunk("done")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        // FirstToken should appear exactly once.
        let first_token_count = events
            .iter()
            .filter(|e| matches!(e, SamplingEvent::FirstToken { .. }))
            .count();
        assert_eq!(first_token_count, 1);

        let mut saw_reasoning = false;
        let mut saw_text = false;
        for e in &events {
            if let SamplingEvent::ChannelToken { channel, text, .. } = e {
                match channel {
                    SamplingChannel::Reasoning => {
                        assert_eq!(text, "thinking...");
                        saw_reasoning = true;
                    }
                    SamplingChannel::Text => {
                        assert_eq!(text, "done");
                        saw_text = true;
                    }
                }
            }
        }
        assert!(saw_reasoning && saw_text);

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let r = response
                    .reasoning_items()
                    .next()
                    .expect("reasoning sibling preserved");
                let rs::SummaryPart::SummaryText(t) = &r.summary[0];
                assert_eq!(t.text, "thinking...");
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_call_stream_emits_deltas_and_assembles_final_call() {
        // First chunk has id + name + part of arguments.
        let chunk1 = make_chunk(vec![ChatChunkDelta {
            role: None,
            content: None,
            reasoning_content: None,
            tool_calls: vec![ChunkToolCallDelta {
                index: 0,
                id: Some("call_abc".into()),
                kind: Some("function".into()),
                function: Some(ToolCallFunctionDelta {
                    name: Some("do_thing".into()),
                    arguments: Some("{\"x\":".into()),
                }),
            }],
            tool_call_id: None,
        }]);
        // Second chunk has only argument fragment.
        let chunk2 = make_chunk(vec![ChatChunkDelta {
            role: None,
            content: None,
            reasoning_content: None,
            tool_calls: vec![ChunkToolCallDelta {
                index: 0,
                id: None,
                kind: None,
                function: Some(ToolCallFunctionDelta {
                    name: None,
                    arguments: Some("1}".into()),
                }),
            }],
            tool_call_id: None,
        }]);

        let raw = stream::iter::<Vec<Result<ChatCompletionChunk, SamplingError>>>(vec![
            Ok(chunk1),
            Ok(chunk2),
        ])
        .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        let deltas: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                SamplingEvent::ToolCallDelta {
                    tool_index,
                    id,
                    name,
                    arguments_delta,
                    ..
                } => Some((
                    *tool_index,
                    id.clone(),
                    name.clone(),
                    arguments_delta.clone(),
                )),
                _ => None,
            })
            .collect();

        assert_eq!(deltas.len(), 2);
        assert_eq!(deltas[0].0, 0);
        assert_eq!(deltas[0].1.as_deref(), Some("call_abc"));
        assert_eq!(deltas[0].2.as_deref(), Some("do_thing"));
        assert_eq!(deltas[0].3.as_deref(), Some("{\"x\":"));
        assert_eq!(deltas[1].1, None);
        assert_eq!(deltas[1].2, None);
        assert_eq!(deltas[1].3.as_deref(), Some("1}"));

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let calls = response.tool_calls();
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id.as_ref(), "call_abc");
                assert_eq!(calls[0].name, "do_thing");
                assert_eq!(calls[0].arguments.as_ref(), "{\"x\":1}");
                // Tool calls force ToolCalls stop reason.
                assert_eq!(response.stop_reason, Some(StopReason::ToolCalls));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mid_stream_error_yields_failed_no_completed() {
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("hi")),
            Err(SamplingError::EventStreamError("conn reset".into())),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        assert!(
            events
                .iter()
                .any(|e| matches!(e, SamplingEvent::Failed { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, SamplingEvent::Completed { .. }))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_when_stream_stalls() {
        // A stream that yields one chunk then hangs forever.
        let raw = stream::iter(vec![Ok(text_chunk("hello"))])
            .chain(stream::pending())
            .boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_millis(100),
            false,
        ))
        .await;

        // Stream should emit StreamStarted, FirstToken, ChannelToken
        // then Failed(IdleTimeout) when the stall hits the deadline.
        match events.last().unwrap() {
            SamplingEvent::Failed { error, .. } => {
                assert_eq!(error.kind, crate::events::SamplingErrorKind::IdleTimeout);
            }
            other => panic!("expected Failed(IdleTimeout), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn model_metadata_yielded_after_stream_started() {
        let raw = stream::iter(Vec::<Result<ChatCompletionChunk, SamplingError>>::new()).boxed();
        let metadata = ResponseModelMetadata {
            context_window: Some(8192),
            max_completion_tokens: Some(4096),
            models_etag: None,
        };
        let events = collect(stream_chat_completions(
            raw,
            Some(metadata.clone()),
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        assert!(matches!(events[0], SamplingEvent::StreamStarted { .. }));
        match &events[1] {
            SamplingEvent::ModelMetadata { metadata: m, .. } => {
                assert_eq!(m.context_window, Some(8192));
                assert_eq!(m.max_completion_tokens, Some(4096));
            }
            other => panic!("expected ModelMetadata second, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn usage_is_extracted_from_chunk() {
        let mut chunk_with_usage = make_chunk(vec![ChatChunkDelta::default()]);
        chunk_with_usage.usage = Some(Usage {
            prompt_tokens: 100,
            completion_tokens: 50,
            total_tokens: 150,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: None,
        });

        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("ok")),
            Ok(chunk_with_usage),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let u = response.usage.as_ref().expect("usage extracted");
                assert_eq!(u.prompt_tokens, 100);
                assert_eq!(u.completion_tokens, 50);
                assert_eq!(u.total_tokens, 150);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    /// Server-reported cost lands on the response; the REST mapper's `0`
    /// backfill means "unreported" and must yield `None`.
    #[tokio::test]
    async fn cost_is_extracted_and_zero_is_unreported() {
        for (wire, expected) in [(Some(78), Some(78)), (Some(0), None), (None, None)] {
            let mut chunk_with_usage = make_chunk(vec![ChatChunkDelta::default()]);
            chunk_with_usage.usage = Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                prompt_tokens_details: None,
                completion_tokens_details: None,
                cost_in_usd_ticks: wire,
            });
            let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
                Ok(text_chunk("ok")),
                Ok(chunk_with_usage),
                Ok(final_chunk(FinishReason::Stop)),
            ];
            let raw = stream::iter(chunks).boxed();
            let events = collect(stream_chat_completions(
                raw,
                None,
                rid(),
                Duration::from_secs(60),
                false,
            ))
            .await;
            match events.last().unwrap() {
                SamplingEvent::Completed { response, .. } => {
                    assert_eq!(response.cost_usd_ticks, expected, "wire {wire:?}");
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn think_tags_in_content_route_to_reasoning_channel() {
        // A single content delta carrying `<think>…</think>` must split into a
        // reasoning run and a trailing text run, with the tags themselves
        // stripped from the final content + reasoning accumulators.
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk(
                "<think>Let me reason about this.</think>Final answer.",
            )),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            true,
        ))
        .await;

        let mut saw_reasoning = false;
        let mut saw_text = false;
        for e in &events {
            if let SamplingEvent::ChannelToken { channel, text, .. } = e {
                match channel {
                    SamplingChannel::Reasoning => {
                        assert_eq!(text, "Let me reason about this.");
                        saw_reasoning = true;
                    }
                    SamplingChannel::Text => {
                        assert_eq!(text, "Final answer.");
                        saw_text = true;
                    }
                }
            }
        }
        assert!(saw_reasoning && saw_text);

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let a = response.assistant().expect("assistant item present");
                assert_eq!(a.content.as_ref(), "Final answer.");
                let r = response
                    .reasoning_items()
                    .next()
                    .expect("reasoning sibling preserved");
                let rs::SummaryPart::SummaryText(t) = &r.summary[0];
                assert_eq!(t.text, "Let me reason about this.");
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn think_tags_split_across_chunks() {
        // The `<think>` opener and `</think>` closer straddle chunk boundaries;
        // the splitter must still extract the reasoning cleanly.
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("pre <thi")),
            Ok(text_chunk("nk>mid thought<")),
            Ok(text_chunk("/thin")),
            Ok(text_chunk("k>post")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            true,
        ))
        .await;

        let mut reasoning = String::new();
        let mut text = String::new();
        for e in &events {
            if let SamplingEvent::ChannelToken {
                channel, text: t, ..
            } = e
            {
                match channel {
                    SamplingChannel::Reasoning => reasoning.push_str(t),
                    SamplingChannel::Text => text.push_str(t),
                }
            }
        }
        assert_eq!(reasoning, "mid thought");
        assert_eq!(text, "pre post");
    }

    #[tokio::test]
    async fn think_tags_disabled_passthrough_keeps_tags_in_text() {
        // When the flag is off, content is emitted verbatim (no splitting).
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("<think>nope</think>text")),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;

        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                let a = response.assistant().expect("assistant item present");
                assert_eq!(a.content.as_ref(), "<think>nope</think>text");
                assert!(response.reasoning_items().next().is_none());
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn split_think_runs_unit_basic() {
        let mut in_think = false;
        let mut tag_buf = String::new();
        let runs = split_think_runs("a<think>b</think>c", &mut in_think, &mut tag_buf);
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("a".into()),
                ThinkRun::Reasoning("b".into()),
                ThinkRun::Text("c".into()),
            ]
        );
        assert!(!in_think);
        assert!(tag_buf.is_empty());
    }

    #[test]
    fn split_think_runs_unit_unclosed_left_open() {
        // Unclosed think tag at EOF leaves trailing reasoning run + in_think.
        let mut in_think = false;
        let mut tag_buf = String::new();
        let runs = split_think_runs("x<think>still thinking", &mut in_think, &mut tag_buf);
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("x".into()),
                ThinkRun::Reasoning("still thinking".into())
            ]
        );
        assert!(in_think);
    }

    #[test]
    fn split_think_runs_preserves_multibyte_utf8() {
        // Regression: the byte-wise scanner mangled multi-byte UTF-8. Whole
        // chars must survive inside both reasoning and text runs.
        let mut in_think = false;
        let mut tag_buf = String::new();
        let runs = split_think_runs(
            "思考一下：<think>中文推理过程</think>好的，回答你。",
            &mut in_think,
            &mut tag_buf,
        );
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("思考一下：".into()),
                ThinkRun::Reasoning("中文推理过程".into()),
                ThinkRun::Text("好的，回答你。".into()),
            ]
        );
        assert!(!in_think);
        assert!(tag_buf.is_empty());
    }

    #[test]
    fn split_think_runs_multibyte_across_chunk_boundary() {
        // A multi-byte char may straddle two chunks; the buffered partial tag
        // path must not corrupt surrounding UTF-8 content.
        let mut in_think = false;
        let mut tag_buf = String::new();
        // First chunk ends mid-way through 思考 and with a partial opener.
        let first = split_think_runs("前段<thi", &mut in_think, &mut tag_buf);
        assert_eq!(first, vec![ThinkRun::Text("前段".into())]);
        assert!(tag_buf == "<thi");
        // Second chunk completes the tag, carries 思考+reasoning+closer+text.
        let second = split_think_runs("nk>思考过程</think>后段", &mut in_think, &mut tag_buf);
        assert_eq!(
            second,
            vec![
                ThinkRun::Reasoning("思考过程".into()),
                ThinkRun::Text("后段".into())
            ]
        );
        assert!(!in_think);
        assert!(tag_buf.is_empty());
    }

    #[tokio::test]
    async fn later_missing_cost_does_not_clobber_earlier_ticks() {
        let mut first = make_chunk(vec![ChatChunkDelta::default()]);
        first.usage = Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: Some(99),
        });
        let mut second = make_chunk(vec![ChatChunkDelta::default()]);
        second.usage = Some(Usage {
            prompt_tokens: 12,
            completion_tokens: 6,
            total_tokens: 18,
            prompt_tokens_details: None,
            completion_tokens_details: None,
            cost_in_usd_ticks: Some(0),
        });
        let chunks: Vec<Result<ChatCompletionChunk, SamplingError>> = vec![
            Ok(text_chunk("ok")),
            Ok(first),
            Ok(second),
            Ok(final_chunk(FinishReason::Stop)),
        ];
        let raw = stream::iter(chunks).boxed();
        let events = collect(stream_chat_completions(
            raw,
            None,
            rid(),
            Duration::from_secs(60),
            false,
        ))
        .await;
        match events.last().unwrap() {
            SamplingEvent::Completed { response, .. } => {
                assert_eq!(response.cost_usd_ticks, Some(99));
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }
}
