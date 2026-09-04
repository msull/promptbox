//! Projects: the context for the work being dictated. A project carries
//! vocabulary (speech-recognition hints), deterministic correction rules
//! applied to fresh dictation, a glossary and freeform context for AI
//! rewrites. Projects are persisted by the app; this module is pure data
//! plus the correction algorithm and the line formats the editor uses.

use serde::{Deserialize, Serialize};

pub const DEFAULT_PROJECT: &str = "Default";

/// A deterministic replacement: when the recognizer produces `from`
/// (matched as whole words, case-insensitively), write `to` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    pub from: String,
    pub to: String,
}

/// A term the AI should know, with what it means in this project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlossaryEntry {
    pub term: String,
    pub meaning: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    /// Names and jargon the recognizer should be primed with.
    #[serde(default)]
    pub vocabulary: Vec<String>,
    /// Applied in order to every finalized utterance, never to text the
    /// user has already edited.
    #[serde(default)]
    pub corrections: Vec<Correction>,
    #[serde(default)]
    pub glossary: Vec<GlossaryEntry>,
    /// Freeform description, conventions, and rewrite preferences.
    #[serde(default)]
    pub context: String,
}

impl Project {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            vocabulary: Vec::new(),
            corrections: Vec::new(),
            glossary: Vec::new(),
            context: String::new(),
        }
    }

    /// Terms worth priming the recognizer with: the vocabulary plus every
    /// correction target and glossary term, deduplicated, in that order.
    #[must_use]
    pub fn recognition_terms(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let candidates = self
            .vocabulary
            .iter()
            .chain(self.corrections.iter().map(|c| &c.to))
            .chain(self.glossary.iter().map(|g| &g.term));
        for term in candidates {
            let term = term.trim();
            if !term.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(term)) {
                out.push(term.to_owned());
            }
        }
        out
    }

    /// What the AI is told about the project alongside a rewrite; empty
    /// when the project has nothing to say.
    #[must_use]
    pub fn ai_context(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let context = self.context.trim();
        if !context.is_empty() {
            out.push_str(context);
            out.push('\n');
        }
        if !self.glossary.is_empty() {
            out.push_str("Glossary:\n");
            for g in &self.glossary {
                let meaning = g.meaning.trim();
                if meaning.is_empty() {
                    let _ = writeln!(out, "- {}", g.term.trim());
                } else {
                    let _ = writeln!(out, "- {}: {meaning}", g.term.trim());
                }
            }
        }
        let terms = self.recognition_terms();
        if !terms.is_empty() {
            out.push_str("Names and terms to spell exactly like this: ");
            out.push_str(&terms.join(", "));
            out.push('\n');
        }
        out.trim_end().to_owned()
    }

    /// Runs this project's correction rules over `text`.
    #[must_use]
    pub fn correct(&self, text: &str) -> String {
        apply_corrections(text, &self.corrections)
    }
}

/// The list every installation starts with.
#[must_use]
pub fn default_projects() -> Vec<Project> {
    vec![Project::new(DEFAULT_PROJECT)]
}

/// Word tokens of `text`: maximal runs of alphanumeric characters (plus
/// apostrophes inside a word), as byte ranges and lowercase forms.
fn words(text: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let is_word = |c: char| c.is_alphanumeric() || c == '\'' || c == '’';
    for (i, c) in text.char_indices() {
        match (is_word(c), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                out.push((s..i, text[s..i].to_lowercase()));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s..text.len(), text[s..].to_lowercase()));
    }
    out
}

/// Applies `rules` in order. A rule matches wherever its words appear as
/// consecutive whole words in `text`, ignoring case and the separators
/// between them; the whole span is replaced by `to`. Everything outside the
/// matches, including all punctuation and spacing, is kept byte for byte.
#[must_use]
pub fn apply_corrections(text: &str, rules: &[Correction]) -> String {
    let mut current = text.to_owned();
    for rule in rules {
        let pattern: Vec<String> = words(&rule.from).into_iter().map(|(_, w)| w).collect();
        if pattern.is_empty() {
            continue;
        }
        let toks = words(&current);
        let mut out = String::with_capacity(current.len());
        let mut copied = 0usize;
        let mut i = 0;
        while i < toks.len() {
            let fits = i + pattern.len() <= toks.len()
                && pattern.iter().zip(&toks[i..]).all(|(p, (_, w))| p == w);
            if fits {
                let span = toks[i].0.start..toks[i + pattern.len() - 1].0.end;
                out.push_str(&current[copied..span.start]);
                out.push_str(&rule.to);
                copied = span.end;
                i += pattern.len();
            } else {
                i += 1;
            }
        }
        out.push_str(&current[copied..]);
        current = out;
    }
    current
}

/// Line formats the project editor uses: one entry per line.
pub mod lines {
    use super::{Correction, GlossaryEntry};

    const ARROW: &str = "=>";

    #[must_use]
    pub fn vocabulary_to_text(v: &[String]) -> String {
        v.join("\n")
    }

    #[must_use]
    pub fn vocabulary_from_text(s: &str) -> Vec<String> {
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// `heard words => Written Form`
    #[must_use]
    pub fn corrections_to_text(c: &[Correction]) -> String {
        c.iter()
            .map(|c| format!("{} {ARROW} {}", c.from, c.to))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Lines without `=>` are ignored, as are rules with an empty side.
    #[must_use]
    pub fn corrections_from_text(s: &str) -> Vec<Correction> {
        s.lines()
            .filter_map(|l| {
                let (from, to) = l.split_once(ARROW)?;
                let (from, to) = (from.trim(), to.trim());
                (!from.is_empty() && !to.is_empty()).then(|| Correction {
                    from: from.to_owned(),
                    to: to.to_owned(),
                })
            })
            .collect()
    }

    /// `Term: what it means`; the meaning is optional.
    #[must_use]
    pub fn glossary_to_text(g: &[GlossaryEntry]) -> String {
        g.iter()
            .map(|g| {
                if g.meaning.is_empty() {
                    g.term.clone()
                } else {
                    format!("{}: {}", g.term, g.meaning)
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[must_use]
    pub fn glossary_from_text(s: &str) -> Vec<GlossaryEntry> {
        s.lines()
            .filter_map(|l| {
                let (term, meaning) = l.split_once(':').unwrap_or((l, ""));
                let term = term.trim();
                (!term.is_empty()).then(|| GlossaryEntry {
                    term: term.to_owned(),
                    meaning: meaning.trim().to_owned(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(from: &str, to: &str) -> Correction {
        Correction {
            from: from.into(),
            to: to.into(),
        }
    }

    #[test]
    fn corrections_match_whole_words_case_insensitively() {
        let rules = [
            rule("you never sheets", "Univer Sheets"),
            rule("fast html", "FastHTML"),
        ];
        assert_eq!(
            apply_corrections("Open You Never Sheets, then fast HTML.", &rules),
            "Open Univer Sheets, then FastHTML."
        );
        assert_eq!(
            apply_corrections("fasthtml is one word", &rules),
            "fasthtml is one word",
            "partial words never match"
        );
        assert_eq!(
            apply_corrections("A fast, html page", &rules),
            "A FastHTML page",
            "separators between the words are ignored"
        );
    }

    #[test]
    fn corrections_apply_in_order_and_keep_the_rest() {
        let rules = [
            rule("dynamo db", "DynamoDB"),
            rule("DynamoDB table", "Table"),
        ];
        assert_eq!(
            apply_corrections("the dynamo DB table\n\n(dynamo db)", &rules),
            "the Table\n\n(DynamoDB)"
        );
        assert_eq!(apply_corrections("", &rules), "");
        assert_eq!(
            apply_corrections("héllo wörld", &[rule("wörld", "World")]),
            "héllo World"
        );
        assert_eq!(
            apply_corrections("x", &[rule("  ", "y")]),
            "x",
            "empty pattern is skipped"
        );
    }

    #[test]
    fn line_formats_round_trip() {
        use lines::*;
        let c = vec![rule("you never", "Univer"), rule("dynamo db", "DynamoDB")];
        assert_eq!(corrections_from_text(&corrections_to_text(&c)), c);
        assert_eq!(
            corrections_from_text("junk line\n => nothing\nfoo => "),
            Vec::<Correction>::new()
        );
        let g = vec![
            GlossaryEntry {
                term: "Univer Sheets".into(),
                meaning: "the spreadsheet product".into(),
            },
            GlossaryEntry {
                term: "FastHTML".into(),
                meaning: String::new(),
            },
        ];
        assert_eq!(glossary_from_text(&glossary_to_text(&g)), g);
        let v = vec!["Pydantic".to_owned(), "DynamoDB".to_owned()];
        assert_eq!(vocabulary_from_text(&vocabulary_to_text(&v)), v);
        assert_eq!(vocabulary_from_text("  \n a \n\n b "), vec!["a", "b"]);
    }

    #[test]
    fn ai_context_and_recognition_terms() {
        let mut p = Project::new("Acme");
        assert_eq!(p.ai_context(), "");
        assert!(p.recognition_terms().is_empty());
        p.vocabulary = vec!["Pydantic".into(), "fasthtml".into()];
        p.corrections = vec![rule("fast html", "FastHTML")];
        p.glossary = vec![GlossaryEntry {
            term: "Univer Sheets".into(),
            meaning: "spreadsheet app".into(),
        }];
        p.context = "A FastHTML web app.".into();
        assert_eq!(
            p.recognition_terms(),
            vec!["Pydantic", "fasthtml", "Univer Sheets"],
            "duplicates are case-insensitive"
        );
        assert_eq!(
            p.ai_context(),
            "A FastHTML web app.\nGlossary:\n- Univer Sheets: spreadsheet app\n\
             Names and terms to spell exactly like this: Pydantic, fasthtml, Univer Sheets"
        );
    }
}
