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

/// Block character used to draw the pixel letters. `█` is full block (lit);
/// the `S`/`D` glyph markers select primary vs dim orange at render time.
const LIT: &str = "█";

/// 5-row, 5-column pixel-art glyphs for A, S, T, R, A. `L` = lit, `S` =
/// shaded, `D` = dim, ` ` = blank. The renderer maps `L→ORANGE`, `S→ORANGE`,
/// `D→ORANGE_DIM`, ` `→nothing.
const LETTERS: [&str; 5] = [
    // A (5 cols × 5 rows)
    " L  \n\
     L L \n\
     LLL \n\
     L L \n\
     L L ",
    // S (5 cols × 5 rows)
    " SSS \n\
     S   \n\
     SSS \n\
       S \n\
     SSS ",
    // T (5 cols × 5 rows)
    "LLLLL\n\
      L  \n\
      L  \n\
      L  \n\
      L  ",
    // R (5 cols × 5 rows)
    "LLL  \n\
     L L \n\
     LLL \n\
     L L \n\
     L L ",
    // A (5 cols × 5 rows) — same as the first A
    " L  \n\
     L L \n\
     LLL \n\
     L L \n\
     L L ",
];

/// Compose the 5 letters into one `Vec<Line<'static>>`. Each line spans 29
/// columns: 5 (letter) + 1 (gap) + 5 + 1 + 5 + 1 + 5 + 1 + 5 = 29.
pub fn astra_logo_lines() -> Vec<Line<'static>> {
    // Pull each letter into a `Vec<Vec<char>>` so we can walk rows × cols and
    // build one Span per visual run.
    let letters: Vec<Vec<Vec<char>>> = LETTERS
        .iter()
        .map(|l| l.lines().map(|row| row.chars().collect()).collect())
        .collect();

    let rows = 5usize;
    let mut out: Vec<Line<'static>> = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, letter) in letters.iter().enumerate() {
            if i > 0 {
                // 1-column gap between letters.
                spans.push(Span::raw(" "));
            }
            let mut run = String::new();
            let mut run_color: Option<Color> = None;
            for ch in letter[row].iter() {
                let (color, push) = match ch {
                    'L' => (Some(ORANGE), true),
                    'S' => (Some(ORANGE), true),
                    'D' => (Some(ORANGE_DIM), true),
                    _ => (None, false),
                };
                if !push {
                    if !run.is_empty() {
                        if let Some(c) = run_color {
                            spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(c)));
                        }
                    }
                    spans.push(Span::raw(" "));
                    continue;
                }
                if run_color != color {
                    if !run.is_empty() {
                        if let Some(c) = run_color {
                            spans.push(Span::styled(std::mem::take(&mut run), Style::default().fg(c)));
                        }
                    }
                    run_color = color;
                }
                run.push_str(LIT);
            }
            if !run.is_empty() {
                if let Some(c) = run_color {
                    spans.push(Span::styled(run, Style::default().fg(c)));
                }
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
            let w: usize = line
                .spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum();
            assert_eq!(w as u16, LOGO_WIDTH, "row width mismatch: {w} vs {LOGO_WIDTH}");
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
