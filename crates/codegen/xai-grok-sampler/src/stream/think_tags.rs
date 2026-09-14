//! Inline thinking-tag extraction shared by the three L2 stream transforms.
//!
//! Some OpenAI-compatible providers do not expose a native reasoning channel
//! and instead embed the model's chain of thought in `content` as literal
//! tags. The recognised tag pairs cover the observed provider variants:
//!
//! | Provider / runtime                | Tags                          |
//! |-----------------------------------|-------------------------------|
//! | MiniMax native / vLLM / Qwen      | `<think>` `</think>`          |
//! | MiniMax presets / proxies         | `<thinking>` `</thinking>`    |
//! | vLLM MiniMax M3 reasoning parser  | `<mm:think>` `</mm:think>`    |
//! | assorted proxies / self-hosted    | `<reasoning>`, `<thought>`    |
//!
//! Matching is ASCII-case-insensitive. Tags may straddle chunk boundaries and
//! a partial tag left at end-of-stream is flushed by [`ThinkTagSplitter::finish`]
//! so trailing literal text is never dropped from the transcript.

/// A contiguous run of content that belongs to either the reasoning or the
/// regular text channel, after tag extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThinkRun {
    Reasoning(String),
    Text(String),
}

/// `(tag, opens_thinking)` pairs. Longer tags that share a prefix with shorter
/// ones (e.g. `<think>` / `<thinking>`) are disambiguated by [`probe`] only
/// once the full tag has arrived.
const TAGS: &[(&str, bool)] = &[
    ("<think>", true),
    ("</think>", false),
    ("<thinking>", true),
    ("</thinking>", false),
    ("<reasoning>", true),
    ("</reasoning>", false),
    ("<thought>", true),
    ("</thought>", false),
    ("<mm:think>", true),
    ("</mm:think>", false),
];

enum TagProbe {
    /// The candidate is a complete tag; the bool is `opens_thinking`.
    Full(bool),
    /// The candidate can still become a tag with more chars.
    Prefix,
    /// The candidate can never be a tag.
    NoMatch,
}

/// Classify a candidate against the tag table, ASCII-case-insensitively.
/// `tag[..candidate.len()]` is safe because `tag` is ASCII and the length is
/// bounded by the check below; non-ASCII candidate bytes simply fail the
/// comparison.
fn probe(candidate: &str) -> TagProbe {
    let mut prefix = false;
    for (tag, opens) in TAGS {
        if candidate.len() > tag.len() {
            continue;
        }
        if tag[..candidate.len()].eq_ignore_ascii_case(candidate) {
            if candidate.len() == tag.len() {
                return TagProbe::Full(*opens);
            }
            prefix = true;
        }
    }
    if prefix {
        TagProbe::Prefix
    } else {
        TagProbe::NoMatch
    }
}

/// Stateful splitter for a single response stream. Feed every content delta to
/// [`split`](Self::split); after the underlying stream ends call
/// [`finish`](Self::finish) once to flush any buffered partial tag.
#[derive(Debug, Default)]
pub(crate) struct ThinkTagSplitter {
    in_think: bool,
    /// Pending buffer holding a partial (not-yet-classified) tag. Bounded by
    /// the longest tag in [`TAGS`].
    tag_buf: String,
}

impl ThinkTagSplitter {
    pub(crate) fn split(&mut self, chunk: &str) -> Vec<ThinkRun> {
        fn flush(cur: &mut String, in_think: bool, runs: &mut Vec<ThinkRun>) {
            if !cur.is_empty() {
                runs.push(if in_think {
                    ThinkRun::Reasoning(std::mem::take(cur))
                } else {
                    ThinkRun::Text(std::mem::take(cur))
                });
            }
        }

        let mut runs: Vec<ThinkRun> = Vec::new();
        let mut cur = String::new();

        for c in chunk.chars() {
            if self.tag_buf.is_empty() && c != '<' {
                cur.push(c);
                continue;
            }
            self.tag_buf.push(c);
            match probe(&self.tag_buf) {
                TagProbe::Full(opens) => {
                    flush(&mut cur, self.in_think, &mut runs);
                    self.in_think = opens;
                    self.tag_buf.clear();
                }
                TagProbe::Prefix => {}
                TagProbe::NoMatch => {
                    // The buffered chars are literal text; the char that broke
                    // the match may itself start a new tag.
                    let literal = std::mem::take(&mut self.tag_buf);
                    cur.push_str(&literal);
                    if c == '<' {
                        self.tag_buf.push('<');
                    }
                }
            }
        }

        flush(&mut cur, self.in_think, &mut runs);
        runs
    }

    /// Flush a partial tag left at end-of-stream. The buffered chars never
    /// formed a tag, so they are literal text routed by the current state.
    pub(crate) fn finish(&mut self) -> Vec<ThinkRun> {
        let mut runs = Vec::new();
        if !self.tag_buf.is_empty() {
            let literal = std::mem::take(&mut self.tag_buf);
            runs.push(if self.in_think {
                ThinkRun::Reasoning(literal)
            } else {
                ThinkRun::Text(literal)
            });
        }
        runs
    }
}

/// Split a complete text into `(visible_text, reasoning_text)`.
/// Used by final-assembly paths that only have the accumulated string.
pub(crate) fn split_think_tags(text: &str) -> (String, String) {
    let mut splitter = ThinkTagSplitter::default();
    let mut runs = splitter.split(text);
    runs.extend(splitter.finish());

    let mut visible = String::new();
    let mut reasoning = String::new();
    for run in runs {
        match run {
            ThinkRun::Text(t) => visible.push_str(&t),
            ThinkRun::Reasoning(t) => reasoning.push_str(&t),
        }
    }
    (visible, reasoning)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(splitter: &mut ThinkTagSplitter) -> Vec<ThinkRun> {
        splitter.finish()
    }

    fn feed(splitter: &mut ThinkTagSplitter, chunk: &str) -> Vec<ThinkRun> {
        splitter.split(chunk)
    }

    #[test]
    fn basic_split() {
        let mut s = ThinkTagSplitter::default();
        let runs = feed(&mut s, "a<think>b</think>c");
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("a".into()),
                ThinkRun::Reasoning("b".into()),
                ThinkRun::Text("c".into()),
            ]
        );
    }

    #[test]
    fn all_supported_tag_pairs() {
        for (open, close) in [
            ("<think>", "</think>"),
            ("<thinking>", "</thinking>"),
            ("<reasoning>", "</reasoning>"),
            ("<thought>", "</thought>"),
            ("<mm:think>", "</mm:think>"),
        ] {
            let mut s = ThinkTagSplitter::default();
            let runs = feed(&mut s, &format!("pre{open}mid{close}post"));
            assert_eq!(
                runs,
                vec![
                    ThinkRun::Text("pre".into()),
                    ThinkRun::Reasoning("mid".into()),
                    ThinkRun::Text("post".into()),
                ],
                "pair {open}..{close}"
            );
        }
    }

    #[test]
    fn case_insensitive_matching() {
        let mut s = ThinkTagSplitter::default();
        let runs = feed(&mut s, "a<THINK>b</Thinking>c");
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("a".into()),
                ThinkRun::Reasoning("b".into()),
                ThinkRun::Text("c".into()),
            ]
        );
    }

    #[test]
    fn tags_split_across_chunks() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "pre <thi");
        runs.extend(feed(&mut s, "nk>mid thought<"));
        runs.extend(feed(&mut s, "/thin"));
        runs.extend(feed(&mut s, "k>post"));
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("pre ".into()),
                ThinkRun::Reasoning("mid thought".into()),
                ThinkRun::Text("post".into()),
            ]
        );
    }

    #[test]
    fn ambiguous_prefix_resolves_to_longer_tag() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "<think");
        assert!(runs.is_empty());
        runs.extend(feed(&mut s, "ing>deep</thinking>done"));
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Reasoning("deep".into()),
                ThinkRun::Text("done".into()),
            ]
        );
    }

    #[test]
    fn no_match_flushes_literal_and_reprocesses() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "a<thix");
        // `<thi` is literal and `x` is regular text; both flush as one run.
        assert_eq!(runs, vec![ThinkRun::Text("a<thix".into())]);
        runs.extend(feed(&mut s, "b<think>r</think>"));
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("a<thix".into()),
                ThinkRun::Text("b".into()),
                ThinkRun::Reasoning("r".into()),
            ]
        );
    }

    #[test]
    fn mismatch_char_can_start_a_new_tag() {
        let mut s = ThinkTagSplitter::default();
        // `<thi` literal, then `<` restarts a candidate that completes.
        let runs = feed(&mut s, "a<thix<th");
        assert_eq!(runs, vec![ThinkRun::Text("a<thix".into())]);
        let mut runs2 = feed(&mut s, "ink>r</think>");
        runs2.extend(drain(&mut s));
        assert_eq!(runs2, vec![ThinkRun::Reasoning("r".into()),]);
    }

    #[test]
    fn eof_partial_open_tag_is_flushed_as_text() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "x<thi");
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![ThinkRun::Text("x".into()), ThinkRun::Text("<thi".into())]
        );
    }

    #[test]
    fn eof_partial_close_tag_is_flushed_as_reasoning() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "x<think>thought</thi");
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("x".into()),
                ThinkRun::Reasoning("thought".into()),
                ThinkRun::Reasoning("</thi".into()),
            ]
        );
    }

    #[test]
    fn unclosed_think_at_eof_keeps_reasoning() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "x<think>still thinking");
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("x".into()),
                ThinkRun::Reasoning("still thinking".into()),
            ]
        );
    }

    #[test]
    fn preserves_multibyte_utf8() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(
            &mut s,
            "思考一下：<think>中文推理过程</think>好的，回答你。",
        );
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("思考一下：".into()),
                ThinkRun::Reasoning("中文推理过程".into()),
                ThinkRun::Text("好的，回答你。".into()),
            ]
        );
    }

    #[test]
    fn multibyte_across_chunk_boundary() {
        let mut s = ThinkTagSplitter::default();
        let mut runs = feed(&mut s, "前段<thi");
        runs.extend(feed(&mut s, "nk>思考过程</think>后段"));
        runs.extend(drain(&mut s));
        assert_eq!(
            runs,
            vec![
                ThinkRun::Text("前段".into()),
                ThinkRun::Reasoning("思考过程".into()),
                ThinkRun::Text("后段".into()),
            ]
        );
    }

    #[test]
    fn pure_split_returns_text_and_reasoning() {
        let (text, reasoning) = split_think_tags("a<thinking>b</thinking>c<thi");
        assert_eq!(text, "ac<thi");
        assert_eq!(reasoning, "b");
    }

    #[test]
    fn no_tags_passes_through_untouched() {
        let (text, reasoning) = split_think_tags("a < b and c > d");
        assert_eq!(text, "a < b and c > d");
        assert!(reasoning.is_empty());
    }
}
