//! Guided wizard for `/connect`. Three text fields (URL, Model ID, API key)
//! plus a preset selector that pre-fills the URL when a non-custom preset is
//! chosen. Submits the same `Action::ConnectCustomModel` payload as the
//! one-shot `/connect <preset> <model_id> <api_key>` path, so the persistence
//! + restart-required semantics are identical.
//!
//! The wizard stays in this file rather than `views/modal.rs` to keep that
//! file focused on `ActiveModal` wiring; the actual state + key handler +
//! renderer live here. The `ActiveModal::ConnectWizard` variant references
//! this module via a `Box`ed `ConnectWizardState`.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{StatefulWidget, Widget};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

/// Stable preset ids mirrored from `slash/commands/connect.rs` so the
/// wizard can populate the URL when a non-custom vendor is picked. The
/// source of truth for the canonical labels still lives next to the slash
/// command — duplicated here to avoid a circular module dependency
/// (`views/` ↔ `slash/`).
const PRESETS: &[(&str, &str)] = &[
    ("openai", "https://api.openai.com/v1"),
    ("openai_responses", "https://api.openai.com/v1"),
    ("anthropic", "https://api.anthropic.com/v1"),
    ("xai", "https://api.x.ai/v1"),
    ("deepseek", "https://api.deepseek.com/v1"),
    ("zhipu", "https://open.bigmodel.cn/api/paas/v4"),
    ("xiaomi", "https://api.xiaomimimo.com/v1"),
    ("minimax_cn", "https://api.minimaxi.com/v1"),
    ("zai", "https://api.z.ai/api/paas/v4"),
    ("custom", ""),
];

/// Preset vendors that embed reasoning as `<think>…</think>` tags in the
/// assistant content (no native reasoning_content channel). Mirrors
/// `THINK_TAG_PRESETS` in the slash command; duplicated for the same reason
/// as `PRESETS`.
const THINK_TAG_PRESETS: &[&str] = &["minimax_cn"];

/// Result of running the wizard: the assembled values plus the resolved
/// preset id. The dispatcher turns this into an `Action::ConnectCustomModel`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectWizardResult {
    pub provider: String,
    pub model_id: String,
    pub api_key: String,
    pub base_url: String,
    pub injects_think_tags: bool,
}

/// One field in the wizard's editable form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Preset,
    Url,
    ModelId,
    ApiKey,
}

impl Field {
    /// Tab order; used by Tab/Shift+Tab navigation.
    const ORDER: &'static [Field] = &[Field::Preset, Field::Url, Field::ModelId, Field::ApiKey];

    fn index(self) -> usize {
        Self::ORDER.iter().position(|f| *f == self).unwrap_or(0)
    }

    fn from_index(i: usize) -> Field {
        Self::ORDER[i.min(Self::ORDER.len() - 1)]
    }

    fn next(self) -> Field {
        let i = self.index();
        Self::from_index(i + 1)
    }

    fn prev(self) -> Field {
        let i = self.index();
        if i == 0 {
            Field::Preset
        } else {
            Self::from_index(i - 1)
        }
    }
}

/// What the modal reports after handling one key event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardOutcome {
    /// Nothing changed.
    Unhandled,
    /// Cursor moved / text edited / focus changed — redraw.
    Changed,
    /// User pressed Esc — close the modal without committing.
    Closed,
    /// User pressed Enter on a valid form — commit.
    Submitted(ConnectWizardResult),
}

/// Wizard state. Boxed by `ActiveModal::ConnectWizard`.
#[derive(Debug, Clone)]
pub struct ConnectWizardState {
    /// Index into `PRESETS`. The chosen preset pre-fills `base_url` (unless
    /// the user has typed something custom into the URL field).
    pub preset_idx: usize,
    /// Editable URL field. Pre-filled from the preset unless the user has
    /// edited it after picking a preset — then we leave their text alone.
    pub base_url: String,
    /// Model id (free-form text).
    pub model_id: String,
    /// API key (free-form text; never echoed).
    pub api_key: String,
    /// Currently-focused field.
    pub focused: Field,
    /// Cursor offset within the focused text field (byte index). One offset
    /// per field — when focus moves we snapshot/restore the offsets so each
    /// field keeps its own cursor position across Tab navigation.
    pub cursor_preset: usize,
    pub cursor_url: usize,
    pub cursor_model: usize,
    pub cursor_key: usize,
    /// Inline validation error (`""` = no error). Displayed beneath the
    /// focused field. Prevents submission while non-empty.
    pub error: String,
}

impl Default for ConnectWizardState {
    fn default() -> Self {
        let mut s = Self {
            preset_idx: 0,
            base_url: PRESETS[0].1.to_string(),
            model_id: String::new(),
            api_key: String::new(),
            focused: Field::Preset,
            cursor_preset: 0,
            cursor_url: PRESETS[0].1.len(),
            cursor_model: 0,
            cursor_key: 0,
            error: String::new(),
        };
        // Place the URL cursor at the end of the pre-filled URL so the user
        // can immediately append or backspace-edit.
        s
    }
}

impl ConnectWizardState {
    /// Resolve the preset the user has currently picked. Returns the id
    /// (`"openai"`, `"custom"`, etc.).
    pub fn current_preset_id(&self) -> &'static str {
        PRESETS[self.preset_idx.min(PRESETS.len() - 1)].0
    }

    fn set_preset(&mut self, idx: usize) {
        let idx = idx.min(PRESETS.len() - 1);
        if idx == self.preset_idx {
            return;
        }
        let old_url = PRESETS[self.preset_idx].1;
        let new_url = PRESETS[idx].1;
        self.preset_idx = idx;
        if self.base_url == old_url || self.base_url.is_empty() {
            self.base_url = new_url.to_string();
            self.cursor_url = self.base_url.len();
        }
        // `custom` clears the URL so the user has to type one; the cursor
        // lands at offset 0 for immediate typing.
        if self.current_preset_id() == "custom" && self.base_url.is_empty() {
            self.cursor_url = 0;
        }
        self.error.clear();
    }

    /// Build the result payload. Validates first; returns `None` and sets
    /// `error` if any required field is empty.
    fn validate_and_build(&mut self) -> Option<ConnectWizardResult> {
        let provider = self.current_preset_id().to_string();
        let url = self.base_url.trim().to_string();
        let model_id = self.model_id.trim().to_string();
        let api_key = self.api_key.trim().to_string();

        if model_id.is_empty() {
            self.error = "Model ID is required.".to_string();
            self.focused = Field::ModelId;
            self.cursor_model = self.model_id.len();
            return None;
        }
        if provider == "custom" && url.is_empty() {
            self.error = "Base URL is required for the 'custom' preset.".to_string();
            self.focused = Field::Url;
            self.cursor_url = self.base_url.len();
            return None;
        }
        if api_key.is_empty() {
            self.error = "API key is required.".to_string();
            self.focused = Field::ApiKey;
            self.cursor_key = self.api_key.len();
            return None;
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            self.error = "Base URL must start with http:// or https://".to_string();
            self.focused = Field::Url;
            self.cursor_url = self.base_url.len();
            return None;
        }

        self.error.clear();
        let injects_think_tags = THINK_TAG_PRESETS.contains(&provider.as_str());
        Some(ConnectWizardResult {
            provider,
            model_id,
            api_key,
            base_url: url,
            injects_think_tags,
        })
    }

    fn focused_text_mut(&mut self) -> Option<&mut String> {
        match self.focused {
            Field::Url => Some(&mut self.base_url),
            Field::ModelId => Some(&mut self.model_id),
            Field::ApiKey => Some(&mut self.api_key),
            Field::Preset => None,
        }
    }

    fn focused_cursor(&self) -> usize {
        match self.focused {
            Field::Url => self.cursor_url,
            Field::ModelId => self.cursor_model,
            Field::ApiKey => self.cursor_key,
            Field::Preset => self.cursor_preset,
        }
    }

    fn set_focused_cursor(&mut self, c: usize) {
        match self.focused {
            Field::Url => self.cursor_url = c,
            Field::ModelId => self.cursor_model = c,
            Field::ApiKey => self.cursor_key = c,
            Field::Preset => self.cursor_preset = c,
        }
    }

    fn insert_char(&mut self, c: char) {
        if self.focused == Field::Preset {
            return;
        }
        // Snapshot `cur` before taking the mutable borrow on the focused
        // field — `self.focused_cursor()` and `self.focused_text_mut()` would
        // otherwise conflict.
        let cur = self.focused_cursor();
        let buf = self.focused_text_mut().expect("text field focused");
        let cur = cur.min(buf.len());
        buf.insert(cur, c);
        let new_cur = cur + c.len_utf8();
        // NLL releases `buf` here; the next line needs a fresh `&mut self`.
        self.set_focused_cursor(new_cur);
        self.error.clear();
    }

    fn backspace(&mut self) {
        if self.focused == Field::Preset {
            return;
        }
        let cur = self.focused_cursor();
        if cur == 0 {
            return;
        }
        let buf = self.focused_text_mut().expect("text field focused");
        // Find the previous char boundary.
        let mut start = cur - 1;
        while !buf.is_char_boundary(start) {
            start -= 1;
        }
        buf.replace_range(start..cur, "");
        self.set_focused_cursor(start);
        self.error.clear();
    }

    fn delete_forward(&mut self) {
        if self.focused == Field::Preset {
            return;
        }
        let cur = self.focused_cursor();
        let buf = self.focused_text_mut().expect("text field focused");
        if cur >= buf.len() {
            return;
        }
        let mut end = cur + 1;
        while end < buf.len() && !buf.is_char_boundary(end) {
            end += 1;
        }
        buf.replace_range(cur..end, "");
        self.error.clear();
    }

    fn move_cursor_left(&mut self) {
        let cur = self.focused_cursor();
        if cur == 0 {
            return;
        }
        let mut prev = cur - 1;
        if self.focused != Field::Preset {
            let buf = self.focused_text_mut_cloned();
            while prev > 0 && !buf.is_char_boundary(prev) {
                prev -= 1;
            }
        }
        self.set_focused_cursor(prev);
    }

    fn move_cursor_right(&mut self) {
        let cur = self.focused_cursor();
        let len = match self.focused {
            Field::Preset => PRESETS.len(),
            Field::Url => self.base_url.len(),
            Field::ModelId => self.model_id.len(),
            Field::ApiKey => self.api_key.len(),
        };
        if cur >= len {
            return;
        }
        let mut next = cur + 1;
        if self.focused != Field::Preset {
            let buf = self.focused_text_mut_cloned();
            while next < buf.len() && !buf.is_char_boundary(next) {
                next += 1;
            }
        }
        self.set_focused_cursor(next);
    }

    fn move_cursor_home(&mut self) {
        self.set_focused_cursor(0);
    }

    fn move_cursor_end(&mut self) {
        let len = match self.focused {
            Field::Preset => PRESETS.len().saturating_sub(1),
            Field::Url => self.base_url.len(),
            Field::ModelId => self.model_id.len(),
            Field::ApiKey => self.api_key.len(),
        };
        self.set_focused_cursor(len);
    }

    /// Internal helper: clone the currently-focused text field's buffer so
    /// `move_cursor_*` can find char boundaries without holding a mutable
    /// borrow across the cursor update.
    fn focused_text_mut_cloned(&mut self) -> String {
        match self.focused {
            Field::Url => self.base_url.clone(),
            Field::ModelId => self.model_id.clone(),
            Field::ApiKey => self.api_key.clone(),
            Field::Preset => String::new(),
        }
    }

    fn focus_next(&mut self) {
        self.focused = self.focused.next();
    }

    fn focus_prev(&mut self) {
        self.focused = self.focused.prev();
    }
}

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Handle a single key event. Returns `WizardOutcome` so the caller (the
/// modal dispatcher in `app/modals.rs`) can close the modal, submit the
/// form, or just redraw.
pub fn handle_wizard_key(state: &mut ConnectWizardState, key: &KeyEvent) -> WizardOutcome {
    // Global shortcuts first: Esc closes, Enter submits (when valid), Tab
    // moves focus forward, BackTab moves backward.
    match key.code {
        KeyCode::Esc => return WizardOutcome::Closed,
        KeyCode::Enter => {
            if let Some(result) = state.validate_and_build() {
                return WizardOutcome::Submitted(result);
            }
            return WizardOutcome::Changed;
        }
        KeyCode::Tab => {
            state.focus_next();
            return WizardOutcome::Changed;
        }
        KeyCode::BackTab => {
            state.focus_prev();
            return WizardOutcome::Changed;
        }
        _ => {}
    }

    // Preset selector handles Up/Down/Enter/Home/End when focused.
    if state.focused == Field::Preset {
        match key.code {
            KeyCode::Up => {
                if state.preset_idx > 0 {
                    state.set_preset(state.preset_idx - 1);
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Down => {
                if state.preset_idx + 1 < PRESETS.len() {
                    state.set_preset(state.preset_idx + 1);
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::NONE) => {
                if state.preset_idx + 1 < PRESETS.len() {
                    state.set_preset(state.preset_idx + 1);
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::NONE) => {
                if state.preset_idx > 0 {
                    state.set_preset(state.preset_idx - 1);
                }
                return WizardOutcome::Changed;
            }
            KeyCode::Home => {
                state.preset_idx = 0;
                return WizardOutcome::Changed;
            }
            KeyCode::End => {
                state.set_preset(PRESETS.len() - 1);
                return WizardOutcome::Changed;
            }
            _ => return WizardOutcome::Unhandled,
        }
    }

    // Text-field handling.
    match key.code {
        KeyCode::Backspace => {
            state.backspace();
            WizardOutcome::Changed
        }
        KeyCode::Delete => {
            state.delete_forward();
            WizardOutcome::Changed
        }
        KeyCode::Left => {
            state.move_cursor_left();
            WizardOutcome::Changed
        }
        KeyCode::Right => {
            state.move_cursor_right();
            WizardOutcome::Changed
        }
        KeyCode::Home => {
            state.move_cursor_home();
            WizardOutcome::Changed
        }
        KeyCode::End => {
            state.move_cursor_end();
            WizardOutcome::Changed
        }
        KeyCode::Char(c) => {
            // Don't insert control chars (other than tab, handled above).
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return WizardOutcome::Unhandled;
            }
            state.insert_char(c);
            WizardOutcome::Changed
        }
        _ => WizardOutcome::Unhandled,
    }
}

// ---- Rendering ----------------------------------------------------------

/// Style tokens. Centralized so the wizard's palette matches the rest of
/// the pager without pulling in the full theme resolver (the modal renders
/// through ModalWindow chrome which has its own theme).
mod style {
    use ratatui::style::Color;
    pub const DIM: Color = Color::Rgb(150, 150, 150);
    pub const ACCENT: Color = Color::Rgb(255, 165, 0);
    pub const ERROR: Color = Color::Rgb(255, 100, 100);
    pub const FIELD_BORDER: Color = Color::Rgb(80, 80, 80);
    pub const FIELD_BORDER_FOCUSED: Color = Color::Rgb(255, 165, 0);
}

/// Render the wizard. The `area` is the full content rectangle the modal
/// dispatcher gives us; we center a fixed-width card inside it.
pub fn render_wizard(buf: &mut Buffer, area: Rect, state: &mut ConnectWizardState) {
    // Clear behind us so any stale glyphs under the modal disappear.
    Clear.render(area, buf);

    // Layout: vertical stack of (title, preset picker, URL field, model field,
    // key field, error/help text, footer). Each section is a single row of
    // borders + content. The preset picker shows up to 5 entries at a time.
    let card_width = 64u16.min(area.width.saturating_sub(4));
    let card_height = 14u16.min(area.height.saturating_sub(2));
    let card = Rect {
        x: area.x + (area.width.saturating_sub(card_width)) / 2,
        y: area.y + (area.height.saturating_sub(card_height)) / 2,
        width: card_width,
        height: card_height,
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(style::FIELD_BORDER_FOCUSED))
        .title(Span::styled(
            " Connect a custom model ",
            Style::default()
                .fg(style::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
    inner(&outer, card, buf);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // preset picker
            Constraint::Length(3), // URL
            Constraint::Length(3), // model id
            Constraint::Length(3), // API key
            Constraint::Length(1), // error row
            Constraint::Length(1), // footer / shortcuts
        ])
        .split(outer.inner(card));

    render_preset(buf, chunks[0], state);
    render_text_field(
        buf,
        chunks[1],
        "Base URL",
        &state.base_url,
        state.cursor_url,
        state.focused == Field::Url,
        false,
    );
    render_text_field(
        buf,
        chunks[2],
        "Model ID",
        &state.model_id,
        state.cursor_model,
        state.focused == Field::ModelId,
        false,
    );
    render_text_field(
        buf,
        chunks[3],
        "API key",
        &state.api_key,
        state.cursor_key,
        state.focused == Field::ApiKey,
        true, // mask
    );

    // Error line.
    let err_style = if state.error.is_empty() {
        Style::default().fg(style::DIM)
    } else {
        Style::default().fg(style::ERROR)
    };
    let err_text = if state.error.is_empty() {
        Span::styled(
            format!(
                "Preset: {} · {}",
                PRESETS[state.preset_idx].0,
                if PRESETS[state.preset_idx].1.is_empty() {
                    "type your own URL"
                } else {
                    PRESETS[state.preset_idx].1
                }
            ),
            Style::default().fg(style::DIM),
        )
    } else {
        Span::styled(state.error.clone(), err_style)
    };
    Paragraph::new(Line::from(err_text))
        .wrap(Wrap { trim: true })
        .render(chunks[4], buf);

    // Footer with shortcuts.
    let footer = match state.focused {
        Field::Preset => "Tab next · Up/Down change preset · Enter connect · Esc cancel",
        _ => "Tab/Shift+Tab next/prev · Enter connect · Esc cancel",
    };
    Paragraph::new(Span::styled(footer, Style::default().fg(style::DIM)))
        .render(chunks[5], buf);
}

fn inner(block: &Block, area: Rect, buf: &mut Buffer) {
    block.render(area, buf);
}

fn render_preset(buf: &mut Buffer, area: Rect, state: &mut ConnectWizardState) {
    let focused = state.focused == Field::Preset;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(
            if focused {
                style::FIELD_BORDER_FOCUSED
            } else {
                style::FIELD_BORDER
            },
        ))
        .title(Span::styled(
            " Preset ",
            Style::default().fg(if focused {
                style::ACCENT
            } else {
                style::DIM
            }),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    // Show up to 3 visible entries: the selected one + one above + one below
    // (when at the boundaries, fall back to neighbors).
    let mut items: Vec<ListItem> = Vec::new();
    for (i, (id, url)) in PRESETS.iter().enumerate() {
        let label = if url.is_empty() {
            format!("{:<14} (custom URL)", id)
        } else {
            format!("{:<14} {}", id, url)
        };
        let style = if i == state.preset_idx {
            Style::default()
                .fg(style::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        items.push(ListItem::new(Span::styled(label, style)));
    }
    let mut list_state = ListState::default();
    list_state.select(Some(state.preset_idx));
    List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 40, 40))
                .add_modifier(Modifier::BOLD),
        )
        .render(inner, buf, &mut list_state);
}

fn render_text_field(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    value: &str,
    cursor: usize,
    focused: bool,
    mask: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(
            if focused {
                style::FIELD_BORDER_FOCUSED
            } else {
                style::FIELD_BORDER
            },
        ))
        .title(Span::styled(
            format!(" {} ", label),
            Style::default().fg(if focused {
                style::ACCENT
            } else {
                style::DIM
            }),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    // Mask API keys.
    let display: String = if mask && !value.is_empty() {
        "\u{2022}".repeat(value.chars().count())
    } else {
        value.to_string()
    };

    // Compose the visible text plus a "block" cursor when focused.
    let mut spans: Vec<Span> = Vec::new();
    let cursor = cursor.min(display.len());
    if focused {
        spans.push(Span::styled(
            display[..cursor].to_string(),
            Style::default().fg(Color::White),
        ));
        spans.push(Span::styled(
            "\u{2588}".to_string(),
            Style::default()
                .fg(style::ACCENT)
                .add_modifier(Modifier::SLOW_BLINK),
        ));
        spans.push(Span::styled(
            display[cursor..].to_string(),
            Style::default().fg(Color::White),
        ));
    } else {
        spans.push(Span::styled(display, Style::default().fg(Color::White)));
    }

    Paragraph::new(Line::from(spans))
        .wrap(Wrap { trim: false })
        .render(inner, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn kc(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn default_state_uses_first_preset_and_prefilled_url() {
        let s = ConnectWizardState::default();
        assert_eq!(s.preset_idx, 0);
        assert_eq!(s.base_url, "https://api.openai.com/v1");
        assert_eq!(s.focused, Field::Preset);
        assert!(s.error.is_empty());
    }

    #[test]
    fn switching_preset_updates_url_when_url_was_unedited() {
        let mut s = ConnectWizardState::default();
        s.set_preset(3); // xai
        assert_eq!(s.current_preset_id(), "xai");
        assert_eq!(s.base_url, "https://api.x.ai/v1");
    }

    #[test]
    fn switching_preset_does_not_overwrite_user_typed_url() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::Url;
        s.insert_char('h');
        s.insert_char('t');
        s.insert_char('t');
        s.insert_char('p');
        // base_url now starts with "http"; the URL no longer matches the
        // OpenAI preset URL, so further preset switches must not overwrite.
        s.set_preset(2); // anthropic
        assert_eq!(s.current_preset_id(), "anthropic");
        assert!(
            s.base_url.starts_with("http"),
            "user-typed URL must survive preset change, got {:?}",
            s.base_url
        );
    }

    #[test]
    fn custom_preset_clears_url() {
        let mut s = ConnectWizardState::default();
        s.set_preset(9); // custom (last)
        assert_eq!(s.current_preset_id(), "custom");
        assert_eq!(s.base_url, "");
    }

    #[test]
    fn validation_requires_model_id() {
        let mut s = ConnectWizardState::default();
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("Model ID"));
        assert_eq!(s.focused, Field::ModelId);
    }

    #[test]
    fn validation_requires_url_for_custom_preset() {
        let mut s = ConnectWizardState::default();
        s.model_id = "m".into();
        s.set_preset(9); // custom
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("Base URL"));
        assert_eq!(s.focused, Field::Url);
    }

    #[test]
    fn validation_requires_api_key() {
        let mut s = ConnectWizardState::default();
        s.model_id = "m".into();
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("API key"));
        assert_eq!(s.focused, Field::ApiKey);
    }

    #[test]
    fn validation_rejects_non_http_url() {
        let mut s = ConnectWizardState::default();
        s.model_id = "m".into();
        s.api_key = "sk".into();
        s.base_url = "ftp://nope".into();
        let res = s.validate_and_build();
        assert!(res.is_none());
        assert!(s.error.contains("http"));
    }

    #[test]
    fn validation_passes_for_openai_preset() {
        let mut s = ConnectWizardState::default();
        s.model_id = "gpt-5.6-luna".into();
        s.api_key = "sk-test".into();
        let res = s.validate_and_build().expect("valid");
        assert_eq!(res.provider, "openai");
        assert_eq!(res.model_id, "gpt-5.6-luna");
        assert_eq!(res.base_url, "https://api.openai.com/v1");
        assert!(!res.injects_think_tags);
    }

    #[test]
    fn minimax_preset_sets_injects_think_tags() {
        let mut s = ConnectWizardState::default();
        s.set_preset(7); // minimax_cn
        s.model_id = "MiniMax-M3".into();
        s.api_key = "sk-mm".into();
        let res = s.validate_and_build().expect("valid");
        assert_eq!(res.provider, "minimax_cn");
        assert!(res.injects_think_tags);
    }

    #[test]
    fn tab_cycles_through_fields() {
        let mut s = ConnectWizardState::default();
        assert_eq!(s.focused, Field::Preset);
        s.focus_next();
        assert_eq!(s.focused, Field::Url);
        s.focus_next();
        assert_eq!(s.focused, Field::ModelId);
        s.focus_next();
        assert_eq!(s.focused, Field::ApiKey);
        s.focus_next();
        // Wraps back to Preset.
        assert_eq!(s.focused, Field::Preset);
    }

    #[test]
    fn backtab_cycles_back() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ApiKey;
        s.focus_prev();
        assert_eq!(s.focused, Field::ModelId);
    }

    #[test]
    fn typing_inserts_chars_at_cursor() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ModelId;
        s.insert_char('a');
        s.insert_char('b');
        s.insert_char('c');
        assert_eq!(s.model_id, "abc");
        assert_eq!(s.cursor_model, 3);
    }

    #[test]
    fn backspace_removes_previous_char() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::ModelId;
        s.insert_char('a');
        s.insert_char('b');
        s.backspace();
        assert_eq!(s.model_id, "a");
        assert_eq!(s.cursor_model, 1);
    }

    #[test]
    fn esc_closes() {
        let mut s = ConnectWizardState::default();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Esc));
        assert_eq!(out, WizardOutcome::Closed);
    }

    #[test]
    fn enter_with_valid_form_submits() {
        let mut s = ConnectWizardState::default();
        s.model_id = "m".into();
        s.api_key = "sk".into();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Enter));
        match out {
            WizardOutcome::Submitted(r) => {
                assert_eq!(r.model_id, "m");
                assert_eq!(r.api_key, "sk");
                assert_eq!(r.provider, "openai");
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[test]
    fn enter_with_invalid_form_sets_error() {
        let mut s = ConnectWizardState::default();
        let out = handle_wizard_key(&mut s, &k(KeyCode::Enter));
        assert_eq!(out, WizardOutcome::Changed);
        assert!(!s.error.is_empty());
    }

    #[test]
    fn preset_arrows_navigate() {
        let mut s = ConnectWizardState::default();
        handle_wizard_key(&mut s, &k(KeyCode::Down));
        assert_eq!(s.preset_idx, 1);
        handle_wizard_key(&mut s, &k(KeyCode::Up));
        assert_eq!(s.preset_idx, 0);
    }

    #[test]
    fn preset_vim_keys_navigate() {
        let mut s = ConnectWizardState::default();
        handle_wizard_key(&mut s, &kc('j'));
        assert_eq!(s.preset_idx, 1);
        handle_wizard_key(&mut s, &kc('k'));
        assert_eq!(s.preset_idx, 0);
    }

    #[test]
    fn typing_in_text_field_does_not_change_preset() {
        let mut s = ConnectWizardState::default();
        s.focused = Field::Url;
        s.insert_char('x');
        assert_eq!(s.preset_idx, 0);
    }
}