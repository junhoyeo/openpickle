//! Line breaking that keeps Korean words whole.
//!
//! GPUI may break a line between any two Hangul syllables. The design (and Korean typography in
//! general) breaks only at spaces. `keep_words` therefore decides the breaks itself and turns the
//! space at each break into a newline, which leaves every byte offset, and so every styled range,
//! where it was.

use std::{cell::RefCell, collections::HashMap};

use gpui::{Pixels, SharedString, TextRun, Window, font, px};

use crate::theme;

thread_local! {
    /// Advance of each character in the body font, measured the way GPUI's own wrapper does.
    static WIDTHS: RefCell<HashMap<char, f32>> = RefCell::new(HashMap::new());
}

/// Breaks body text (the 15 pt system font) so that no line is wider than `wrap_width`.
pub fn keep_words(text: &str, wrap_width: Pixels, window: &Window) -> SharedString {
    WIDTHS.with_borrow_mut(|widths| {
        let width_of = |c: char| {
            *widths.entry(c).or_insert_with(|| {
                let mut buffer = [0; 4];
                let text = c.encode_utf8(&mut buffer);
                let run = TextRun {
                    len: text.len(),
                    font: font(theme::FONT),
                    color: theme::color(theme::TEXT),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                f32::from(
                    window
                        .text_system()
                        .layout_line(text, theme::BODY, &[run], None)
                        .width,
                )
            })
        };
        // One pixel of slack keeps rounding from pushing a full line over the edge.
        break_lines(text, f32::from(wrap_width - px(1.)), width_of).into()
    })
}

/// [`keep_words`] for text of another size than the body's. Character widths grow with the
/// size, so the room is scaled instead. Two percent are held back for heavier weights.
pub fn keep_words_at(
    text: &str,
    size: Pixels,
    wrap_width: Pixels,
    window: &Window,
) -> SharedString {
    keep_words(text, wrap_width * (theme::BODY / size) * 0.98, window)
}

fn break_lines(text: &str, wrap_width: f32, mut width_of: impl FnMut(char) -> f32) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut line = 0.;
        for (index, word) in paragraph.split(' ').enumerate() {
            let word_width: f32 = word.chars().map(&mut width_of).sum();
            if index > 0 {
                let space = width_of(' ');
                // A word wider than the whole line gets a line to itself; GPUI then breaks it
                // wherever it has to.
                if line + space + word_width > wrap_width || line > wrap_width {
                    out.push('\n');
                    line = 0.;
                } else {
                    out.push(' ');
                    line += space;
                }
            }
            out.push_str(word);
            line += word_width;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::break_lines;

    /// Every character is 10 wide.
    fn wrap(text: &str, width: f32) -> String {
        break_lines(text, width, |_| 10.)
    }

    #[test]
    fn breaks_only_at_spaces_and_keeps_byte_offsets() {
        let text = "취소표 감시는 계속 돌려둘게요";
        let wrapped = wrap(text, 80.);
        assert_eq!(wrapped, "취소표 감시는\n계속 돌려둘게요");
        assert_eq!(wrapped.len(), text.len());
    }

    #[test]
    fn keeps_existing_newlines_and_isolates_overlong_words() {
        assert_eq!(wrap("가 나\n\n다 라", 100.), "가 나\n\n다 라");
        assert_eq!(
            wrap("a https://example.com/long b c", 50.),
            "a\nhttps://example.com/long\nb c"
        );
    }
}
