//! Table-driven tests for the provisional-span document prototype.

use super::*;
use crate::ports::speech::{SessionId, SpeechEvent, SpeechEventKind, UtteranceId};

const S1: SessionId = 1;
const S2: SessionId = 2;

/// One scripted step. Speech events carry (session, sequence).
#[derive(Debug, Clone)]
enum Step {
    Session(SessionId),
    Cursor(usize),
    Start {
        s: SessionId,
        seq: u64,
        u: UtteranceId,
    },
    Partial {
        s: SessionId,
        seq: u64,
        u: UtteranceId,
        r: u64,
        text: &'static str,
    },
    Final {
        s: SessionId,
        seq: u64,
        u: UtteranceId,
        text: &'static str,
    },
    Edit {
        range: Range<usize>,
        text: &'static str,
        policy: OverlapPolicy,
    },
    /// Expect the *next* speech event to be rejected with this reason.
    Reject(Rejection),
    /// Expect the *next* manual edit to fail with this error.
    EditFails(EditError),
    /// Assert the rendered text right now.
    Rendered(&'static str),
    /// Assert the committed text right now.
    Committed(&'static str),
}

use Step::{Committed, Cursor, Edit, EditFails, Final, Partial, Reject, Rendered, Session, Start};

struct Case {
    name: &'static str,
    steps: Vec<Step>,
    rendered: &'static str,
    committed: &'static str,
    history_len: usize,
}

fn ev(s: SessionId, seq: u64, kind: SpeechEventKind) -> SpeechEvent {
    SpeechEvent {
        session: s,
        sequence: seq,
        audio_range: 0..0,
        kind,
    }
}

#[allow(clippy::too_many_lines)]
fn run(case: &Case) {
    let mut doc = Document::new();
    doc.set_active_session(S1);
    let mut expect_reject: Option<Rejection> = None;
    let mut expect_edit_fail: Option<EditError> = None;
    let check = |doc: &mut Document,
                 res: Result<Applied, Rejection>,
                 expect_reject: &mut Option<Rejection>,
                 step: &Step| {
        let before = doc.rendered();
        match (expect_reject.take(), res) {
            (None, Ok(_)) => {}
            (Some(want), Err(got)) => {
                assert_eq!(want, got, "[{}] step {step:?}", case.name);
                assert_eq!(
                    doc.rendered(),
                    before,
                    "[{}] rejected event mutated doc",
                    case.name
                );
            }
            (want, got) => panic!(
                "[{}] step {step:?}: wanted {want:?}, got {got:?}",
                case.name
            ),
        }
    };
    for step in &case.steps {
        match step {
            Session(s) => doc.set_active_session(*s),
            Cursor(p) => doc.set_cursor(*p),
            Start { s, seq, u } => {
                let r = doc.apply_event(&ev(
                    *s,
                    *seq,
                    SpeechEventKind::VoiceStarted { utterance: *u },
                ));
                check(&mut doc, r, &mut expect_reject, step);
            }
            Partial { s, seq, u, r, text } => {
                let res = doc.apply_event(&ev(
                    *s,
                    *seq,
                    SpeechEventKind::Partial {
                        utterance: *u,
                        revision: *r,
                        text: (*text).to_owned(),
                    },
                ));
                check(&mut doc, res, &mut expect_reject, step);
            }
            Final { s, seq, u, text } => {
                let res = doc.apply_event(&ev(
                    *s,
                    *seq,
                    SpeechEventKind::Final {
                        utterance: *u,
                        text: (*text).to_owned(),
                        confidence: None,
                    },
                ));
                check(&mut doc, res, &mut expect_reject, step);
            }
            Edit {
                range,
                text,
                policy,
            } => {
                let before = doc.rendered();
                let res = doc.apply_manual_edit(range.clone(), text, *policy);
                match (expect_edit_fail.take(), res) {
                    (None, Ok(())) => {}
                    (Some(want), Err(got)) => {
                        assert_eq!(want, got, "[{}] {step:?}", case.name);
                        assert_eq!(doc.rendered(), before);
                    }
                    (want, got) => panic!("[{}] {step:?}: wanted {want:?}, got {got:?}", case.name),
                }
            }
            Reject(r) => expect_reject = Some(*r),
            EditFails(e) => expect_edit_fail = Some(*e),
            Rendered(s) => assert_eq!(doc.rendered(), *s, "[{}] mid-case rendered", case.name),
            Committed(s) => assert_eq!(doc.committed(), *s, "[{}] mid-case committed", case.name),
        }
        // Invariant: rendered is always valid and consistent with committed + span.
        let r = doc.rendered();
        assert!(std::str::from_utf8(r.as_bytes()).is_ok());
        if let Some(pr) = doc.provisional_range() {
            assert!(r.is_char_boundary(pr.start) && r.is_char_boundary(pr.end));
        }
    }
    assert!(expect_reject.is_none(), "[{}] unused Reject", case.name);
    assert_eq!(doc.rendered(), case.rendered, "[{}] rendered", case.name);
    assert_eq!(doc.committed(), case.committed, "[{}] committed", case.name);
    assert_eq!(
        doc.history().len(),
        case.history_len,
        "[{}] history len",
        case.name
    );
    // Invariant 5: replaying the single history reproduces committed text.
    assert_eq!(
        doc.replay_history(),
        doc.committed(),
        "[{}] replay",
        case.name
    );
}

#[allow(clippy::too_many_lines)]
fn cases() -> Vec<Case> {
    let mp = OverlapPolicy::CommitProvisional;
    vec![
        Case {
            name: "single partial then final",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "run the",
                },
                Rendered("run the"),
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "Run the tests.",
                },
            ],
            rendered: "Run the tests.",
            committed: "Run the tests.",
            history_len: 1,
        },
        Case {
            name: "revisions replace in place",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "add a",
                },
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "add a pedantic",
                },
                Partial {
                    s: S1,
                    seq: 4,
                    u: 1,
                    r: 3,
                    text: "add a Pydantic model",
                },
            ],
            rendered: "add a Pydantic model",
            committed: "",
            history_len: 0,
        },
        Case {
            name: "duplicate sequence rejected",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "hello",
                },
                Reject(Rejection::DuplicateSeq),
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 2,
                    text: "hello there",
                },
            ],
            rendered: "hello",
            committed: "",
            history_len: 0,
        },
        Case {
            name: "out of order sequence rejected",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 5,
                    u: 1,
                    r: 1,
                    text: "five",
                },
                Reject(Rejection::OutOfOrderSeq),
                Partial {
                    s: S1,
                    seq: 4,
                    u: 1,
                    r: 2,
                    text: "four",
                },
            ],
            rendered: "five",
            committed: "",
            history_len: 0,
        },
        Case {
            name: "stale revision rejected",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 3,
                    text: "third",
                },
                Reject(Rejection::StaleRevision),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "second",
                },
            ],
            rendered: "third",
            committed: "",
            history_len: 0,
        },
        Case {
            name: "partial after final rejected",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "done",
                },
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "Done.",
                },
                Reject(Rejection::UtteranceFinished),
                Partial {
                    s: S1,
                    seq: 4,
                    u: 1,
                    r: 2,
                    text: "done late",
                },
            ],
            rendered: "Done.",
            committed: "Done.",
            history_len: 1,
        },
        Case {
            name: "old session after restart is stale",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "first session",
                },
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "First session.",
                },
                Session(S2),
                Reject(Rejection::StaleSession),
                Partial {
                    s: S1,
                    seq: 4,
                    u: 2,
                    r: 1,
                    text: "late from old worker",
                },
                Start {
                    s: S2,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S2,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "second",
                },
            ],
            rendered: "First session. second",
            committed: "First session.",
            history_len: 1,
        },
        Case {
            name: "final without any partial inserts at start anchor",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "prefix",
                    policy: mp,
                },
                Cursor(3),
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Cursor(6),
                Final {
                    s: S1,
                    seq: 2,
                    u: 1,
                    text: "X",
                },
            ],
            rendered: "pre Xfix",
            committed: "pre Xfix",
            history_len: 2,
        },
        Case {
            name: "manual insert before anchor shifts span",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "ab cd",
                    policy: mp,
                },
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "ef",
                },
                Rendered("ab cd ef"),
                Edit {
                    range: 0..0,
                    text: "ZZ ",
                    policy: mp,
                },
                Rendered("ZZ ab cd ef"),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "ef gh",
                },
                Rendered("ZZ ab cd ef gh"),
                Final {
                    s: S1,
                    seq: 4,
                    u: 1,
                    text: "ef gh.",
                },
            ],
            rendered: "ZZ ab cd ef gh.",
            committed: "ZZ ab cd ef gh.",
            history_len: 3,
        },
        Case {
            name: "manual delete before anchor shifts span left",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "ab cd",
                    policy: mp,
                },
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "ef",
                },
                Edit {
                    range: 0..3,
                    text: "",
                    policy: mp,
                },
                Rendered("cd ef"),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "ef!",
                },
                Final {
                    s: S1,
                    seq: 4,
                    u: 1,
                    text: "ef!",
                },
            ],
            rendered: "cd ef!",
            committed: "cd ef!",
            history_len: 3,
        },
        Case {
            name: "manual edit after span leaves anchor alone",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "ab",
                    policy: mp,
                },
                Cursor(2),
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "cd",
                },
                Rendered("ab cd"),
                Edit {
                    range: 5..5,
                    text: " tail",
                    policy: mp,
                },
                Rendered("ab cd tail"),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "cd ef",
                },
                Final {
                    s: S1,
                    seq: 4,
                    u: 1,
                    text: "cd ef",
                },
            ],
            rendered: "ab cd ef tail",
            committed: "ab cd ef tail",
            history_len: 3,
        },
        Case {
            name: "overlapping edit commits provisional first",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "hello world",
                },
                Edit {
                    range: 6..11,
                    text: "there",
                    policy: OverlapPolicy::CommitProvisional,
                },
                Committed("hello there"),
                Reject(Rejection::UtteranceFinished),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "hello world again",
                },
            ],
            rendered: "hello there",
            committed: "hello there",
            history_len: 2,
        },
        Case {
            name: "overlapping edit cancels provisional",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "AB",
                    policy: mp,
                },
                Cursor(2),
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "hello",
                },
                Rendered("AB hello"),
                // Delete "B hel" (1..6): crosses into the span.
                Edit {
                    range: 1..6,
                    text: "-",
                    policy: OverlapPolicy::CancelProvisional,
                },
            ],
            rendered: "A-",
            committed: "A-",
            history_len: 2,
        },
        Case {
            name: "cursor moved between partials does not move span",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "one two",
                    policy: mp,
                },
                Cursor(3),
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "X",
                },
                Rendered("one X two"),
                Cursor(0),
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "XY",
                },
                Rendered("one XY two"),
                Final {
                    s: S1,
                    seq: 4,
                    u: 1,
                    text: "XYZ",
                },
            ],
            rendered: "one XYZ two",
            committed: "one XYZ two",
            history_len: 2,
        },
        Case {
            name: "two utterances append with spacing",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "first",
                },
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "First.",
                },
                Start {
                    s: S1,
                    seq: 4,
                    u: 2,
                },
                Partial {
                    s: S1,
                    seq: 5,
                    u: 2,
                    r: 1,
                    text: "second",
                },
                Rendered("First. second"),
                Final {
                    s: S1,
                    seq: 6,
                    u: 2,
                    text: "Second.",
                },
            ],
            rendered: "First. Second.",
            committed: "First. Second.",
            history_len: 2,
        },
        Case {
            name: "unicode before anchor: precomposed and combining",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "caf\u{e9} e\u{301} ",
                    policy: mp,
                },
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "naïve",
                },
                Rendered("caf\u{e9} e\u{301} naïve"),
                Edit {
                    range: 0..0,
                    text: "→",
                    policy: mp,
                },
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "naïve.",
                },
            ],
            rendered: "→caf\u{e9} e\u{301} naïve.",
            committed: "→caf\u{e9} e\u{301} naïve.",
            history_len: 3,
        },
        Case {
            name: "grapheme boundary rejected inside zwj emoji and flag",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "👨‍👩‍👧 🇺🇸 x",
                    policy: mp,
                },
                // "👨" is 4 bytes; offset 4 lands before the ZWJ, inside the cluster.
                EditFails(EditError::NotGraphemeBoundary),
                Edit {
                    range: 4..4,
                    text: "!",
                    policy: mp,
                },
                // Family emoji is 18 bytes + space = 19; flag starts at 19 and is 8 bytes.
                EditFails(EditError::NotGraphemeBoundary),
                Edit {
                    range: 23..23,
                    text: "!",
                    policy: mp,
                },
                // 19+8 = 27 is the boundary after the flag.
                Edit {
                    range: 27..27,
                    text: "!",
                    policy: mp,
                },
                EditFails(EditError::OutOfBounds),
                Edit {
                    range: 100..101,
                    text: "",
                    policy: mp,
                },
            ],
            rendered: "👨‍👩‍👧 🇺🇸! x",
            committed: "👨‍👩‍👧 🇺🇸! x",
            history_len: 2,
        },
        Case {
            name: "emoji partial replaced by shorter ascii revision",
            steps: vec![
                Edit {
                    range: 0..0,
                    text: "a",
                    policy: mp,
                },
                Cursor(1),
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "🎉🎉 中文",
                },
                Edit {
                    range: 0..0,
                    text: "<",
                    policy: mp,
                },
                Partial {
                    s: S1,
                    seq: 3,
                    u: 1,
                    r: 2,
                    text: "ok",
                },
                Rendered("<a ok"),
                Edit {
                    range: 5..5,
                    text: ">",
                    policy: mp,
                },
                Final {
                    s: S1,
                    seq: 4,
                    u: 1,
                    text: "ok",
                },
            ],
            rendered: "<a ok>",
            committed: "<a ok>",
            history_len: 4,
        },
        Case {
            name: "empty final on stop drops span without history",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "half a",
                },
                Final {
                    s: S1,
                    seq: 3,
                    u: 1,
                    text: "",
                },
            ],
            rendered: "",
            committed: "",
            history_len: 0,
        },
        Case {
            name: "new utterance while old span live commits the old one",
            steps: vec![
                Start {
                    s: S1,
                    seq: 1,
                    u: 1,
                },
                Partial {
                    s: S1,
                    seq: 2,
                    u: 1,
                    r: 1,
                    text: "old",
                },
                Start {
                    s: S1,
                    seq: 3,
                    u: 2,
                },
                Partial {
                    s: S1,
                    seq: 4,
                    u: 2,
                    r: 1,
                    text: "new",
                },
                Rendered("old new"),
                Reject(Rejection::UtteranceFinished),
                Final {
                    s: S1,
                    seq: 5,
                    u: 1,
                    text: "old late",
                },
                Final {
                    s: S1,
                    seq: 6,
                    u: 2,
                    text: "new.",
                },
            ],
            rendered: "old new.",
            committed: "old new.",
            history_len: 2,
        },
    ]
}

#[test]
fn table_driven_document_cases() {
    for case in cases() {
        run(&case);
    }
}

#[test]
fn undo_reverts_last_entry_and_cancels_provisional_first() {
    let mut doc = Document::new();
    doc.set_active_session(S1);
    doc.apply_manual_edit(0..0, "hello", OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.apply_manual_edit(5..5, " world", OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.apply_event(&ev(S1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }))
        .unwrap();
    doc.apply_event(&ev(
        S1,
        2,
        SpeechEventKind::Partial {
            utterance: 1,
            revision: 1,
            text: "prov".into(),
        },
    ))
    .unwrap();
    assert_eq!(doc.rendered(), "hello world prov");
    assert!(doc.undo());
    assert_eq!(doc.rendered(), "hello world");
    assert!(doc.undo());
    assert_eq!(doc.rendered(), "hello");
    assert_eq!(doc.cursor(), 5);
    assert!(doc.undo());
    assert_eq!(doc.rendered(), "");
    assert!(!doc.undo());
}

#[test]
fn redo_reapplies_until_a_new_edit_supersedes_it() {
    let mut doc = Document::new();
    doc.apply_manual_edit(0..0, "one", OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.apply_manual_edit(3..3, " two", OverlapPolicy::CommitProvisional)
        .unwrap();
    assert!(!doc.can_redo());
    assert!(doc.undo());
    assert_eq!(doc.committed(), "one");
    assert!(doc.can_redo());
    assert!(doc.redo());
    assert_eq!(doc.committed(), "one two");
    assert_eq!(doc.cursor(), 7);
    assert!(!doc.redo());
    assert!(doc.undo());
    doc.apply_manual_edit(3..3, " three", OverlapPolicy::CommitProvisional)
        .unwrap();
    assert!(!doc.can_redo(), "new edit clears redo");
    assert_eq!(doc.replay_history(), doc.committed());
}

#[test]
fn whole_document_replacement_invalidates_anchors_of_a_live_utterance() {
    // An utterance starts (anchor captured near the end of a long text),
    // then an AI rewrite replaces the document with something shorter.
    let mut doc = Document::new();
    doc.set_active_session(S1);
    let long = "x".repeat(246);
    doc.apply_manual_edit(0..0, &long, OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.apply_event(&ev(S1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }))
        .unwrap();
    doc.replace_all_from("short", EditSource::Ai);
    doc.apply_event(&ev(
        S1,
        2,
        SpeechEventKind::Partial {
            utterance: 1,
            revision: 1,
            text: "more".into(),
        },
    ))
    .unwrap();
    assert_eq!(doc.rendered(), "short more");
    doc.apply_event(&ev(
        S1,
        3,
        SpeechEventKind::Final {
            utterance: 1,
            text: "more.".into(),
            confidence: None,
        },
    ))
    .unwrap();
    assert_eq!(doc.committed(), "short more.");
    // Same for a persisted draft loading over a live anchor.
    doc.apply_event(&ev(S1, 4, SpeechEventKind::VoiceStarted { utterance: 2 }))
        .unwrap();
    doc.load("hi");
    doc.apply_event(&ev(
        S1,
        5,
        SpeechEventKind::Final {
            utterance: 2,
            text: "there".into(),
            confidence: None,
        },
    ))
    .unwrap();
    assert_eq!(doc.committed(), "hi there");
}

#[test]
fn replace_all_is_one_undoable_entry() {
    let mut doc = Document::new();
    doc.apply_manual_edit(0..0, "keep me", OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.replace_all("");
    assert!(doc.is_empty());
    assert_eq!(doc.history().len(), 2);
    assert!(doc.undo());
    assert_eq!(doc.committed(), "keep me");
}

#[test]
fn ignored_events_still_advance_sequence() {
    let mut doc = Document::new();
    doc.set_active_session(S1);
    let r = doc.apply_event(&ev(S1, 7, SpeechEventKind::ProcessingDelayed));
    assert_eq!(r, Ok(Applied::Ignored));
    let r = doc.apply_event(&ev(S1, 7, SpeechEventKind::VoiceStarted { utterance: 1 }));
    assert_eq!(r, Err(Rejection::DuplicateSeq));
}

#[test]
fn provisional_range_matches_rendered() {
    let mut doc = Document::new();
    doc.set_active_session(S1);
    doc.apply_manual_edit(0..0, "héllo", OverlapPolicy::CommitProvisional)
        .unwrap();
    doc.set_cursor(usize::MAX);
    doc.apply_event(&ev(S1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }))
        .unwrap();
    doc.apply_event(&ev(
        S1,
        2,
        SpeechEventKind::Partial {
            utterance: 1,
            revision: 1,
            text: "wörld".into(),
        },
    ))
    .unwrap();
    let r = doc.rendered();
    let pr = doc.provisional_range().unwrap();
    assert_eq!(&r[pr], " wörld");
}

#[test]
fn caption_is_last_committed_sentence_plus_live_span() {
    let mut doc = Document::new();
    assert_eq!(doc.caption(), "");
    doc.load("First point. Second point.");
    assert_eq!(
        doc.caption(),
        "Second point.",
        "no live span: sentence at the cursor"
    );
    doc.set_active_session(1);
    doc.apply_event(&ev(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }))
        .unwrap();
    doc.apply_event(&ev(
        1,
        2,
        SpeechEventKind::Partial {
            utterance: 1,
            revision: 1,
            text: "third po".into(),
        },
    ))
    .unwrap();
    assert_eq!(doc.caption(), "Second point. third po");
    doc.apply_event(&ev(
        1,
        3,
        SpeechEventKind::Final {
            utterance: 1,
            text: "Third point.".into(),
            confidence: None,
        },
    ))
    .unwrap();
    assert_eq!(doc.caption(), "Third point.");
    let mut empty = Document::new();
    empty.set_active_session(1);
    empty
        .apply_event(&ev(1, 1, SpeechEventKind::VoiceStarted { utterance: 1 }))
        .unwrap();
    empty
        .apply_event(&ev(
            1,
            2,
            SpeechEventKind::Partial {
                utterance: 1,
                revision: 1,
                text: "hello".into(),
            },
        ))
        .unwrap();
    assert_eq!(
        empty.caption(),
        "hello",
        "nothing committed: live text alone"
    );
}
