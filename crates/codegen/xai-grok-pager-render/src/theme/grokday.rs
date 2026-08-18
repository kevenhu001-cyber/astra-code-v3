//! Astra Day theme — white/black canvas with a single pixel-orange accent.
//!
//! Light counterpart to Astra Night. Backgrounds are near-white with a single
//! pure-black fg, and accent colors use the same `#FF6A00` family as
//! Astra Night so dark/light polarity is the only difference between the two.

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds (white-based) ──────────────────────────────────────
    pub const BG: Color = rgb(255, 255, 255); // #FFFFFF — pure white canvas
    pub const BG_DARK: Color = rgb(245, 245, 245); // #F5F5F5
    pub const BG_STORM_DARK: Color = rgb(238, 238, 238); // #EEEEEE
    pub const BG_STORM: Color = rgb(250, 250, 250); // #FAFAFA — main bg
    pub const BG_HIGHLIGHT: Color = rgb(232, 232, 232); // #E8E8E8 — highlight bg

    // ── Text / grays ────────────────────────────────────────────────────
    pub const FG: Color = rgb(0, 0, 0); //           #000000 — primary text
    pub const FG_DARK: Color = rgb(38, 38, 38); //    #262626 — secondary text
    pub const FG_GUTTER: Color = rgb(170, 170, 170); // #AAAAAA — dim
    pub const COMMENT: Color = rgb(118, 118, 118); // #767676 — muted
    pub const DARK3: Color = rgb(140, 140, 140); //   #8C8C8C — medium gray
    pub const DARK5: Color = rgb(98, 98, 98); //      #626262 — bright gray

    // ── Accent colors (deepened for white-bg contrast) ──────────────────
    pub const ORANGE: Color = rgb(204, 85, 0); // #CC5500 — deeper orange reads on white
    pub const ORANGE_DIM: Color = rgb(150, 60, 0); // darker still for muted surfaces

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
    pub const YELLOW: Color = ORANGE_DIM;

    pub const RED_LIGHT: Color = rgb(255, 235, 220); // orange-tinted diff delete bg
    pub const GREEN_LIGHT: Color = rgb(255, 235, 220); // orange-tinted diff insert bg
}
use palette::*;

impl Theme {
    pub const fn grokday() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(228, 228, 228),
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(208, 208, 208),
            bg_terminal: BG,

            accent_user: FG_DARK,
            accent_assistant: MAGENTA,
            accent_thinking: MAGENTA,
            accent_tool: DARK5,
            accent_system: BLUE,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: MAGENTA,
            accent_skill: BLUE,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(165, 165, 165), // #a5a5a5 — slightly darker than FG_GUTTER
            gray: COMMENT,
            gray_bright: DARK5,

            command: YELLOW,
            path: ORANGE,
            running: CYAN,
            warning: YELLOW,

            fuzzy_accent: BLUE,

            accent_plan: ORANGE_DIM, // deep orange (readable on light bg)

            accent_verify: ORANGE_DIM, // orange-tinted verify accent

            accent_remember: ORANGE, // orange remember accent

            selection_border: rgb(185, 185, 190),
            prompt_border: rgb(200, 200, 205), // #C8C8CD — dimmer prompt chrome
            prompt_border_active: rgb(165, 165, 175), // #A5A5AF — darker (more apparent) when focused
            hover_border: rgb(212, 212, 216),

            accent_model: TEAL,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: BG_HIGHLIGHT,

            diff_delete_bg: RED_LIGHT,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_LIGHT,
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,

            bg_visual: rgb(198, 198, 198),

            paste_bg: BG_HIGHLIGHT,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: TEAL,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: BLUE,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: PURPLE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: DARK5,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: COMMENT,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: DARK3,
            md_heading_h6_mod: Modifier::empty(),
            md_code: BLUE1,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK,
            md_muted: COMMENT,
            md_code_bg: rgb(228, 228, 228),
            md_text: FG_DARK,
            link_fg: BLUE, // #2F64D2 -- deep blue for light bg
        }
    }
}
