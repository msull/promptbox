//! Pure text-range helpers for transcript operations. All offsets are byte
//! offsets into `text`; results are always on char boundaries.
//!
//! "Last sentence" and "last paragraph" are relative to a cursor: the unit
//! that ends at or contains the cursor, which for dictation is the thing
//! most recently spoken.

use std::ops::Range;

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '!' | '?') || is_fullwidth_terminator(c)
}

/// Full-width terminators end a sentence without a following space.
fn is_fullwidth_terminator(c: char) -> bool {
    matches!(c, '。' | '！' | '？')
}

/// Trailing whitespace before `end` is skipped; returns the new end.
fn trim_end_ws(text: &str, end: usize) -> usize {
    text[..end].trim_end().len()
}

/// Range to remove for "delete last sentence" at `cursor`: from the start
/// of the sentence that ends at or contains the cursor, through the
/// cursor, plus the whitespace separating it from the previous sentence.
/// Returns `None` when there is nothing before the cursor.
#[must_use]
pub fn last_sentence_range(text: &str, cursor: usize) -> Option<Range<usize>> {
    let cursor = cursor.min(text.len());
    let end = trim_end_ws(text, cursor);
    if end == 0 {
        return None;
    }
    // Skip the terminator(s) of this sentence, then look for the previous
    // terminator followed by whitespace, or a paragraph break.
    let body_end = text[..end].trim_end_matches(is_terminator).len();
    let mut start = 0;
    let mut last_was_term = false;
    for (i, c) in text[..body_end].char_indices() {
        if c == '\n' {
            start = i + 1;
            last_was_term = false;
        } else if is_fullwidth_terminator(c) || (c.is_whitespace() && last_was_term) {
            start = i + c.len_utf8();
            last_was_term = false;
        } else {
            last_was_term = is_terminator(c);
        }
    }
    // Also swallow whitespace between the previous sentence and this one
    // so no dangling space is left behind.
    let start = trim_end_ws(text, start);
    Some(start..cursor)
}

/// Range to remove for "delete last paragraph" at `cursor`: the paragraph
/// containing or ending at the cursor plus the blank-line separator before
/// it. Paragraphs are separated by a newline followed by optional spaces
/// and another newline.
#[must_use]
pub fn last_paragraph_range(text: &str, cursor: usize) -> Option<Range<usize>> {
    let cursor = cursor.min(text.len());
    let end = trim_end_ws(text, cursor);
    if end == 0 {
        return None;
    }
    let before = &text[..end];
    let start = paragraph_separator_end(before).unwrap_or(0);
    let start = trim_end_ws(text, start);
    Some(start..cursor)
}

/// Byte index just after the last paragraph separator in `s`, if any.
fn paragraph_separator_end(s: &str) -> Option<usize> {
    let mut best = None;
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\r') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'\n' {
                // Consume the whole run of blank lines.
                let mut k = j + 1;
                while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                    k += 1;
                }
                best = Some(k);
                i = k;
                continue;
            }
        }
        i += 1;
    }
    best
}

/// Text to insert at `cursor` so a new paragraph begins there: enough
/// newlines to leave exactly one blank line after whatever precedes.
#[must_use]
pub fn paragraph_break_for(text: &str, cursor: usize) -> &'static str {
    let before = &text[..cursor.min(text.len())];
    if before.is_empty() || before.ends_with("\n\n") {
        ""
    } else if before.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_sentence_cases() {
        let cases: &[(&str, &str, Option<Range<usize>>)] = &[
            ("empty", "", None),
            ("only spaces", "   ", None),
            ("single sentence", "Run the tests.", Some(0..14)),
            ("two sentences", "One. Two.", Some(4..9)),
            ("swallows separating space", "One.  Two.", Some(4..10)),
            (
                "trailing space after cursor unit",
                "One. Two. ",
                Some(4..10),
            ),
            ("unterminated last", "One. two words", Some(4..14)),
            ("question and bang", "Really? Yes!", Some(7..12)),
            (
                "ellipsis counts as one terminator",
                "Wait... Go.",
                Some(7..11),
            ),
            ("newline is a boundary", "One\nTwo.", Some(3..8)),
            ("unicode before", "Café. Naïve.", Some(6..14)),
            ("cjk terminator", "一。二。", Some(6..12)),
        ];
        for (name, text, want) in cases {
            let got = last_sentence_range(text, text.len());
            assert_eq!(got, *want, "{name}");
            if let Some(r) = got {
                assert!(
                    text.is_char_boundary(r.start) && text.is_char_boundary(r.end),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn last_sentence_respects_cursor_position() {
        let text = "One. Two. Three.";
        assert_eq!(last_sentence_range(text, 9), Some(4..9)); // after "Two."
        assert_eq!(last_sentence_range(text, 7), Some(4..7)); // inside "Two."
        assert_eq!(last_sentence_range(text, 4), Some(0..4)); // right after "One."
        assert_eq!(last_sentence_range(text, 100), Some(9..16)); // clamped
    }

    #[test]
    fn last_paragraph_cases() {
        let cases: &[(&str, &str, Option<Range<usize>>)] = &[
            ("empty", "", None),
            ("one paragraph", "A. B.", Some(0..5)),
            ("two paragraphs", "First.\n\nSecond.", Some(6..15)),
            ("blank line with spaces", "First.\n  \nSecond.", Some(6..17)),
            ("multiple blank lines", "A\n\n\n\nB", Some(1..6)),
            ("single newline is same paragraph", "A\nB", Some(0..3)),
            ("cursor mid paragraph", "First.\n\nSec", Some(6..11)),
        ];
        for (name, text, want) in cases {
            assert_eq!(last_paragraph_range(text, text.len()), *want, "{name}");
        }
    }

    #[test]
    fn paragraph_break_avoids_extra_blank_lines() {
        assert_eq!(paragraph_break_for("", 0), "");
        assert_eq!(paragraph_break_for("A", 1), "\n\n");
        assert_eq!(paragraph_break_for("A\n", 2), "\n");
        assert_eq!(paragraph_break_for("A\n\n", 3), "");
        assert_eq!(paragraph_break_for("A\n\nB", 1), "\n\n");
    }
}
