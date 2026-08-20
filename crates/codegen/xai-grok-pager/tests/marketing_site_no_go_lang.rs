//! Regression test for the marketing site copy.
//!
//! Background: the project rebranded from xAI/Grok to Astra, and the
//! project itself is written in Rust. Hand-sweeps of `site/index.html`,
//! `site/login.html`, `site/account.html`, `site/authorize.html`,
//! `site/favicon.svg`, and the install scripts are easy to drift on:
//! someone copy-pastes a stale snippet from an older blog post or PR
//! description and the site suddenly advertises a Go toolchain or a
//! `go install` one-liner. This test walks the `site/` tree and fails on
//! any leftover Go-language marketing copy so the drift gets caught at
//! PR time.
//!
//! Scope: `site/` plus `crates/codegen/astra-auth/assets/site/` (the
//! embedded copy served by the auth server). Internal Rust crates are
//! not in scope; references like `mod golang;` or `go test ./...` inside
//! tool-classifier tests are legitimate fixtures.
//!
//! Allowlist: a few cases legitimately mention `go`/`Go` because the
//! string is either a domain name component or appears in a
//! non-language context. Each entry is a (pattern, exact line) pair; the
//! line must appear verbatim in the file for the allowlist to apply.

use std::fs;
use std::path::{Path, PathBuf};

/// Marketing-site files we audit. Paths are written relative to the
/// test crate root (`crates/codegen/xai-grok-pager/...`); three `../`s
/// climb back to the repo root.
const SITE_ROOTS: &[&str] = &[
    // repo root → site/
    "../../../site",
    // repo root → astra-auth/assets/site/ (embedded copy)
    "../../../crates/codegen/astra-auth/assets/site",
];

/// One forbidden marketing phrase. `word_boundaries = true` adds a
/// start/end ASCII-non-alphanumeric check, so a `cargo install` line in
/// a CI snippet does *not* trigger the otherwise-substring-matching
/// `go install` rule.
struct ForbiddenPhrase {
    needle: &'static str,
    word_boundaries: bool,
}

/// Phrases that trip the test. All entries use canonical Go-language
/// capitalisation. Negative framing (e.g. "no Go toolchain", "you do not
/// need a Go runtime") is *correct* marketing copy and is excluded by
/// design — only positive Go claims leak the rewrite gap.
const FORBIDDEN_PHRASES: &[ForbiddenPhrase] = &[
    // The exact wrong install command — the headline bug the hand sweep
    // fixed.
    ForbiddenPhrase {
        needle: "go install github.com/kevenhu001-cyber/astra-harness/cmd/astra@latest",
        word_boundaries: false,
    },
    // Generic `go install` framing. Word-bounded so `cargo install
    // dotslash` lines elsewhere in the repo do not match.
    ForbiddenPhrase {
        needle: "go install",
        word_boundaries: true,
    },
    // Words used in marketing paragraphs claiming the project *is* a
    // Go project.
    ForbiddenPhrase {
        needle: "written in Go",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "built with Go",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "built in Go",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "you'll need a Go toolchain",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "you will need a Go toolchain",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "you need a Go toolchain",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "requires the Go toolchain",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "requires Go to install",
        word_boundaries: false,
    },
    ForbiddenPhrase {
        needle: "Golang runtime",
        word_boundaries: false,
    },
];

/// Exact-line allowlist. Empty today; populated only if a specific line
/// in the marketing copy legitimately needs to mention a forbidden
/// phrase in a non-marketing context.
const ALLOWLIST: &[(&str, &str)] = &[];

/// True when `line` contains `phrase.needle`, optionally with
/// word-boundary semantics.
fn phrase_hits(line: &str, phrase: &ForbiddenPhrase) -> bool {
    if !line.contains(phrase.needle) {
        return false;
    }
    if !phrase.word_boundaries {
        return true;
    }
    for (idx, _) in line.match_indices(phrase.needle) {
        let bytes = line.as_bytes();
        let start_ok = idx == 0 || !bytes[idx - 1].is_ascii_alphanumeric();
        let end = idx + phrase.needle.len();
        let end_ok = end >= line.len() || !bytes[end].is_ascii_alphanumeric();
        if start_ok && end_ok {
            return true;
        }
    }
    false
}

/// Walk every regular file under a site root, restricted to a few
/// extensions so we don't recurse into vendored libraries or icons.
fn collect_marketing_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(collect_marketing_files(&path));
        } else if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            if matches!(ext, "html" | "md" | "css" | "js" | "txt" | "sh" | "ps1") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// True if `line` at `idx` matches an allowlist entry verbatim.
fn line_allowlisted(content: &str, idx: usize, line: &str) -> bool {
    if ALLOWLIST.is_empty() {
        return false;
    }
    let line_no = idx;
    ALLOWLIST.iter().any(|(allow_pat, allow_line)| {
        line.contains(allow_pat)
            && content
                .lines()
                .nth(line_no)
                .map(|l| l == *allow_line)
                .unwrap_or(false)
    })
}

/// Single shared audit. Walks every `SITE_ROOTS` entry, accumulates
/// violations, and panics with the violation list if anything
/// goes wrong. Each test below is just a labelled entry point so a
/// failure points the operator at the right subtree.
fn audit(label: &str) {
    let mut violations: Vec<String> = Vec::new();
    let mut roots_seen: usize = 0;
    for rel in SITE_ROOTS {
        let root = manifest_join(rel);
        if !root.is_dir() {
            continue;
        }
        roots_seen += 1;
        let files = collect_marketing_files(&root);
        for path in files {
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue, // binary blob; skip
            };
            for (idx, line) in content.lines().enumerate() {
                let line_no = idx + 1;
                for phrase in FORBIDDEN_PHRASES {
                    if !phrase_hits(line, phrase) {
                        continue;
                    }
                    if line_allowlisted(&content, idx, line) {
                        continue;
                    }
                    violations.push(format!(
                        "{label}: {}:{}  {}  >>>{}<<<",
                        path.display(),
                        line_no,
                        phrase.needle,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        roots_seen > 0,
        "no marketing site root found under {label}; checked: {:?}",
        SITE_ROOTS
    );
    assert!(
        violations.is_empty(),
        "{label} still contains Go-language marketing copy:\n{}",
        violations.join("\n")
    );
}

#[test]
fn no_go_marketing_in_site_index() {
    audit("site/index.html");
}

#[test]
fn no_go_marketing_in_site_auth_assets() {
    audit("site/assets/ + astra-auth embedded copy");
}

#[test]
fn no_go_marketing_in_site_install_scripts() {
    audit("site/install/");
}

fn manifest_join(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}