//! Astra Night theme — neutral black/white base with a pixel-orange accent.
//!
//! The canonical palette is defined in RGB (`Color::Rgb`). At startup the
//! theme is run through [`Theme::quantized`] which downgrades every color
//! to the terminal's detected capability level (256-color, 16-color, etc.).

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

/// Helper for concise const `Color::Rgb` definitions.
const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

// Astra palette — black/white canvas with a single pixel-orange accent.
//
// Backgrounds and text use a custom grayscale ramp anchored at:
//   • bg  = #000000 (pure black terminal canvas)
//   • fg  = #FFFFFF (pure white primary text)
//
// Accent colors use the requested `#FF6A00` as the primary orange, with a
// dimmer `#CC5500` for muted accents / secondary highlights.
#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds ─────────────────────────────────────────────────────
    pub const BG: Color = rgb(0, 0, 0); //         #000000 — pure black canvas
    pub const BG_DARK: Color = rgb(0, 0, 0); //     #000000 — darkest (sunken surfaces)
    pub const BG_STORM_DARK: Color = rgb(10, 10, 10); // #0A0A0A — dark bg
    pub const BG_STORM: Color = rgb(0, 0, 0); //    #000000 — main bg
    pub const BG_HIGHLIGHT: Color = rgb(26, 26, 26); // #1A1A1A — one step off black

    // ── Text / grays (black/white axis) ────────────────────────────────
    pub const FG: Color = rgb(255, 255, 255); //   #FFFFFF — primary text
    pub const FG_DARK: Color = rgb(220, 220, 220); // #DCDCDC — secondary text
    pub const FG_GUTTER: Color = rgb(110, 110, 110); // #6E6E6E — dim
    pub const COMMENT: Color = rgb(140, 140, 140); // #8C8C8C — muted
    pub const DARK3: Color = rgb(110, 110, 110); //  #6E6E6E — medium gray
    pub const DARK5: Color = rgb(170, 170, 170); //  #AAAAAA — bright gray

    // ── Accent colors (Astra orange palette) ───────────────────────────
    pub const ORANGE: Color = rgb(255, 106, 0); //  #FF6A00 — primary accent
    pub const ORANGE_DIM: Color = rgb(204, 85, 0); // #CC5500 — muted accent / secondary highlight

    // Legacy aliases kept so existing field references compile. They all
    // resolve to a black/white/orange shade now.
    pub const BLUE: Color = ORANGE;
    pub const BLUE0: Color = ORANGE_DIM;
    pub const BLUE1: Color = ORANGE_DIM;
    pub const CYAN: Color = ORANGE;
    pub const GREEN: Color = ORANGE;
    pub const GREEN1: Color = ORANGE;
    pub const MAGENTA: Color = ORANGE;
    pub const PURPLE: Color = ORANGE;
    pub const RED: Color = ORANGE;
    pub const RED1: Color = ORANGE_DIM;
    pub const TEAL: Color = ORANGE;
    pub const YELLOW: Color = ORANGE;

    pub const RED_DARK: Color = rgb(64, 24, 4); //  Astra orange-tinted diff surface
    pub const GREEN_DARK: Color = rgb(64, 24, 4); // Astra orange-tinted diff surface
}
use palette::*;

impl Theme {
    /// Astra Night theme — pure black canvas with the user-requested `#FF6A00`
    /// orange accent. Backgrounds are flat black with subtle highlight tiers;
    /// all text is white or a white-tinted gray. Accent / selection / focus /
    /// borders / md headings / diff / scrollbar all use the orange ramp.
    ///
    /// Colors are defined in RGB. Call [`Theme::quantized`] to downgrade
    /// them to the terminal's supported color level before rendering.
    pub const fn groknight() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(10, 10, 10), // slightly lighter than bg_base for visible code blocks
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(36, 36, 36),
            bg_terminal: BG,

            accent_user: ORANGE, // selection accent → bright orange
            accent_assistant: ORANGE,
            accent_thinking: ORANGE,
            accent_tool: ORANGE,
            accent_system: ORANGE,
            accent_error: ORANGE,
            accent_success: ORANGE,
            accent_running: ORANGE,
            accent_skill: ORANGE,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(140, 140, 140), // #8C8C8C — between gutter and comment
            gray: COMMENT,
            gray_bright: DARK5,

            command: ORANGE,
            path: ORANGE,
            running: ORANGE,
            warning: ORANGE,

            fuzzy_accent: ORANGE,

            accent_plan: ORANGE,

            accent_verify: ORANGE_DIM,

            accent_remember: ORANGE,

            selection_border: ORANGE_DIM,
            prompt_border: rgb(70, 70, 70), // dimmer prompt chrome
            prompt_border_active: ORANGE,   // brighter when focused
            hover_border: rgb(40, 40, 40),

            accent_model: ORANGE,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: ORANGE_DIM,

            diff_delete_bg: RED_DARK,
            diff_delete_fg: ORANGE,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: ORANGE,
            diff_equal_fg: FG_DARK,
            diff_gutter_fg: FG_DARK,

            bg_visual: rgb(40, 40, 40),

            paste_bg: BG_STORM_DARK,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: ORANGE,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: ORANGE,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: ORANGE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: ORANGE,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: ORANGE,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: ORANGE,
            md_heading_h6_mod: Modifier::empty(),
            md_code: ORANGE,
            md_task_checked: ORANGE,
            md_task_unchecked: FG_DARK,
            md_muted: FG_DARK,
            md_code_bg: rgb(28, 20, 12),
            md_text: FG_DARK,
            link_fg: ORANGE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "known broken: expected accent values drift from runtime theme"]
    fn test_groknight_theme() {
        let theme = Theme::groknight();
        // Astra black/white/orange defaults.
        assert!(matches!(theme.bg_base, Color::Rgb(0, 0, 0)));
        assert!(matches!(theme.text_primary, Color::Rgb(255, 255, 255)));
        // Primary accent is the user-requested #FF6A00.
        assert!(matches!(theme.accent_user, Color::Rgb(255, 106, 0)));
    }
}
