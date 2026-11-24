//! How large an answer is, and how to keep one inside a limit.
//!
//! The JSON of one block diagram can be over a megabyte. A reader that
//! pays for every byte it is sent, such as a language model, needs to
//! know the size of an answer before it asks for all of it.
//! [`json_bytes`] gives the size of a value's JSON without building the
//! text, and [`cut`] shortens a text to a number of bytes at a line break.

use std::io;

use serde::Serialize;

/// A budget in bytes for one answer to a reader that pays for every byte:
/// 64 KiB, in the order of 16,000 tokens of a language model.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024;

/// Counts the bytes written to it and keeps none.
struct Counter(u64);

impl io::Write for Counter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The number of bytes of `value` as JSON: of `serde_json::to_string`,
/// or of `serde_json::to_string_pretty` when `pretty` is set. A line
/// break that a command prints after the text is not counted.
///
/// The text is not built, so the cost is time and no memory. A value
/// that does not serialize counts as 0 bytes.
pub fn json_bytes<T: Serialize + ?Sized>(value: &T, pretty: bool) -> u64 {
    let mut counter = Counter(0);
    let written = if pretty {
        serde_json::to_writer_pretty(&mut counter, value)
    } else {
        serde_json::to_writer(&mut counter, value)
    };
    match written {
        Ok(()) => counter.0,
        Err(_) => 0,
    }
}

/// A text, or the start of one. See [`cut`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut<'a> {
    /// The part that is kept. It is a prefix of the whole text.
    pub text: &'a str,
    /// The length of the whole text in bytes.
    pub total_bytes: usize,
}

impl Cut<'_> {
    /// How many bytes of the whole text are not kept.
    pub fn omitted_bytes(&self) -> usize {
        self.total_bytes - self.text.len()
    }

    /// Whether the whole text is kept.
    pub fn is_whole(&self) -> bool {
        self.omitted_bytes() == 0
    }

    /// The line that says how much is missing, without a line break.
    /// `None` when the whole text is kept.
    pub fn notice(&self) -> Option<String> {
        (!self.is_whole()).then(|| {
            format!(
                "... {} of {} bytes not shown.",
                self.omitted_bytes(),
                self.total_bytes
            )
        })
    }
}

/// Keep at most `max_bytes` bytes of `text`.
///
/// A text that fits is kept whole. A longer one is cut after the last
/// line break inside the limit, so that no line is shown in part. When
/// the limit holds no whole line, the text is cut at the last character
/// boundary inside the limit. The kept part is always valid text, and it
/// is empty when `max_bytes` is 0.
pub fn cut(text: &str, max_bytes: usize) -> Cut<'_> {
    let total_bytes = text.len();
    if total_bytes <= max_bytes {
        return Cut { text, total_bytes };
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if let Some(line_end) = text[..end].rfind('\n') {
        end = line_end + 1;
    }
    Cut {
        text: &text[..end],
        total_bytes,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_count_is_the_length_of_the_text() {
        let values = [
            json!(null),
            json!({}),
            json!({ "a": [1, 2, { "b": "テスト\n\u{1b}" }], "c": 1.5, "d": "x".repeat(10_000) }),
        ];
        for value in &values {
            assert_eq!(
                json_bytes(value, false),
                serde_json::to_string(value).unwrap().len() as u64
            );
            assert_eq!(
                json_bytes(value, true),
                serde_json::to_string_pretty(value).unwrap().len() as u64
            );
        }
        assert_eq!(json_bytes("a", false), 3);
    }

    #[test]
    fn a_text_that_fits_is_kept_whole() {
        for (text, max) in [("", 0), ("abc", 3), ("abc\n", 100), ("é", 2)] {
            let kept = cut(text, max);
            assert_eq!(kept.text, text);
            assert!(kept.is_whole());
            assert_eq!(kept.notice(), None);
        }
    }

    #[test]
    fn a_longer_text_is_cut_at_a_line_break() {
        let text = "first\nsecond\nthird\n";
        assert_eq!(cut(text, 15).text, "first\nsecond\n");
        assert_eq!(cut(text, 13).text, "first\nsecond\n");
        assert_eq!(cut(text, 12).text, "first\n");
        assert_eq!(cut(text, 6).text, "first\n");
        // No whole line fits.
        assert_eq!(cut(text, 3).text, "fir");
        assert_eq!(cut(text, 0).text, "");
        let kept = cut(text, 12);
        assert_eq!(kept.total_bytes, 19);
        assert_eq!(kept.omitted_bytes(), 13);
        assert_eq!(kept.notice().unwrap(), "... 13 of 19 bytes not shown.");
    }

    #[test]
    fn a_cut_never_splits_a_character_and_never_exceeds_the_limit() {
        let text = "aé\u{1f600}テスト\nb";
        for max in 0..=text.len() + 2 {
            let kept = cut(text, max);
            assert!(kept.text.len() <= max, "{max}");
            assert!(text.starts_with(kept.text), "{max}");
            assert_eq!(kept.total_bytes, text.len());
            assert_eq!(kept.is_whole(), max >= text.len());
        }
        assert_eq!(cut("éé", 3).text, "é");
        assert_eq!(cut("éé", 1).text, "");
    }
}
