//! The byte budget of a tool result, shared by every tool.
//!
//! The reader of a tool result is a language model that pays for every
//! byte. A tool therefore hands its answer to [`fit`] as an [`Answer`]: the
//! whole answer, and where it has one, a smaller answer that stands for it.
//! [`fit`] returns the whole answer when it is inside the budget. Otherwise
//! it returns the smaller one, says how large the whole answer is, and says
//! how to get the rest.
//!
//! The size of a result is the number of bytes of its text plus the number
//! of bytes of its `structuredContent` as compact JSON ([`size`]). The
//! JSON-RPC line is longer by the escaping of the text.

use buzzard_lint::budget;
use serde_json::{json, Map, Value};

/// The smallest accepted `max_bytes`. The notice of a reduced answer always
/// fits in it.
pub(crate) const MIN_MAX_BYTES: usize = 1024;

/// What a successful tool call returns.
#[derive(Debug)]
pub(crate) struct Output {
    pub text: String,
    /// The machine-readable form of the same answer, always a JSON object.
    pub structured: Value,
}

/// The size of a result that [`fit`] compares with the budget.
pub(crate) fn size(output: &Output) -> u64 {
    (output.text.len() as u64).saturating_add(budget::json_bytes(&output.structured, false))
}

/// The answer of a tool before the budget is applied.
#[derive(Debug)]
pub(crate) struct Answer {
    /// The whole answer. `None` when the tool saw that it is larger than
    /// the budget and did not build it.
    full: Option<Output>,
    /// The size of the whole answer ([`size`]).
    full_bytes: u64,
    /// The text of a smaller answer that stands for the whole one. Without
    /// one, the text of the whole answer is cut.
    summary_text: Option<String>,
    /// The `structuredContent` of the smaller answer. Without one, a
    /// reduced answer holds its `budget` entry only.
    summary_structured: Option<Value>,
    /// One sentence on how to get what a reduced answer leaves out.
    more: &'static str,
    /// The caller asked for everything, so only an explicit `max_bytes`
    /// bounds the answer.
    unbounded: bool,
}

impl Answer {
    /// An answer without a smaller form. When it is over the budget, its
    /// text is cut and its `structuredContent` is dropped.
    pub(crate) fn whole(output: Output, more: &'static str) -> Self {
        Self {
            full_bytes: size(&output),
            full: Some(output),
            summary_text: None,
            summary_structured: None,
            more,
            unbounded: false,
        }
    }

    /// The smaller answer to return when the whole one is over the budget.
    pub(crate) fn or(mut self, summary: Output) -> Self {
        self.summary_text = Some(summary.text);
        self.summary_structured = Some(summary.structured);
        self
    }

    /// As [`Answer::or`], for a smaller answer that has the text of the
    /// whole one, cut to the budget.
    pub(crate) fn or_structured(mut self, structured: Value) -> Self {
        self.summary_structured = Some(structured);
        self
    }

    /// The smaller answer alone, for a whole answer of `full_bytes` bytes
    /// that was not built.
    pub(crate) fn reduced(summary: Output, full_bytes: u64, more: &'static str) -> Self {
        Self {
            full: None,
            full_bytes,
            summary_text: Some(summary.text),
            summary_structured: Some(summary.structured),
            more,
            unbounded: false,
        }
    }

    /// The caller asked for everything: the default budget does not apply.
    pub(crate) fn unbounded(mut self) -> Self {
        self.unbounded = true;
        self
    }
}

/// Room for the line that says how much of a text was cut
/// ([`budget::Cut::notice`]) and its line breaks.
const CUT_NOTICE_BYTES: usize = 96;

/// Apply the budget. `asked` is the caller's `max_bytes`, and `default` is
/// the budget without one.
///
/// The result is never larger than the budget, as long as the budget is at
/// least [`MIN_MAX_BYTES`].
pub(crate) fn fit(answer: Answer, asked: Option<usize>, default: usize) -> Output {
    let max = match asked {
        Some(max) => max,
        None if answer.unbounded => usize::MAX,
        None => default,
    };
    let Answer {
        full,
        full_bytes,
        summary_text,
        summary_structured,
        more,
        ..
    } = answer;
    let full = match full {
        Some(full) if size(&full) <= max as u64 => return full,
        other => other,
    };
    // The whole answer is too large, or was not built.
    let mut text = summary_text
        .or(full.map(|full| full.text))
        .unwrap_or_default();
    let marker = json!({ "max_bytes": max, "full_bytes": full_bytes, "more": more });
    let mut structured = match summary_structured {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    structured.insert("budget".to_owned(), marker.clone());
    let notice = format!(
        "... The whole answer is {full_bytes} bytes, and the limit `max_bytes` is {max}. {more}\n"
    );
    let bytes = |map: &Map<String, Value>| {
        usize::try_from(budget::json_bytes(map, false)).unwrap_or(usize::MAX)
    };
    let mut used = bytes(&structured);
    let fits = used
        .saturating_add(text.len())
        .saturating_add(notice.len())
        .saturating_add(1)
        <= max;
    // A summary that does not fit whole keeps its structured half when
    // that leaves half of the budget for the text.
    if !fits && used > max / 2 {
        structured = Map::new();
        structured.insert("budget".to_owned(), marker);
        used = bytes(&structured);
    }
    let room = max.saturating_sub(used).saturating_sub(notice.len());
    if !fits {
        let cut = budget::cut(&text, room.saturating_sub(CUT_NOTICE_BYTES));
        let line = cut.notice().unwrap_or_default();
        let kept = cut.text.len();
        text.truncate(kept);
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !line.is_empty() {
            text.push_str(&line);
            text.push('\n');
        }
    } else if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&notice);
    Output {
        text,
        structured: Value::Object(structured),
    }
}

/// Cut the message of a failed call to the budget. The message of a
/// failure quotes arguments, which can be thousands of bytes long.
pub(crate) fn fit_message(message: &str, max: usize) -> String {
    let cut = budget::cut(message, max.saturating_sub(CUT_NOTICE_BYTES));
    match cut.notice() {
        None => message.to_owned(),
        Some(notice) => format!("{} {notice}", cut.text.trim_end()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(lines: usize, items: usize) -> Output {
        Output {
            text: "one line of text\n".repeat(lines),
            structured: json!({ "items": vec!["item"; items] }),
        }
    }

    const MORE: &str = "Ask for less.";

    #[test]
    fn an_answer_inside_the_budget_is_returned_unchanged() {
        let whole = output(3, 3);
        let bytes = usize::try_from(size(&whole)).unwrap();
        let fitted = fit(Answer::whole(output(3, 3), MORE), Some(bytes), 10);
        assert_eq!(fitted.text, whole.text);
        assert_eq!(fitted.structured, whole.structured);
        // One byte less, and the answer is reduced.
        let fitted = fit(Answer::whole(output(3, 3), MORE), Some(bytes - 1), 10);
        assert_eq!(fitted.structured["budget"]["full_bytes"], bytes);
    }

    #[test]
    fn the_summary_stands_in_and_names_the_true_size() {
        let whole = output(10_000, 10_000);
        let bytes = size(&whole);
        let answer = Answer::whole(whole, MORE).or(Output {
            text: "10000 items\n".to_owned(),
            structured: json!({ "count": 10_000 }),
        });
        let fitted = fit(answer, None, 4096);
        assert!(size(&fitted) <= 4096);
        assert_eq!(fitted.structured["count"], 10_000);
        assert_eq!(
            fitted.structured["budget"],
            json!({ "max_bytes": 4096, "full_bytes": bytes, "more": MORE })
        );
        assert!(fitted.text.starts_with("10000 items\n"), "{}", fitted.text);
        assert!(fitted.text.contains(&bytes.to_string()), "{}", fitted.text);
        assert!(fitted.text.ends_with("Ask for less.\n"), "{}", fitted.text);
    }

    #[test]
    fn without_a_summary_the_text_is_cut_at_a_line() {
        let fitted = fit(Answer::whole(output(10_000, 10_000), MORE), None, 2048);
        assert!(size(&fitted) <= 2048);
        assert!(fitted.structured.get("items").is_none());
        assert!(fitted.text.starts_with("one line of text\n"));
        assert!(
            fitted.text.contains(" bytes not shown.\n"),
            "{}",
            fitted.text
        );
        for line in fitted
            .text
            .lines()
            .take_while(|line| !line.starts_with("..."))
        {
            assert_eq!(line, "one line of text");
        }
    }

    #[test]
    fn no_budget_however_small_is_exceeded() {
        for max in [MIN_MAX_BYTES, MIN_MAX_BYTES + 1, 1500, 5000, 70_000] {
            for (lines, items) in [(0, 0), (1, 100_000), (100_000, 1), (5000, 5000)] {
                let summary = output(lines / 2, items / 2);
                for answer in [
                    Answer::whole(output(lines, items), MORE),
                    Answer::whole(output(lines, items), MORE).or(summary),
                    Answer::reduced(output(lines, items), u64::MAX, MORE),
                ] {
                    let fitted = fit(answer, Some(max), 10);
                    assert!(size(&fitted) <= max as u64, "{max} {lines} {items}");
                    assert!(fitted.structured.is_object());
                }
            }
        }
    }

    #[test]
    fn an_unbounded_answer_obeys_only_an_explicit_limit() {
        let bytes = size(&output(5000, 5000));
        let whole = || Answer::whole(output(5000, 5000), MORE).unbounded();
        assert_eq!(size(&fit(whole(), None, 2048)), bytes);
        assert!(size(&fit(whole(), Some(2048), 2048)) <= 2048);
    }

    #[test]
    fn a_long_message_is_cut() {
        assert_eq!(fit_message("short", 1024), "short");
        let long = "x".repeat(5000);
        let cut = fit_message(&long, 1024);
        assert!(cut.len() <= 1024, "{}", cut.len());
        assert!(cut.ends_with("bytes not shown."), "{cut}");
    }
}
