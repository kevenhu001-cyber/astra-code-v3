//! Crossfade between two `Theme` snapshots on theme switch.
//!
//! When the user picks a new theme (via `/theme`, the picker modal, or
//! the auto-mode appearance watcher), the pager would otherwise snap
//! the entire canvas from old to new in a single frame. On a 24-bit
//! terminal that snap is jarring — the eye loses its place, the
//! selected row's accent color flips instantly, and animations
//! momentarily desync.
//!
//! This module stores the previous `Theme` alongside a monotonic
//! "started at" timestamp. The caller asks for a frame-time `sample()`
//! and gets back a third `Theme` whose every field is a per-channel
//! linear RGB lerp between the two endpoints. The crossfade is
//! complete after [`TRANSITION_DURATION_SECS`]; subsequent calls
//! return the new theme verbatim.
//!
//! ## Scope
//!
//! Only `Color`-typed theme fields participate. `Modifier`s (e.g. for
//! the H1 / H2 markdown headings) and the `ThemeKind` itself are
//! not interpolated — the modifier set is a discrete choice and the
//! kind is what triggered the transition in the first place.
//!
//! ## Concurrency
//!
//! Like the rest of the theme cache, the transition state lives in
//! atomics so the render loop can read it without taking a lock. The
//! "previous theme" is stored as raw RGB triples to avoid a
//! `Color`-by-`Color` compare inside the atomic load.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use ratatui::style::Color;

use super::Theme;

/// How long the crossfade lasts. 200 ms is the sweet spot on a
/// 60-fps TUI: fast enough to feel responsive, slow enough to be
/// perceptible (about 12 frames). On a 240-Hz terminal the user
/// still sees ~48 frames of the transition.
pub const TRANSITION_DURATION_SECS: f32 = 0.20;

/// Monotonic timestamp (nanoseconds since the rate-limiter epoch) at
/// which the currently-stored transition started, or `u64::MAX` to
/// signal "no active transition".
static START_NS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Packed (r << 16) | (g << 8) | b for each of the two snapshots'
/// accent_user, accent_assistant, accent_error and bg_base fields
/// (the four colors that move the most visually). We only need the
/// "old" snapshot here because the "new" snapshot is always the
/// current `Theme` — readers can pull the new colors from
/// `Theme::current()` and lerp themselves.
static OLD_ACCENT_USER: AtomicU64 = AtomicU64::new(0);
static OLD_ACCENT_ASSISTANT: AtomicU64 = AtomicU64::new(0);
static OLD_ACCENT_ERROR: AtomicU64 = AtomicU64::new(0);
static OLD_BG_BASE: AtomicU64 = AtomicU64::new(0);
static OLD_TEXT_PRIMARY: AtomicU64 = AtomicU64::new(0);
static OLD_GRAY: AtomicU64 = AtomicU64::new(0);
static OLD_PROMPT_BORDER: AtomicU64 = AtomicU64::new(0);
static OLD_PROMPT_BORDER_ACTIVE: AtomicU64 = AtomicU64::new(0);

/// Begin a crossfade from `prev` to the theme that will become
/// `current()` on the next render. Idempotent: calling this with
/// an already-running transition replaces the start time and the
/// "old" snapshot with the new ones (so an interrupted transition
/// doesn't pop).
pub fn start(prev: &Theme) {
    let now = monotonic_nanos();
    START_NS.store(now, Ordering::Release);
    OLD_ACCENT_USER.store(pack(prev.accent_user), Ordering::Relaxed);
    OLD_ACCENT_ASSISTANT.store(pack(prev.accent_assistant), Ordering::Relaxed);
    OLD_ACCENT_ERROR.store(pack(prev.accent_error), Ordering::Relaxed);
    OLD_BG_BASE.store(pack(prev.bg_base), Ordering::Relaxed);
    OLD_TEXT_PRIMARY.store(pack(prev.text_primary), Ordering::Relaxed);
    OLD_GRAY.store(pack(prev.gray), Ordering::Relaxed);
    OLD_PROMPT_BORDER.store(pack(prev.prompt_border), Ordering::Relaxed);
    OLD_PROMPT_BORDER_ACTIVE.store(pack(prev.prompt_border_active), Ordering::Relaxed);
}

/// Cancel any in-flight transition and snap back to the current theme.
/// Called when the terminal-native lock engages (minimal mode) — the
/// transition machinery is wasted work when the theme is the terminal
/// default, and the atomic loads would just return the same value
/// forever.
pub fn cancel() {
    START_NS.store(u64::MAX, Ordering::Release);
}

/// Read the per-frame "live" theme, applying any in-flight crossfade
/// on top of the static `Theme::current()` snapshot.
///
/// When no transition is active, this is a single atomic load that
/// returns `None` and the caller should use the plain `current()` —
/// the crossfade hot path is allocation-free.
pub fn sample_live(theme: Theme) -> Theme {
    let start_ns = START_NS.load(Ordering::Acquire);
    if start_ns == u64::MAX {
        return theme;
    }
    let elapsed_ns = monotonic_nanos().saturating_sub(start_ns);
    let elapsed_secs = elapsed_ns as f32 / 1_000_000_000.0;
    if elapsed_secs >= TRANSITION_DURATION_SECS {
        // Transition complete — clear and return the new theme.
        cancel();
        return theme;
    }
    // Eased progress: smoothstep on the linear fraction. Avoids the
    // perceptible "ease-in" ramp at t=0 and "ease-out" ramp at t=1
    // that linear lerp gives you.
    let t = elapsed_secs / TRANSITION_DURATION_SECS;
    let t = t.clamp(0.0, 1.0);
    let eased = t * t * (3.0 - 2.0 * t);

    let lerp_rgb = |new: Color, old_packed: u64| -> Color {
        let Some(old) = unpack(old_packed) else {
            return new;
        };
        let (Color::Rgb(nr, ng, nb), Color::Rgb(or, og, ob)) = (new, old) else {
            return new;
        };
        let lerp = |a: u8, b: u8| -> u8 {
            let av = a as f32;
            let bv = b as f32;
            (av + (bv - av) * eased).round().clamp(0.0, 255.0) as u8
        };
        Color::Rgb(lerp(nr, or), lerp(ng, og), lerp(nb, ob))
    };

    Theme {
        accent_user: lerp_rgb(theme.accent_user, OLD_ACCENT_USER.load(Ordering::Relaxed)),
        accent_assistant: lerp_rgb(
            theme.accent_assistant,
            OLD_ACCENT_ASSISTANT.load(Ordering::Relaxed),
        ),
        accent_error: lerp_rgb(theme.accent_error, OLD_ACCENT_ERROR.load(Ordering::Relaxed)),
        bg_base: lerp_rgb(theme.bg_base, OLD_BG_BASE.load(Ordering::Relaxed)),
        text_primary: lerp_rgb(theme.text_primary, OLD_TEXT_PRIMARY.load(Ordering::Relaxed)),
        gray: lerp_rgb(theme.gray, OLD_GRAY.load(Ordering::Relaxed)),
        prompt_border: lerp_rgb(theme.prompt_border, OLD_PROMPT_BORDER.load(Ordering::Relaxed)),
        prompt_border_active: lerp_rgb(
            theme.prompt_border_active,
            OLD_PROMPT_BORDER_ACTIVE.load(Ordering::Relaxed),
        ),
        ..theme
    }
}

/// Read the crossfade progress in `[0.0, 1.0]`. `1.0` means the
/// transition has finished (or none was running). Useful for
/// callers that want to drive their own animation timing off the
/// theme change.
#[allow(dead_code)]
pub fn progress() -> f32 {
    let start_ns = START_NS.load(Ordering::Acquire);
    if start_ns == u64::MAX {
        return 1.0;
    }
    let elapsed = monotonic_nanos().saturating_sub(start_ns) as f32 / 1_000_000_000.0;
    if elapsed >= TRANSITION_DURATION_SECS {
        1.0
    } else {
        (elapsed / TRANSITION_DURATION_SECS).clamp(0.0, 1.0)
    }
}

// -- Internal helpers --------------------------------------------------------

fn pack(c: Color) -> u64 {
    match c {
        Color::Rgb(r, g, b) => ((r as u64) << 16) | ((g as u64) << 8) | (b as u64),
        // Non-RGB colors (named ANSI, indexed) can't be lerped in a
        // stable way; pack 0 to mark the slot as "use new color" so the
        // crossfade simply snaps for this field.
        _ => 0,
    }
}

fn unpack(p: u64) -> Option<Color> {
    if p == 0 {
        return None;
    }
    let r = ((p >> 16) & 0xFF) as u8;
    let g = ((p >> 8) & 0xFF) as u8;
    let b = (p & 0xFF) as u8;
    Some(Color::Rgb(r, g, b))
}

fn monotonic_nanos() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    epoch.elapsed().as_nanos() as u64
}
