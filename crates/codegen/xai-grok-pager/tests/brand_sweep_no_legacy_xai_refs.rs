//! Regression test for the Astra brand sweep.
//!
//! Background: the project rebranded from xAI/Grok to Astra. The user-facing
//! docs were hand-swept, but new copy gets added all the time and it's easy
//! to reintroduce a stale `grok.com`, `x.ai/cli/install.sh`, or
//! `xai.com` reference by accident. This test fails CI if any of those
//! strings reappear in the user-guide markdown so the regression is caught
//! at PR time, not after release.
//!
//! Scope: the user-facing docs directory
//! (`crates/codegen/xai-grok-pager/docs/`). Internal crates, telemetry
//! identifiers, ACP wire-protocol namespaces (`x.ai/fs/*`, `x.ai/git/*`,
//! etc.), OAuth scope strings, and the `grok-build` model id are all
//! deliberately excluded — they are wire-level identifiers that real
//! upstream systems depend on and renaming them would break the product.
//!
//! Allowlist: reserved for lines that legitimately reference the legacy
//! names in the context of "what we used to be called". As of the
//! topodrive migration the user-guide docs no longer contain any of the
//! forbidden substrings, so the allowlist is empty. Add an entry here only
//! if a future rewrite intentionally reintroduces a legacy string.

use std::fs;
use std::path::{Path, PathBuf};

/// Patterns that signal a leaked legacy brand reference in user-facing copy.
/// Matched as plain substrings (case-sensitive) so a comment accidentally
/// saying `grok.com` trips the test even if it's in a code block. The
/// test fails if any match falls outside the allowlist below.
///
/// `docs.x.ai` / `accounts.x.ai` / `console.x.ai` were removed once the
/// user-guide docs stopped referencing those endpoints — keep the list
/// focused on legacy product/brand strings that would clearly signal a
/// rebrand regression.
const FORBIDDEN_SUBSTRINGS: &[&str] = &["grok.com", "x.ai/cli/install", "xai.com"];

/// One forbidden substring + a unique context line from the file it's in.
/// Allows legitimate mentions (e.g. user-supplied `allowed_domains` example
/// values) to pass without renaming. Listed lines are matched verbatim;
/// any drift in the surrounding context (even a single space) makes the
/// allowlist miss and the test fails loudly.
const ALLOWLIST: &[(&str, &str)] = &[];

/// Docs root. Relative to the crate root because the test is `cargo test`
/// from any working directory.
fn docs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docs")
}

fn collect_markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => panic!("read_dir({}) failed: {e}", root.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_markdown_files(&path));
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn line_of(content: &str, line_number: usize) -> &str {
    content
        .lines()
        .nth(line_number.saturating_sub(1))
        .unwrap_or("")
}

/// Walk every `*.md` under `docs/` and report any forbidden substring whose
/// containing line is not in the allowlist. The error message lists each
/// failure as `file:line  pattern  snippet` so a future sweeper can grep
/// straight to the spot.
#[test]
fn no_legacy_xai_brands_in_user_docs() {
    let root = docs_root();
    assert!(root.is_dir(), "docs root must exist: {}", root.display());

    let mut violations: Vec<String> = Vec::new();
    for path in collect_markdown_files(&root) {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                violations.push(format!("{}: unreadable: {e}", path.display()));
                continue;
            }
        };
        for (line_no, line) in content.lines().enumerate() {
            let line_no = line_no + 1;
            for pat in FORBIDDEN_SUBSTRINGS {
                if !line.contains(pat) {
                    continue;
                }
                let allowed = ALLOWLIST
                    .iter()
                    .any(|(allow_pat, allow_line)| *allow_pat == *pat && **allow_line == *line);
                if !allowed {
                    violations.push(format!(
                        "{}:{}  {}  >>>{}<<<",
                        path.display(),
                        line_no,
                        pat,
                        line.trim()
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "user-facing docs still contain legacy xAI/Grok brand strings:\n{}",
        violations.join("\n")
    );
}

/// Convenience: pin a single line per pattern so future sweeps have a
/// starting point. These are informational — the strict test above is the
/// source of truth. They print (don't assert) on test failure to help a
/// human spot the regression pattern faster than reading the violation list.
#[test]
fn brand_sweep_allowlist_drift_check() {
    let root = docs_root();
    for (pat, line) in ALLOWLIST {
        let mut found = false;
        for path in collect_markdown_files(&root) {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if content.lines().any(|l| *l == **line) {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "allowlist entry for `{pat}` no longer matches any line in docs; \
             the surrounding context drifted. Update ALLOWLIST (or remove the \
             line if the rewrite is now legitimate). Line was: {line:?}",
        );
        let _ = line_of; // silence unused-import lint if the helper drops out
    }
}
