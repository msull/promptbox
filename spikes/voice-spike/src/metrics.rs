//! Metrics computed from a feed timeline plus engine counters.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::{Counters, SAMPLE_RATE};
use crate::events::{SessionId, SpeechEventKind, UtteranceId};
use crate::feeder::FeedResult;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RunMetrics {
    pub model: String,
    pub fixture: String,
    pub realtime: bool,
    #[serde(with = "nan_f64")]
    pub duration_s: f64,
    #[serde(with = "nan_f64")]
    pub speech_s: f64,
    #[serde(with = "nan_f64")]
    pub load_ms: f64,
    #[serde(with = "nan_f64")]
    pub state_create_ms: f64,
    pub utterances: usize,
    pub partials: usize,
    pub finals: usize,
    #[serde(with = "nan_f64")]
    pub ttfp_med_ms: f64,
    #[serde(with = "nan_f64")]
    pub ttfp_max_ms: f64,
    #[serde(with = "nan_f64")]
    pub ttfp_audio_med_ms: f64,
    #[serde(with = "nan_f64")]
    pub partials_per_s: f64,
    #[serde(with = "nan_f64")]
    pub partial_latency_med_ms: f64,
    #[serde(with = "nan_f64")]
    pub partial_latency_p95_ms: f64,
    #[serde(with = "nan_f64")]
    pub final_latency_med_ms: f64,
    #[serde(with = "nan_f64")]
    pub stability: f64,
    #[serde(with = "nan_f64")]
    pub mean_retracted_words: f64,
    #[serde(with = "nan_f64")]
    pub final_differs_frac: f64,
    #[serde(with = "nan_f64")]
    pub wer: f64,
    pub wer_s: usize,
    pub wer_d: usize,
    pub wer_i: usize,
    pub wer_n: usize,
    #[serde(with = "nan_f64")]
    pub rtf: f64,
    #[serde(with = "nan_f64")]
    pub wall_rtf: f64,
    pub full_calls: u64,
    #[serde(with = "nan_f64")]
    pub full_med_ms: f64,
    #[serde(with = "nan_f64")]
    pub peak_rss_mb: f64,
    pub dropped_audio_chunks: u64,
    pub dropped_events: u64,
    pub gap_samples: u64,
    pub delayed: u64,
    pub hallucinations: u64,
    pub forced_splits: u64,
    pub drain_timed_out: bool,
    pub hypothesis: String,
}

/// JSON has no NaN: serialize it as `null` and read `null` back as NaN.
mod nan_f64 {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[allow(clippy::trivially_copy_pass_by_ref)] // serde `with` requires a reference
    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_nan() { None } else { Some(*v) }.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
    }
}

/// Lowercase alphanumeric words; apostrophes kept inside words.
#[must_use]
pub fn normalize_words(s: &str) -> Vec<String> {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '\'' {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(|w| w.trim_matches('\'').to_owned())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word error rate components (substitutions, deletions, insertions, N).
#[must_use]
pub fn wer(reference: &[String], hypothesis: &[String]) -> (usize, usize, usize, usize) {
    let n = reference.len();
    let m = hypothesis.len();
    // dp[i][j] = (cost, s, d, i)
    let mut dp = vec![vec![(0usize, 0usize, 0usize, 0usize); m + 1]; n + 1];
    for i in 1..=n {
        dp[i][0] = (i, 0, i, 0);
    }
    for j in 1..=m {
        dp[0][j] = (j, 0, 0, j);
    }
    for i in 1..=n {
        for j in 1..=m {
            let (c, s, d, ins) = dp[i - 1][j - 1];
            let sub = if reference[i - 1] == hypothesis[j - 1] {
                (c, s, d, ins)
            } else {
                (c + 1, s + 1, d, ins)
            };
            let (c, s, d, ins) = dp[i - 1][j];
            let del = (c + 1, s, d + 1, ins);
            let (c, s, d, ins) = dp[i][j - 1];
            let insn = (c + 1, s, d, ins + 1);
            dp[i][j] = [sub, del, insn]
                .into_iter()
                .min_by_key(|x| x.0)
                .expect("3 items");
        }
    }
    let (_, s, d, i) = dp[n][m];
    (s, d, i, n)
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(f64::total_cmp);
    let idx = ((v.len() - 1) as f64 * p).round() as usize;
    v[idx]
}

/// Peak resident set size of this process in MB (macOS reports bytes).
#[must_use]
#[allow(unsafe_code)]
pub fn peak_rss_mb() -> f64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage fills the struct we own; RUSAGE_SELF is always valid.
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if rc != 0 {
        return f64::NAN;
    }
    // SAFETY: rc == 0 means the struct was initialized.
    let usage = unsafe { usage.assume_init() };
    let bytes = usage.ru_maxrss as f64;
    if cfg!(target_os = "macos") {
        bytes / 1_048_576.0
    } else {
        bytes / 1024.0
    }
}

#[derive(Default)]
struct UttStats {
    started_wall: Option<f64>,
    started_audio: Option<u64>,
    ended_audio: Option<u64>,
    partials: Vec<(f64, u64, String)>, // wall, audio end, text
    final_text: Option<String>,
    final_audio_end: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn compute(
    model: &str,
    fixture: &str,
    realtime: bool,
    duration_s: f64,
    load_ms: f64,
    reference: &str,
    result: &FeedResult,
    counters: &Counters,
) -> RunMetrics {
    let mut utts: BTreeMap<(SessionId, UtteranceId), UttStats> = BTreeMap::new();
    let mut partial_lat = Vec::new();
    let mut final_lat = Vec::new();
    let mut finals_in_order = Vec::new();
    for e in &result.timeline {
        let key = e.event.utterance().map(|u| (e.event.session, u));
        match &e.event.kind {
            SpeechEventKind::VoiceStarted { .. } => {
                let s = utts.entry(key.expect("utt")).or_default();
                s.started_wall = Some(e.wall_ms);
                s.started_audio = Some(e.event.audio_range.start);
            }
            SpeechEventKind::VoiceEnded { .. } => {
                utts.entry(key.expect("utt")).or_default().ended_audio =
                    Some(e.event.audio_range.start);
            }
            SpeechEventKind::Partial { text, .. } => {
                utts.entry(key.expect("utt")).or_default().partials.push((
                    e.wall_ms,
                    e.event.audio_range.end,
                    text.clone(),
                ));
                if let Some(l) = e.latency_ms {
                    partial_lat.push(l);
                }
            }
            SpeechEventKind::Final { text, .. } => {
                let s = utts.entry(key.expect("utt")).or_default();
                s.final_text = Some(text.clone());
                s.final_audio_end = Some(e.event.audio_range.end);
                finals_in_order.push(text.clone());
                if let Some(l) = e.latency_ms {
                    final_lat.push(l);
                }
            }
            _ => {}
        }
    }

    let mut ttfp = Vec::new();
    let mut ttfp_audio = Vec::new();
    let mut speech_samples = 0u64;
    let mut prefix_changes = 0usize;
    let mut pairs = 0usize;
    let mut retracted = 0usize;
    let mut final_differs = 0usize;
    let mut finals_with_partials = 0usize;
    let mut partials = 0usize;
    for s in utts.values() {
        partials += s.partials.len();
        if let (Some(a), Some(b)) = (s.started_audio, s.ended_audio.or(s.final_audio_end)) {
            speech_samples += b.saturating_sub(a);
        }
        if let (Some(w0), Some((w1, a1, _))) = (s.started_wall, s.partials.first()) {
            ttfp.push(w1 - w0);
            if let Some(a0) = s.started_audio {
                ttfp_audio.push((a1 - a0) as f64 * 1000.0 / f64::from(SAMPLE_RATE));
            }
        }
        for w in s.partials.windows(2) {
            let prev = normalize_words(&w[0].2);
            let cur = normalize_words(&w[1].2);
            let common = prev.iter().zip(&cur).take_while(|(a, b)| a == b).count();
            pairs += 1;
            if common < prev.len() {
                prefix_changes += 1;
                retracted += prev.len() - common;
            }
        }
        if let (Some(f), Some((_, _, last))) = (&s.final_text, s.partials.last()) {
            finals_with_partials += 1;
            if normalize_words(f) != normalize_words(last) {
                final_differs += 1;
            }
        }
    }

    let hypothesis = finals_in_order.join(" ");
    let (ws, wd, wi, wn) = wer(&normalize_words(reference), &normalize_words(&hypothesis));
    let speech_s = speech_samples as f64 / f64::from(SAMPLE_RATE);
    let full_calls = Counters::get(&counters.full_calls);
    let full_s = Counters::get(&counters.full_time_us) as f64 / 1e6;
    let frac = |num: usize, den: usize| {
        if den == 0 {
            f64::NAN
        } else {
            num as f64 / den as f64
        }
    };

    RunMetrics {
        model: model.to_owned(),
        fixture: fixture.to_owned(),
        realtime,
        duration_s,
        speech_s,
        load_ms,
        state_create_ms: Counters::get(&counters.state_create_us) as f64 / 1000.0,
        utterances: utts.len(),
        partials,
        finals: finals_in_order.len(),
        ttfp_med_ms: median(&mut ttfp),
        ttfp_max_ms: ttfp.iter().copied().fold(f64::NAN, f64::max),
        ttfp_audio_med_ms: median(&mut ttfp_audio),
        partials_per_s: if speech_s > 0.0 {
            partials as f64 / speech_s
        } else {
            f64::NAN
        },
        partial_latency_med_ms: median(&mut partial_lat),
        partial_latency_p95_ms: percentile(&mut partial_lat, 0.95),
        final_latency_med_ms: median(&mut final_lat),
        stability: 1.0 - frac(prefix_changes, pairs),
        mean_retracted_words: frac(retracted, pairs),
        final_differs_frac: frac(final_differs, finals_with_partials),
        wer: frac(ws + wd + wi, wn),
        wer_s: ws,
        wer_d: wd,
        wer_i: wi,
        wer_n: wn,
        rtf: full_s / duration_s,
        wall_rtf: result.wall_secs / duration_s,
        full_calls,
        full_med_ms: if full_calls == 0 {
            f64::NAN
        } else {
            full_s * 1000.0 / full_calls as f64
        },
        peak_rss_mb: peak_rss_mb(),
        dropped_audio_chunks: result.dropped_chunks,
        dropped_events: Counters::get(&counters.dropped_events),
        gap_samples: Counters::get(&counters.gap_samples),
        delayed: Counters::get(&counters.delayed_count),
        hallucinations: Counters::get(&counters.hallucinations_dropped),
        forced_splits: Counters::get(&counters.forced_splits),
        drain_timed_out: result.drain_timed_out,
        hypothesis,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        normalize_words(s)
    }

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(
            w("Add a Pydantic model, isn't it?"),
            ["add", "a", "pydantic", "model", "isn't", "it"]
        );
        assert_eq!(w("'quoted' DynamoDB."), ["quoted", "dynamodb"]);
    }

    #[test]
    fn wer_identical_is_zero() {
        assert_eq!(wer(&w("a b c d"), &w("a b c d")), (0, 0, 0, 4));
    }

    #[test]
    fn wer_one_substitution_in_four() {
        assert_eq!(wer(&w("a b c d"), &w("a x c d")), (1, 0, 0, 4));
    }

    #[test]
    fn wer_deletion_and_insertion() {
        assert_eq!(wer(&w("a b c d"), &w("a c d")), (0, 1, 0, 4));
        assert_eq!(wer(&w("a b c d"), &w("a b z c d")), (0, 0, 1, 4));
    }

    #[test]
    fn wer_empty_hypothesis_is_all_deletions() {
        assert_eq!(wer(&w("a b c"), &w("")), (0, 3, 0, 3));
    }

    #[test]
    fn median_and_percentile() {
        assert!(median(&mut []).is_nan());
        assert!((median(&mut [3.0, 1.0, 2.0]) - 2.0).abs() < f64::EPSILON);
        assert!((percentile(&mut [1.0, 2.0, 3.0, 4.0, 100.0], 0.95) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn nan_metrics_round_trip_through_json() {
        let m = RunMetrics {
            ttfp_med_ms: f64::NAN,
            wer: 0.5,
            ..RunMetrics::default()
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"ttfp_med_ms\":null"));
        let back: RunMetrics = serde_json::from_str(&json).unwrap();
        assert!(back.ttfp_med_ms.is_nan());
        assert!((back.wer - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn rss_is_positive() {
        assert!(peak_rss_mb() > 1.0);
    }
}
