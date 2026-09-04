//! Deterministic voice-command extraction. A trigger word ("Zevro") opens
//! the command channel; the phrase after it is matched against a fixed
//! grammar. Commands are extracted only from *final* recognition text, so
//! successive provisional hypotheses can never fire one twice. Everything
//! outside trigger + command phrase stays dictation, with its original
//! casing and punctuation. Words after a trigger that match nothing are
//! dropped and reported, never dictated: the speaker meant a command.
//!
//! Recognition is imperfect ("send" arrives as "sand", "Zevro" as "Zebro"),
//! so words are compared with a one-substitution tolerance for words of
//! four letters or more. Insertions/deletions are not tolerated so that
//! "zero" cannot open the command channel.

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
    let trigger = normalize(trigger);
    let tokens = tokenize(text);
    let mut dictation_parts: Vec<&str> = Vec::new();
    let mut commands = Vec::new();
    let mut keep_from = 0usize; // byte offset where uncommitted dictation starts
    let mut i = 0;
    while i < tokens.len() {
        if !close_enough(&trigger, &tokens[i].norm) {
            i += 1;
            continue;
        }
        let before = text[keep_from..tokens[i].range.start].trim();
        if !before.is_empty() {
            dictation_parts.push(before);
        }
        let (cmd, used) = match_command(&tokens[i + 1..], &trigger);
        commands.push(cmd);
        let last = i + used; // index of the last consumed token
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

/// Returns the command and how many tokens were consumed after the
/// trigger. Unknown consumes everything up to the next trigger so the
/// words are reported, not dictated.
fn match_command(after: &[Token], trigger: &str) -> (Command, usize) {
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
        let n = after
            .iter()
            .position(|t| close_enough(trigger, &t.norm))
            .unwrap_or(after.len());
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
    let trigger = normalize(trigger);
    tokenize(partial)
        .iter()
        .find(|t| close_enough(&trigger, &t.norm))
        .map(|t| t.range.start)
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
                "command then dictation",
                "Zevro new paragraph Then continue here.",
                "Then continue here.",
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
