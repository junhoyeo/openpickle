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
    // One pixel of slack keeps rounding from pushing a full line over the edge, where GPUI
    // would break it again.
    break_body(text, wrap_width - px(1.), false, window)
}

/// Breaks text of `size` the way the phone app's own text does: between words, and also
/// between a number and the Hangul after it, so "안내는 2번." may end a line with the "2". Such
/// a break adds a newline, which moves the byte offsets after it: this is for text without
/// styled ranges. A line may fill the width to the last pixel, so the text must be drawn
/// without wrapping of its own.
pub fn keep_hangul(text: &str, size: Pixels, wrap_width: Pixels, window: &Window) -> SharedString {
    break_body(text, wrap_width * (theme::BODY / size), true, window)
}

fn break_body(text: &str, wrap_width: Pixels, scripts: bool, window: &Window) -> SharedString {
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
        break_lines(text, f32::from(wrap_width), width_of, scripts).into()
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

/// Fills each line with words up to `wrap_width`. With `scripts`, a word may also be broken
/// between a number and the Hangul after it.
fn break_lines(
    text: &str,
    wrap_width: f32,
    mut width_of: impl FnMut(char) -> f32,
    scripts: bool,
) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut line = 0.;
        for (index, word) in paragraph.split(' ').enumerate() {
            for (part_index, part) in parts(word, scripts).into_iter().enumerate() {
                let width: f32 = part.chars().map(&mut width_of).sum();
                // Only the first part of a word has the space before it.
                let space = if index > 0 && part_index == 0 {
                    width_of(' ')
                } else {
                    0.
                };
                if index > 0 || part_index > 0 {
                    // A word wider than the whole line gets a line to itself; GPUI then
                    // breaks it wherever it has to.
                    if line + space + width > wrap_width || line > wrap_width {
                        out.push('\n');
                        line = 0.;
                    } else if part_index == 0 {
                        out.push(' ');
                        line += space;
                    }
                }
                out.push_str(part);
                line += width;
            }
        }
    }
    out
}

/// The pieces of a word a line may end after: the word itself, or with `scripts` what stands
/// before and after each place where a digit is followed by Hangul.
fn parts(word: &str, scripts: bool) -> Vec<&str> {
    if !scripts {
        return vec![word];
    }
    let mut parts = Vec::new();
    let mut start = 0;
    let mut before = None;
    for (at, c) in word.char_indices() {
        if before.is_some_and(|before: char| before.is_ascii_digit())
            && matches!(c, '\u{ac00}'..='\u{d7a3}')
        {
            parts.push(&word[start..at]);
            start = at;
        }
        before = Some(c);
    }
    parts.push(&word[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::break_lines;

    /// Every character is 10 wide.
    fn wrap(text: &str, width: f32) -> String {
        break_lines(text, width, |_| 10., false)
    }

    #[test]
    fn a_line_of_a_call_may_end_between_a_number_and_its_hangul() {
        let wrap = |text: &str, width: f32| break_lines(text, width, |_| 10., true);
        // "2번." is one word, but the phone app ends the line after its digit.
        assert_eq!(wrap("안내는 2번. 다음", 50.), "안내는 2\n번. 다음");
        // Hangul is not broken inside a word, nor a word where Latin letters meet Hangul.
        assert_eq!(wrap("가나다 라마바사", 60.), "가나다\n라마바사");
        assert_eq!(wrap("도우미, AB마트입니다.", 60.), "도우미,\nAB마트입니다.");
        // Spaces are kept as they were, also two in a row.
        assert_eq!(wrap("가  나", 100.), "가  나");
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
