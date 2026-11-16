//! Colors and sizes of the chat screen, read off the Figma frames (440 pt wide, dark).

use gpui::{Hsla, Pixels, Rgba, px, rgb, rgba};

pub const PAGE: u32 = 0x101113;
pub const SURFACE: u32 = 0x25272d;
pub const SURFACE_INSET: u32 = 0x1b1d21;
pub const CHROME: u32 = 0x131414;
pub const CHROME_SELECTED: u32 = 0x353536;
pub const HAIRLINE: u32 = 0x38393b;
pub const TEXT: u32 = 0xf4f5f7;
pub const TEXT_MUTED: u32 = 0xaeb1b9;
pub const TEXT_FAINT: u32 = 0x9195a1;
pub const PLACEHOLDER: u32 = 0x545457;
pub const LINK: u32 = 0x82a8f8;
pub const MINE: u32 = 0x9de68d;
pub const ON_MINE: u32 = 0x000000;
pub const DANGER: u32 = 0xff6b6b;

/// The system font; Hangul falls back to the system's Korean face.
pub const FONT: &str = ".SystemUIFont";

pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn alpha(hex: u32, alpha: f32) -> Rgba {
    rgba((hex << 8) | (alpha.clamp(0., 1.) * 255.) as u32)
}

/// Body text: 15 pt on a 20 pt line.
pub const BODY: Pixels = px(15.);
pub const BODY_LINE: Pixels = px(20.);
/// Card labels and subtitles: 12 pt on a 16 pt line.
pub const SMALL: Pixels = px(12.);
pub const SMALL_LINE: Pixels = px(16.);
/// Header labels: 16 pt on a 22 pt line.
pub const LABEL: Pixels = px(16.);
pub const LABEL_LINE: Pixels = px(22.);

/// The design column. Wider windows keep the conversation this wide and centered.
pub const COLUMN_MAX: Pixels = px(720.);
pub const GUTTER: Pixels = px(16.);
pub const BUBBLE_RADIUS: Pixels = px(21.);
pub const CARD_RADIUS: Pixels = px(26.);
/// Corners where two bubbles of one group meet.
pub const JOINED_RADIUS: Pixels = px(6.);
/// Gap between bubbles of one group, and between groups.
pub const JOINED_GAP: Pixels = px(2.);
pub const GROUP_GAP: Pixels = px(12.);

/// Space the header and the bottom bar cover; the list scrolls underneath both.
pub const HEADER_HEIGHT: Pixels = px(132.);
pub const BOTTOM_BAR_HEIGHT: Pixels = px(146.);
