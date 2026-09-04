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
//! "delete laughs" still deletes the sentence. Saying "abort" anywhere after
//! the trigger cancels that command before it takes effect.

use std::ops::Range;

pub const DEFAULT_TRIGGER: &str = "zevro";
/// Said after the trigger, cancels the command being spoken.
pub const ABORT_WORD: &str = "abort";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    DeleteSentence,
    DeleteParagraph,
    Undo,
    Redo,
    Newline,
    NewParagraph,
    /// "new line last": break the line just before the last sentence.
    NewlineBeforeLast,
    /// "new paragraph last": start a paragraph at the last sentence.
    NewParagraphBeforeLast,
    Clear,
    Copy,
    Send,
    StopListening,
    /// Run the AI clean-up on the whole prompt.
    CleanUp,
    /// The speaker said "abort" after the trigger; nothing runs.
    Aborted,
    /// Trigger heard but the following words matched nothing.
    Unknown(String),
}

impl Command {
    /// What the command does, for the help popup.
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::DeleteSentence => "Delete the last sentence",
            Self::DeleteParagraph => "Delete the last paragraph",
            Self::Undo => "Undo the last change",
            Self::Redo => "Redo",
            Self::Newline => "Insert a line break",
            Self::NewParagraph => "Start a new paragraph",
            Self::NewlineBeforeLast => "Move the last sentence to its own line",
            Self::NewParagraphBeforeLast => "Move the last sentence to a new paragraph",
            Self::Clear => "Clear the prompt (undoable)",
            Self::Copy => "Copy to the clipboard, keep the text",
            Self::Send => "Copy to the clipboard and clear",
            Self::StopListening => "Stop listening",
            Self::CleanUp => "AI clean-up of the whole prompt (undoable)",
            Self::Aborted => "Cancel the command you started",
            Self::Unknown(_) => "",
        }
    }

    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::DeleteSentence => "delete sentence".into(),
            Self::DeleteParagraph => "delete paragraph".into(),
            Self::Undo => "undo".into(),
            Self::Redo => "redo".into(),
            Self::Newline => "new line".into(),
            Self::NewParagraph => "new paragraph".into(),
            Self::NewlineBeforeLast => "new line last".into(),
            Self::NewParagraphBeforeLast => "new paragraph last".into(),
            Self::Clear => "clear".into(),
            Self::Copy => "copy".into(),
            Self::Send => "send".into(),
            Self::StopListening => "stop listening".into(),
            Self::CleanUp => "clean up".into(),
            Self::Aborted => "aborted".into(),
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
    // "DP" arrives as "DP", "D.P." (punctuation stripped), or "D P".
    (&["dp"], Command::DeleteParagraph),
    (&["d", "p"], Command::DeleteParagraph),
    (&["undo"], Command::Undo),
    (&["redo"], Command::Redo),
    (&["new", "line"], Command::Newline),
    (&["newline"], Command::Newline),
    (&["new", "paragraph"], Command::NewParagraph),
    // "... last" puts the break before the last sentence instead of at
    // the cursor. Longest match wins, so these beat the plain forms.
    (&["new", "line", "last"], Command::NewlineBeforeLast),
    (&["newline", "last"], Command::NewlineBeforeLast),
    (
        &["new", "paragraph", "last"],
        Command::NewParagraphBeforeLast,
    ),
    (&["clear", "all"], Command::Clear),
    (&["clear"], Command::Clear),
    (&["copy"], Command::Copy),
    (&["send"], Command::Send),
    (&["stop", "listening"], Command::StopListening),
    (&["stop"], Command::StopListening),
    (&["clean", "up"], Command::CleanUp),
    (&["cleanup"], Command::CleanUp),
];

/// One help row: every spoken phrase that maps to a command, in grammar
/// order, grouped so the popup stays in sync with what actually parses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpEntry {
    pub command: Command,
    pub phrases: Vec<String>,
}

#[must_use]
pub fn help_entries() -> Vec<HelpEntry> {
    let mut out: Vec<HelpEntry> = Vec::new();
    for (phrase, cmd) in GRAMMAR {
        let text = phrase.join(" ");
        match out.iter_mut().find(|e| e.command == *cmd) {
            Some(e) => e.phrases.push(text),
            None => out.push(HelpEntry {
                command: cmd.clone(),
                phrases: vec![text],
            }),
        }
    }
    out
}

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
        let segment_end = next_trigger(&tokens, after, &trigger_key);
        if let Some(k) = (after..segment_end).find(|&k| close_enough(ABORT_WORD, &tokens[k].norm)) {
            commands.push(Command::Aborted);
            let last = if command_only { segment_end - 1 } else { k };
            keep_from = tokens[last].range.end;
            i = last + 1;
            continue;
        }
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
            ("clean up", "Zevro clean up.", "", vec![Command::CleanUp]),
            (
                "cleanup one word",
                "Zevro cleanup",
                "",
                vec![Command::CleanUp],
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
    fn abort_cancels_the_command_being_spoken() {
        use Command::{Aborted, Send};
        let cases: Vec<(&str, &str, Vec<Command>)> = vec![
            ("Zevro delete sentence abort", "", vec![Aborted]),
            ("Zevro abort", "", vec![Aborted]),
            ("Zevro send abort no wait", "", vec![Aborted]),
            ("Zebro delete abort.", "", vec![Aborted]),
            (
                "Keep this. Zevro send abort and carry on",
                "Keep this. and carry on",
                vec![Aborted],
            ),
            ("Zevro send abort zevro send", "", vec![Aborted, Send]),
            ("Zevro send", "", vec![Send]),
        ];
        for (heard, dictation, want) in cases {
            let got = x(heard);
            assert_eq!(got.commands, want, "{heard:?}");
            assert_eq!(got.dictation, dictation, "{heard:?}");
        }
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
    fn help_entries_cover_every_grammar_phrase_once() {
        let entries = help_entries();
        let total: usize = entries.iter().map(|e| e.phrases.len()).sum();
        assert_eq!(total, GRAMMAR.len());
        assert!(entries.iter().all(|e| !e.command.description().is_empty()));
        assert_eq!(entries[0].command, Command::DeleteSentence);
        assert!(entries[0].phrases.contains(&"scratch that".to_owned()));
    }

    #[test]
    fn dp_is_short_for_delete_paragraph() {
        for spoken in ["Zevro DP", "Zevro D.P.", "Zevro D P.", "zebro dp"] {
            let got = extract(spoken, DEFAULT_TRIGGER);
            assert_eq!(got.commands, vec![Command::DeleteParagraph], "{spoken}");
            assert_eq!(got.dictation, "", "{spoken}");
        }
        let got = extract("Zevro DP", DEFAULT_TRIGGER);
        assert_eq!(got.commands, vec![Command::DeleteParagraph]);
    }

    #[test]
    fn last_suffix_selects_the_before_last_sentence_variants() {
        let got = extract("Fix it. Zevro new line last", DEFAULT_TRIGGER);
        assert_eq!(got.dictation, "Fix it.");
        assert_eq!(got.commands, vec![Command::NewlineBeforeLast]);
        let got = extract("Zevro new paragraph last.", DEFAULT_TRIGGER);
        assert_eq!(got.commands, vec![Command::NewParagraphBeforeLast]);
        let got = extract("Zevro new line", DEFAULT_TRIGGER);
        assert_eq!(got.commands, vec![Command::Newline], "plain form unchanged");
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
