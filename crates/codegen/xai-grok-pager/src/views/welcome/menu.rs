//! Menu component — renders shortcut key menus.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;

use super::logo::logo_visual_width;

fn cols(text: &str) -> u16 {
    unicode_width::UnicodeWidthStr::width(text) as u16
}

/// Half-width of the hover-sheen light band, in cells. A glyph within this
/// many columns of the mouse cursor gets the full boost; outside, the boost
/// falls off to zero. Eight cells gives the band enough reach to feel like
/// a soft wash on a 51-col menu without ever being so wide it looks like
/// the whole row is selected.
const SHEEN_HALF_WIDTH: f32 = 8.0;
/// Maximum additive boost toward `text_primary` at the band peak. 0.6 keeps
/// the underlying text color (e.g. `gray_bright` for the shortcut key)
/// readable even at peak, while still creating a clear "lit" feel.
const SHEEN_STRENGTH: f32 = 0.6;

/// Boost `base` toward `target` by `amount` (clamped to `[0, 1]`).
///
/// Linear sRGB lerp is fine here — the menu paints in a small dot
/// density and the human eye reads the band as a soft highlight without
/// noticing gamma. Non-RGB colors (named ANSI, indexed) are returned
/// unchanged so the function is safe to call on quantized themes.
fn boost_color(base: Color, target: Color, amount: f32) -> Color {
    if amount <= 0.0 {
        return base;
    }
    let (Color::Rgb(br, bg, bb), Color::Rgb(tr, tg, tb)) = (base, target) else {
        return base;
    };
    let a = amount.clamp(0.0, 1.0) as f32;
    let lerp = |c: u8, t: u8| -> u8 {
        let v = c as f32 + (t as f32 - c as f32) * a;
        v.round().clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lerp(br, tr), lerp(bg, tg), lerp(bb, tb))
}

/// Paint a horizontal text segment one cell at a time, boosting each cell's
/// color toward `theme.text_primary` based on its distance from the mouse
/// cursor. Used by the hover row to render a soft radial light band.
fn paint_sheen_segment(
    buf: &mut Buffer,
    y: u16,
    x_start: u16,
    text: &str,
    base_style: Style,
    mouse_pos: Option<(u16, u16)>,
    theme: &Theme,
) {
    let target = theme.text_primary;
    let mouse_x = mouse_pos.map(|(mx, _)| mx as f32);
    for (i, ch) in text.chars().enumerate() {
        let col = x_start + i as u16;
        let boost = mouse_x
            .map(|mx| {
                let d = (col as f32 - mx).abs();
                if d >= SHEEN_HALF_WIDTH {
                    0.0
                } else {
                    SHEEN_STRENGTH * 0.5 * (1.0 + (std::f32::consts::PI * d / SHEEN_HALF_WIDTH).cos())
                }
            })
            .unwrap_or(0.0);
        let boosted = match base_style.fg {
            Some(fg) => boost_color(fg, target, boost),
            None => theme.text_primary,
        };
        let style = base_style.fg(boosted).add_modifier(Modifier::BOLD);
        if let Some(cell) = buf.cell_mut((col, y)) {
            cell.set_char(ch);
            cell.set_style(style);
        }
    }
}

/// Render the welcome menu rows as `label … shortcut`, padded within each row.
/// Returns the Rect for each item row (for hit-testing clicks and hover).
pub fn render_menu(
    area: Rect,
    buf: &mut Buffer,
    theme: &Theme,
    items: &[(&str, &str)],
    selected: Option<usize>,
    mouse_pos: Option<(u16, u16)>,
    min_width_hint: u16,
) -> Vec<Rect> {
    let label_style = Style::default()
        .fg(theme.text_primary)
        .add_modifier(Modifier::BOLD);
    let label_selected_style = Style::default()
        .fg(theme.accent_user)
        .bg(theme.bg_highlight)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(theme.gray_bright);
    let key_selected_style = Style::default()
        .fg(theme.accent_user)
        .bg(theme.bg_highlight);

    // Width: label + gap + key. Keep a 4-col gap between label and key for
    // readability.
    let content_min: u16 = items
        .iter()
        .map(|(key, label)| cols(key) + cols(label) + 4)
        .max()
        .unwrap_or(0);
    let menu_width = logo_visual_width(area.height)
        .max(30)
        .max(content_min)
        .max(min_width_hint);

    let [_, menu_centered, _] = Layout::horizontal([
        Constraint::Min(0),
        Constraint::Length(menu_width),
        Constraint::Min(0),
    ])
    .flex(Flex::Center)
    .areas(area);

    let mut rects = Vec::with_capacity(items.len());
    let mut y = menu_centered.y;
    for (i, (key, label)) in items.iter().enumerate() {
        if y >= menu_centered.y + menu_centered.height {
            break;
        }

        let is_selected = selected == Some(i);
        // Mouse hover detection: a row counts as hovered when the mouse
        // is on this row and the row is NOT the currently selected one.
        // Selected rows keep their flat bg_highlight treatment; the sheen
        // is reserved for "I'm about to click this" affordance.
        let is_hovered = !is_selected
            && mouse_pos.is_some_and(|(mx, my)| {
                my == y && mx >= menu_centered.x && mx < menu_centered.x + menu_centered.width
            });
        let key_width = cols(key);
        // The key sits at the right edge, so the label is cut to leave room for it.
        let label = crate::render::line_utils::truncate_str(
            label,
            menu_centered.width.saturating_sub(key_width + 1) as usize,
        );
        let label_len = cols(&label);

        let row_rect = Rect {
            x: menu_centered.x,
            y,
            width: menu_centered.width,
            height: 1,
        };
        rects.push(row_rect);

        // Fill row background when selected/hovered. The hovered row gets
        // a softer tint (`bg_hover` sits between `bg_highlight` and
        // `bg_visual` in the theme hierarchy) so the user can still tell
        // it apart from the selected row at a glance.
        if is_selected {
            let hover_bg = Style::default().bg(theme.bg_highlight);
            for x in menu_centered.x..menu_centered.x + menu_centered.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(hover_bg);
                }
            }
        } else if is_hovered {
            let hover_bg = Style::default().bg(theme.bg_hover);
            for x in menu_centered.x..menu_centered.x + menu_centered.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_style(hover_bg);
                }
            }
        }

        // Label, flush with the left edge of the menu column.
        let lstyle = if is_selected {
            label_selected_style
        } else {
            label_style
        };
        if is_hovered {
            // Cell-level sheen: each character gets its own color,
            // boosted toward `text_primary` by `1 - cos(π d / band) / 2`
            // where `d` is the cell's distance from the mouse cursor.
            // This paints a soft horizontal "light band" radiating from
            // the cursor; the band moves with the mouse.
            paint_sheen_segment(
                buf,
                y,
                menu_centered.x,
                &label,
                label_style,
                mouse_pos,
                theme,
            );
        } else {
            buf.set_span(
                menu_centered.x,
                y,
                &Span::styled(&*label, lstyle),
                label_len,
            );
        }

        // Key shortcut flush with the right edge of the menu column.
        let kstyle = if is_selected {
            key_selected_style
        } else {
            key_style
        };
        let key_x = menu_centered.x + menu_centered.width - key_width;
        if is_hovered {
            paint_sheen_segment(buf, y, key_x, key, key_style, mouse_pos, theme);
        } else {
            buf.set_span(key_x, y, &Span::styled(*key, kstyle), key_width);
        }

        // [x] dismiss affordance restyling (for the import row)
        if let Some(x_offset) = key.rfind("[x]") {
            let key_x_start = menu_centered.x + menu_centered.width - key_width;
            let dismiss_start = key_x_start + x_offset as u16;
            let dismiss_end = dismiss_start + 3;
            let mouse_on_dismiss = mouse_pos
                .is_some_and(|(mx, my)| my == y && mx >= dismiss_start && mx < dismiss_end);
            let dismiss_color = if mouse_on_dismiss {
                theme.text_primary
            } else {
                theme.gray_bright
            };
            let dismiss_style = if is_selected {
                Style::default()
                    .fg(dismiss_color)
                    .bg(theme.bg_highlight)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(dismiss_color)
                    .add_modifier(Modifier::BOLD)
            };
            for (offset, ch) in "[x]".chars().enumerate() {
                let col = dismiss_start + offset as u16;
                if let Some(cell) = buf.cell_mut((col, y)) {
                    cell.set_char(ch);
                    cell.set_style(dismiss_style);
                }
            }
        }

        y += 1;
    }

    rects
}
