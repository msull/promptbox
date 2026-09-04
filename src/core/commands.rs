//! Deterministic voice-command extraction. A trigger word ("Zevro") opens
//! the command channel; the phrase after it is matched against a fixed
//! grammar. Commands are extracted only from *final* recognition text, so
//! successive provisional hypotheses can never fire one twice. Everything
//! outside trigger + command phrase stays dictation, with its original
//! casing and punctuation. Words after a trigger that match nothing are
//! dropped and reported, never dictated: the speaker meant a command.
//!
//! Recognition is imperfect. Command words are compared with a
//! one-substitution tolerance for words of four letters or more ("sand"
//! is "send"). The trigger is matched on a phonetic key (b/v merged,
//! vowels dropped, doubled letters collapsed), optionally across two
//! adjacent tokens, because real speech produced "Zebro", "Zebra",
//! "Zev Bro" and "zebbro" for "Zevro". "zero" keys to "zr" and never
//! matches. An utterance that *starts* with the trigger is treated as a
//! command only: words after the command phrase are ignored, so a garbled
//! "delete laughs" still deletes the sentence.

use std::ops::Range;

pub const DEFAULT_TRIGGER: &str = "zevro";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    DeleteSentence,
    DeleteParagraph,
    Undo,
    Redo,
    Newline,
    NewParagraph,
    Clear,
    Copy,
    Send,
    StopListening,
    /// Trigger heard but the following words matched nothing.
    Unknown(String),
}

impl Command {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::DeleteSentence => "delete sentence".into(),
            Self::DeleteParagraph => "delete paragraph".into(),
            Self::Undo => "undo".into(),
            Self::Redo => "redo".into(),
            Self::Newline => "new line".into(),
            Self::NewParagraph => "new paragraph".into(),
            Self::Clear => "clear".into(),
            Self::Copy => "copy".into(),
            Self::Send => "send".into(),
            Self::StopListening => "stop listening".into(),
            Self::Unknown(w) => format!("unknown command {w:?}"),
        }
    }
}

/// Grammar: normalized word sequence -> command. Longest match wins.
const GRAMMAR: &[(&[&str], Command)] = &[
    (&["delete", "last", "sentence"], Command::DeleteSentence),
    (&["delete", "sentence"], Command::DeleteSentence),
    (&["delete"], Command::DeleteSentence),
    (&["scratch", "that"], Command::DeleteSentence),
    (&["delete", "last", "paragraph"], Command::DeleteParagraph),
    (&["delete", "paragraph"], Command::DeleteParagraph),
    (&["undo"], Command::Undo),
    (&["redo"], Command::Redo),
    (&["new", "line"], Command::Newline),
    (&["newline"], Command::Newline),
    (&["new", "paragraph"], Command::NewParagraph),
    (&["clear", "all"], Command::Clear),
    (&["clear"], Command::Clear),
    (&["copy"], Command::Copy),
    (&["send"], Command::Send),
    (&["stop", "listening"], Command::StopListening),
    (&["stop"], Command::StopListening),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extraction {
    /// Text with trigger and command phrases removed.
    pub dictation: String,
    pub commands: Vec<Command>,
}

struct Token {
    range: Range<usize>,
    norm: String,
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(s)) => {
                out.push(Token {
                    range: s..i,
                    norm: normalize(&text[s..i]),
                });
                start = None;
            }
            (false, None) => start = Some(i),
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push(Token {
            range: s..text.len(),
            norm: normalize(&text[s..]),
        });
    }
    out
}

/// Consonant skeleton used to match the trigger word: lowercase letters
/// only, `v` folded into `b`, vowels dropped after the first letter,
/// doubled letters collapsed. "zevro" / "zebro" / "zebra" / "zebbro" all
/// key to "zbr".
fn phonetic_key(word: &str) -> String {
    let mut out = String::new();
    for (i, c) in word.chars().filter(|c| c.is_alphabetic()).enumerate() {
        let c = match c.to_ascii_lowercase() {
            'v' => 'b',
            c => c,
        };
        if i > 0 && matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y') {
            continue;
        }
        if out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Does the token at `i` (possibly joined with the next one) sound like
/// the trigger? Returns how many tokens it spans.
fn trigger_span(tokens: &[Token], i: usize, trigger_key: &str) -> Option<usize> {
    let one = &tokens[i].norm;
    if close_enough(trigger_key, &phonetic_key(one)) && phonetic_key(one).len() >= 3 {
        return Some(1);
    }
    if let Some(next) = tokens.get(i + 1) {
        let joined = format!("{one}{}", next.norm);
        if phonetic_key(&joined) == trigger_key {
            return Some(2);
        }
    }
    None
}

/// Same length and at most one differing letter (for words >= 4 letters).
fn close_enough(want: &str, got: &str) -> bool {
    if want == got {
        return true;
    }
    if want.chars().count() < 4 || want.chars().count() != got.chars().count() {
        return false;
    }
    want.chars()
        .zip(got.chars())
        .filter(|(a, b)| a != b)
        .count()
        <= 1
}

fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Splits `text` into dictation and commands using `trigger` (compared
/// case- and punctuation-insensitively).
#[must_use]
pub fn extract(text: &str, trigger: &str) -> Extraction {
    let trigger_key = phonetic_key(&normalize(trigger));
    let tokens = tokenize(text);
    let command_only = tokens
        .first()
        .is_some_and(|_| trigger_span(&tokens, 0, &trigger_key).is_some());
    let mut dictation_parts: Vec<&str> = Vec::new();
    let mut commands = Vec::new();
    let mut keep_from = 0usize; // byte offset where uncommitted dictation starts
    let mut i = 0;
    while i < tokens.len() {
        let Some(span) = trigger_span(&tokens, i, &trigger_key) else {
            i += 1;
            continue;
        };
        let before = text[keep_from..tokens[i].range.start].trim();
        if !before.is_empty() {
            dictation_parts.push(before);
        }
        let after = i + span;
        let (cmd, mut used) = match_command(&tokens[after..], &trigger_key);
        if command_only && !matches!(cmd, Command::Unknown(_)) {
            // Whole utterance is a command: swallow any garbled tail up to
            // the next trigger instead of dictating it.
            used = next_trigger(&tokens, after, &trigger_key) - after;
        }
        commands.push(cmd);
        // Index of the last consumed token (the trigger itself if nothing
        // followed it).
        let last = if used == 0 {
            after - 1
        } else {
            after + used - 1
        };
        keep_from = tokens[last].range.end;
        i = last + 1;
    }
    let tail = text[keep_from..].trim();
    if !tail.is_empty() {
        dictation_parts.push(tail);
    }
    Extraction {
        dictation: dictation_parts.join(" "),
        commands,
    }
}

/// Index of the next trigger token at or after `from`, or `tokens.len()`.
fn next_trigger(tokens: &[Token], from: usize, trigger_key: &str) -> usize {
    (from..tokens.len())
        .find(|&j| trigger_span(tokens, j, trigger_key).is_some())
        .unwrap_or(tokens.len())
}

/// Returns the command and how many tokens were consumed after the
/// trigger. Unknown consumes everything up to the next trigger so the
/// words are reported, not dictated.
fn match_command(after: &[Token], trigger_key: &str) -> (Command, usize) {
    let mut best: Option<(Command, usize)> = None;
    for (phrase, cmd) in GRAMMAR {
        let n = phrase.len();
        if after.len() >= n
            && phrase
                .iter()
                .zip(after)
                .all(|(want, tok)| close_enough(want, &tok.norm))
            && best.as_ref().is_none_or(|(_, len)| n > *len)
        {
            best = Some((cmd.clone(), n));
        }
    }
    best.unwrap_or_else(|| {
        let n = next_trigger(after, 0, trigger_key);
        let heard = after[..n]
            .iter()
            .map(|t| t.norm.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        (Command::Unknown(heard), n)
    })
}

/// Byte offset of the first trigger word in a provisional hypothesis, so
/// the UI can highlight the command being spoken before the Final lands.
#[must_use]
pub fn pending_command_offset(partial: &str, trigger: &str) -> Option<usize> {
    let key = phonetic_key(&normalize(trigger));
    let tokens = tokenize(partial);
    (0..tokens.len())
        .find(|&i| trigger_span(&tokens, i, &key).is_some())
        .map(|i| tokens[i].range.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(text: &str) -> Extraction {
        extract(text, DEFAULT_TRIGGER)
    }

    #[test]
    fn extraction_cases() {
        use Command::{Clear, DeleteSentence, NewParagraph, Send, StopListening, Undo, Unknown};
        let cases: Vec<(&str, &str, &str, Vec<Command>)> = vec![
            (
                "no command",
                "Move this into the service layer.",
                "Move this into the service layer.",
                vec![],
            ),
            (
                "trailing command",
                "Move this into the service layer. Zevro delete sentence.",
                "Move this into the service layer.",
                vec![DeleteSentence],
            ),
            ("only a command", "Zevro send", "", vec![Send]),
            ("case and punctuation", "zevro, SEND!", "", vec![Send]),
            (
                "longest phrase wins",
                "Zevro delete last sentence",
                "",
                vec![DeleteSentence],
            ),
            (
                "command-only utterance ignores its tail",
                "Zevro new paragraph Then continue here.",
                "",
                vec![NewParagraph],
            ),
            (
                "mid-sentence command keeps the tail",
                "Start. Zevro new paragraph Then continue here.",
                "Start. Then continue here.",
                vec![NewParagraph],
            ),
            (
                "two commands",
                "First part zevro undo second part Zevro clear.",
                "First part second part",
                vec![Undo, Clear],
            ),
            (
                "unknown drops and reports the rest",
                "Zevro banana split",
                "",
                vec![Unknown("banana split".into())],
            ),
            (
                "unknown stops at the next trigger",
                "Zevro banana zevro send",
                "",
                vec![Unknown("banana".into()), Send],
            ),
            (
                "one-letter slip in a command word",
                "Zevro sand",
                "",
                vec![Send],
            ),
            (
                "one-letter slip in the trigger",
                "Zebro send",
                "",
                vec![Send],
            ),
            (
                "zero is not the trigger",
                "Set it to zero.",
                "Set it to zero.",
                vec![],
            ),
            ("bare trigger", "Zevro", "", vec![Unknown(String::new())]),
            (
                "stop variants",
                "Zevro stop listening",
                "",
                vec![StopListening],
            ),
            (
                "trigger inside a word is not a trigger",
                "The zevrolike thing",
                "The zevrolike thing",
                vec![],
            ),
        ];
        for (name, input, dictation, commands) in cases {
            let got = x(input);
            assert_eq!(got.dictation, dictation, "{name}: dictation");
            assert_eq!(got.commands, commands, "{name}: commands");
        }
    }

    #[test]
    fn real_world_trigger_renderings_from_the_microphone() {
        use Command::{Copy, DeleteSentence, NewParagraph, Send, Unknown};
        // Verbatim whisper finals from a real session saying "Zevro ...".
        let cases: Vec<(&str, Vec<Command>)> = vec![
            ("Zebro Sand", vec![Send]),
            ("Zebro Delete laughs", vec![DeleteSentence]),
            ("Zebra new paragraph", vec![NewParagraph]),
            ("Zev Bro, new paragraph.", vec![NewParagraph]),
            ("Zebro", vec![Unknown(String::new())]),
            ("zebro delete", vec![DeleteSentence]),
            ("zebbro copy", vec![Copy]),
        ];
        for (heard, want) in cases {
            let got = x(heard);
            assert_eq!(got.commands, want, "{heard:?}");
            assert_eq!(got.dictation, "", "{heard:?} should leave no dictation");
        }
        // Known miss: a different first consonant is not the trigger.
        assert!(x("rebro copy").commands.is_empty());
    }

    #[test]
    fn phonetic_key_folds_expected_variants_and_not_zero() {
        for w in ["zevro", "Zebro", "zebra", "zebbro", "zevbro", "ZEVRO"] {
            assert_eq!(phonetic_key(w), "zbr", "{w}");
        }
        assert_eq!(phonetic_key("zero"), "zr");
        assert_ne!(phonetic_key("zipper"), "zbr");
    }

    #[test]
    fn command_only_utterance_swallows_garbled_tail_but_mid_sentence_keeps_it() {
        let got = x("Zevro new paragraph then continue");
        assert_eq!(got.commands, vec![Command::NewParagraph]);
        assert_eq!(got.dictation, "");
        let got = x("Keep this. Zevro new paragraph then continue");
        assert_eq!(got.commands, vec![Command::NewParagraph]);
        assert_eq!(got.dictation, "Keep this. then continue");
    }

    #[test]
    fn close_enough_tolerates_one_substitution_only_on_longer_words() {
        assert!(close_enough("send", "sand"));
        assert!(close_enough("zevro", "zebro"));
        assert!(!close_enough("zevro", "zero"), "deletion must not match");
        assert!(!close_enough("zevro", "zevrow"), "insertion must not match");
        assert!(!close_enough("copy", "cop"));
        assert!(!close_enough("new", "now"), "short words must be exact");
    }

    #[test]
    fn custom_trigger_word() {
        let got = extract("hello computer copy", "Computer");
        assert_eq!(got.dictation, "hello");
        assert_eq!(got.commands, vec![Command::Copy]);
    }

    #[test]
    fn pending_command_offset_points_at_the_trigger() {
        assert_eq!(
            pending_command_offset("Move this. Zevro dele", DEFAULT_TRIGGER),
            Some(11)
        );
        assert_eq!(pending_command_offset("Zevro", DEFAULT_TRIGGER), Some(0));
        assert_eq!(
            pending_command_offset("no trigger here", DEFAULT_TRIGGER),
            None
        );
    }
}
