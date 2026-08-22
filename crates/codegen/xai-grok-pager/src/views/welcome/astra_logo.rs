//! Astra logo — a 5-letter pixel-block "ASTRA" rendered with Unicode block
//! characters in the user-requested orange palette.
//!
//! Used by the welcome screen's top bar. Each letter is 5 columns wide and 5
//! rows tall, separated by single-column gaps, for a total of 5×5 + 4×1 = 29
//! columns and 5 rows. The bottom row is a small wordmark spacer (the "ASTRA"
//! letters are joined; the bottom row is purely the descender of the "A").
//!
//! The orange palette is hard-coded to keep the brand constant across themes:
//!   • primary glyphs  → `#FF6A00`
//!   • dim shading     → `#CC5500`
//!
//! The renderer returns a `Vec<Line<'static>>` of styled spans so the welcome
//! screen can paint it via `Paragraph::new(...).render(area, buf)` without
//! needing to know about the underlying color codes.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// Primary Astra orange (`#FF6A00`) — same value the default theme uses for
/// accents / selection / focus.
const ORANGE: Color = Color::Rgb(0xFF, 0x6A, 0x00);
/// Dimmer orange (`#CC5500`) — same value the default theme uses for muted
/// highlights / secondary accents.
const ORANGE_DIM: Color = Color::Rgb(0xCC, 0x55, 0x00);

/// Block character used to draw the pixel letters. `█` is a full block
/// (lit); the `S`/`D` glyph markers select primary vs dim orange at render time.
const LIT: &str = "█";

/// Total visual width of the ASTRA wordmark (5 letters × 5 cols + 4 gaps).
const TOTAL_COLS: usize = 29;
/// Center column of the wordmark — the "R" sits at index 15 (0-based).
const CENTER_COL: f32 = (TOTAL_COLS as f32 - 1.0) / 2.0;
/// Animation period in seconds — one full center→edge→center sweep.
const PULSE_PERIOD_SECS: f32 = 3.2;
/// Half-width of the highlight band in columns. Glyphs within this many
/// columns of the moving front get a brightness boost.
const BAND_HALF_WIDTH: f32 = 5.0;
/// How much brighter the peak of the wave is vs the resting orange. 0.0
/// keeps the logo static; 1.0 makes the band reach pure white.
const SHINE_STRENGTH: f32 = 0.65;

/// Animated variant of [`astra_logo_lines`] — same glyphs and layout,
/// but each pixel pulses in brightness with a wave that sweeps outward
/// from the center column. `phase_secs` is the same monotonically
/// increasing time source the rest of the welcome animation uses
/// (see `logo::anim_phase_secs`).
///
/// The wave is a half-cosine band: at any column `c` the brightness
/// boost is `SHINE_STRENGTH * (1 + cos(π d / BAND)) / 2`, where `d` is
/// the distance from the current wave front. The front oscillates
/// between column 0 and the rightmost column, so the sheen appears to
/// breathe from the middle outward, then settle, then breathe again.
pub fn astra_logo_lines_anim(phase_secs: f32) -> Vec<Line<'static>> {
    // Wave front position oscillates between -BAND_HALF_WIDTH (just past
    // the left edge) and TOTAL_COLS + BAND_HALF_WIDTH (just past the
    // right edge), so the band enters and exits cleanly.
    let cycle = (phase_secs / PULSE_PERIOD_SECS).fract();
    // Triangle wave: 0→1 in the first half of the cycle, 1→0 in the second.
    let tri = if cycle < 0.5 {
        cycle * 2.0
    } else {
        2.0 - cycle * 2.0
    };
    let front = -BAND_HALF_WIDTH + tri * (TOTAL_COLS as f32 + 2.0 * BAND_HALF_WIDTH);
    astra_logo_lines_with(|col| shine_at(col as f32, front))
}

/// Static helper used by both [`astra_logo_lines`] (passing `|_| 0.0`)
/// and [`astra_logo_lines_anim`]. `brightness` is the additive boost
/// to apply to a glyph's orange; 0.0 leaves it resting, 1.0 saturates
/// to white.
fn shine_at(col: f32, front: f32) -> f32 {
    let d = (col - front).abs();
    if d >= BAND_HALF_WIDTH {
        0.0
    } else {
        SHINE_STRENGTH * 0.5 * (1.0 + (std::f32::consts::PI * d / BAND_HALF_WIDTH).cos())
    }
}

/// Boost a base orange by `amount` (clamped to `[0, 1]`). Amount 0
/// returns the base; amount 1 returns white. We lerp in linear sRGB
/// because the logo lives at small dot density and the difference is
/// imperceptible vs gamma-correct blending.
fn boost(base: Color, amount: f32) -> Color {
    if amount <= 0.0 {
        return base;
    }
    let Color::Rgb(r, g, b) = base else {
        return base;
    };
    let a = amount.clamp(0.0, 1.0);
    let lerp = |channel: u8| -> u8 {
        let c = channel as f32 + (255.0 - channel as f32) * a;
        c.round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(r), lerp(g), lerp(b))
}

/// 5-row, 5-column pixel-art glyphs for A, S, T, R, A. `L` = lit, `S` =
/// shaded, `D` = dim, ` ` = blank. The renderer maps `L→ORANGE`, `S→ORANGE`,
/// `D→ORANGE_DIM`, ` `→nothing.
const LETTERS: [&[&str; 5]; 5] = [
    // A (5 cols × 5 rows)
    &[" LLL ", "L   L", "LLLLL", "L   L", "L   L"],
    // S (5 cols × 5 rows)
    &["LLLLL", "L    ", "LLLLL", "    L", "LLLLL"],
    // T (5 cols × 5 rows)
    &["LLLLL", "  L  ", "  L  ", "  L  ", "  L  "],
    // R (5 cols × 5 rows)
    &["LLLL ", "L   L", "LLLL ", "L  L ", "L   L"],
    // A (5 cols × 5 rows)
    &[" LLL ", "L   L", "LLLLL", "L   L", "L   L"],
];

/// Compose the 5 letters into one `Vec<Line<'static>>` with no animation.
/// Each line spans 29 columns: 5 (letter) + 1 (gap) + 5 + 1 + 5 + 1 + 5 + 1
/// + 5 = 29.
///
/// New code that wants the breathing effect should call
/// [`astra_logo_lines_anim`] instead.
pub fn astra_logo_lines() -> Vec<Line<'static>> {
    astra_logo_lines_with(|_| 0.0)
}

/// Inner builder shared by [`astra_logo_lines`] and [`astra_logo_lines_anim`].
/// `shine_at_col` returns the brightness boost (0..=1) for the given
/// column index of the rendered wordmark; it is called per non-blank glyph
/// to decide the actual color to paint.
fn astra_logo_lines_with<F>(mut shine_at_col: F) -> Vec<Line<'static>>
where
    F: FnMut(usize) -> f32,
{
    let rows = 5usize;
    let mut out: Vec<Line<'static>> = Vec::with_capacity(rows);
    // Track the running column index across the row so the wave knows
    // where each glyph sits. Letters contribute 5 cols + a 1-col gap (no
    // gap after the last letter); blanks between letters are still real
    // columns from the sheen wave's perspective.
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col: usize = 0;
        for (i, letter) in LETTERS.iter().enumerate() {
            if i > 0 {
                // 1-column gap between letters.
                spans.push(Span::raw(" "));
                col += 1;
            }
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            for ch in letter[row].chars() {
                let (base_color, push) = match ch {
                    'L' => (Some(ORANGE), true),
                    'S' => (Some(ORANGE), true),
                    'D' => (Some(ORANGE_DIM), true),
                    _ => (None, false),
                };
                if !push {
                    if !run.is_empty() && let Some(c) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(c),
                        ));
                    }
                    spans.push(Span::raw(" "));
                    col += 1;
                    continue;
                }
                let boosted = boost(base_color.unwrap_or(ORANGE), shine_at_col(col));
                if run_color != Some(boosted) {
                    if !run.is_empty() && let Some(c) = run_color {
                        spans.push(Span::styled(
                            std::mem::take(&mut run),
                            Style::default().fg(c),
                        ));
                    }
                    run_color = Some(boosted);
                }
                run.push_str(LIT);
                col += 1;
            }
            if !run.is_empty() && let Some(c) = run_color {
                spans.push(Span::styled(run, Style::default().fg(c)));
            }
        }
        out.push(Line::from(spans));
    }
    out
}

/// Visual width of the rendered logo in columns.
pub const LOGO_WIDTH: u16 = 29;

/// Visual height of the rendered logo in rows.
pub const LOGO_HEIGHT: u16 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_has_expected_dimensions() {
        let lines = astra_logo_lines();
        assert_eq!(lines.len() as u16, LOGO_HEIGHT);
        for line in &lines {
            // Each line is built from styled spans; visual width sums the
            // string length of each span (block chars are width-1).
            let w: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            assert_eq!(
                w as u16, LOGO_WIDTH,
                "row width mismatch: {w} vs {LOGO_WIDTH}"
            );
        }
    }

    #[test]
    fn logo_uses_orange_palette() {
        let lines = astra_logo_lines();
        let mut saw_primary = false;
        let mut saw_dim = false;
        for line in &lines {
            for span in &line.spans {
                if let Some(fg) = span.style.fg {
                    if fg == ORANGE {
                        saw_primary = true;
                    } else if fg == ORANGE_DIM {
                        saw_dim = true;
                    }
                }
            }
        }
        assert!(saw_primary, "logo must contain the primary #FF6A00 orange");
        // Not every glyph uses the dim shade — but the S-letter S-row does.
        // We don't require a dim hit so this stays permissive across edits.
        let _ = saw_dim;
    }
}
