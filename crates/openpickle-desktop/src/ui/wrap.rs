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

/// Breaks text of `size` the way the phone app's own text was seen to: between words, and
/// inside a word where Hangul meets a Latin letter or a digit, so "안내는 2번." may end a line
/// with the "2". Only the last word of a paragraph was never broken there. Such a break adds a
/// newline, which moves the byte offsets after it: this is for text without styled ranges. A
/// line may fill the width to the last pixel, so the text must be drawn without wrapping of
/// its own.
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

/// Fills each line with words up to `wrap_width`. With `scripts`, a word that does not fit may
/// leave its start on the line, see [`script_break`].
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
        let mut words = paragraph.split(' ').enumerate().peekable();
        while let Some((index, word)) = words.next() {
            let word_width: f32 = word.chars().map(&mut width_of).sum();
            if index > 0 {
                let space = width_of(' ');
                // A word wider than the whole line gets a line to itself; GPUI then breaks it
                // wherever it has to.
                if line + space + word_width > wrap_width || line > wrap_width {
                    let room = wrap_width - line - space;
                    // The last word of a paragraph stays whole.
                    let at = (scripts && words.peek().is_some())
                        .then(|| script_break(word, room, &mut width_of))
                        .flatten();
                    if let Some(at) = at {
                        out.push(' ');
                        out.push_str(&word[..at]);
                        out.push('\n');
                        out.push_str(&word[at..]);
                        line = word[at..].chars().map(&mut width_of).sum();
                        continue;
                    }
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

/// Where the phone app breaks a word that does not fit in `room`: at the last place in it
/// where Hangul meets a Latin letter or a digit and what stands before fits ("통지는 2" /
/// "번.", "최근 AB" / "마트를"). A word without such a place stays whole.
fn script_break(word: &str, room: f32, mut width_of: impl FnMut(char) -> f32) -> Option<usize> {
    let hangul = |c: char| matches!(c, '\u{ac00}'..='\u{d7a3}');
    let mut width = 0.;
    let mut found = None;
    let mut before: Option<char> = None;
    for (at, c) in word.char_indices() {
        if let Some(before) = before
            && before.is_alphanumeric()
            && c.is_alphanumeric()
            && hangul(before) != hangul(c)
        {
            found = Some(at);
        }
        width += width_of(c);
        if width > room {
            break;
        }
        before = Some(c);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::break_lines;

    /// Every character is 10 wide.
    fn wrap(text: &str, width: f32) -> String {
        break_lines(text, width, |_| 10., false)
    }

    #[test]
    fn a_line_of_a_call_may_end_inside_a_word_where_the_scripts_meet() {
        let wrap = |text: &str, width: f32| break_lines(text, width, |_| 10., true);
        // After the digit, or after the Latin letters, also when more of the word would fit.
        assert_eq!(wrap("안내는 2번. 다음", 50.), "안내는 2\n번. 다음");
        assert_eq!(wrap("최근 AB마트를 찾는", 50.), "최근 AB\n마트를\n찾는");
        assert_eq!(
            wrap("도우미는 AB마트로 간다", 80.),
            "도우미는 AB\n마트로 간다"
        );
        // The last word of a paragraph stays whole, and so does a word of one script.
        assert_eq!(wrap("도우미, AB마트입니다.", 80.), "도우미,\nAB마트입니다.");
        assert_eq!(wrap("가나다 라마바사", 60.), "가나다\n라마바사");
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
