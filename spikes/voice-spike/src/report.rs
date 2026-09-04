//! Plain-text output: per-run timeline and the bench table.

use crate::feeder::FeedResult;
use crate::metrics::RunMetrics;

fn f(v: f64, prec: usize) -> String {
    if v.is_nan() {
        "-".to_owned()
    } else {
        format!("{v:.prec$}")
    }
}

pub fn print_timeline(result: &FeedResult) {
    println!(
        "{:>9} {:>9} {:>8} {:>4} {:>4}  event",
        "wall_ms", "audio_ms", "lat_ms", "sess", "seq"
    );
    for e in &result.timeline {
        println!(
            "{:>9.0} {:>9.0} {:>8} {:>4} {:>4}  {}",
            e.wall_ms,
            e.audio_ms,
            e.latency_ms.map_or("-".to_owned(), |l| format!("{l:.0}")),
            e.event.session,
            e.event.sequence,
            e.event.label()
        );
    }
}

pub fn print_summary(m: &RunMetrics) {
    println!(
        "--- {} / {}: load {}ms  utt {}  partials {} ({}/s)  ttfp med {}ms max {}ms (audio {}ms)  \
         lat med {}ms p95 {}ms  final lat {}ms  stability {}  retract {}  final!=last {}  \
         WER {}% (S{} D{} I{} / N{})  rtf {}  wall_rtf {}  full {}x med {}ms  rss {}MB  \
         drops audio {} events {} gap {} delayed {} halluc {} splits {}{}",
        m.model,
        m.fixture,
        f(m.load_ms, 0),
        m.utterances,
        m.partials,
        f(m.partials_per_s, 2),
        f(m.ttfp_med_ms, 0),
        f(m.ttfp_max_ms, 0),
        f(m.ttfp_audio_med_ms, 0),
        f(m.partial_latency_med_ms, 0),
        f(m.partial_latency_p95_ms, 0),
        f(m.final_latency_med_ms, 0),
        f(m.stability, 2),
        f(m.mean_retracted_words, 2),
        f(m.final_differs_frac, 2),
        f(m.wer * 100.0, 1),
        m.wer_s,
        m.wer_d,
        m.wer_i,
        m.wer_n,
        f(m.rtf, 3),
        f(m.wall_rtf, 2),
        m.full_calls,
        f(m.full_med_ms, 0),
        f(m.peak_rss_mb, 0),
        m.dropped_audio_chunks,
        m.dropped_events,
        m.gap_samples,
        m.delayed,
        m.hallucinations,
        m.forced_splits,
        if m.drain_timed_out {
            "  DRAIN TIMEOUT"
        } else {
            ""
        },
    );
    println!("    hypothesis: {}", m.hypothesis);
}

pub fn print_table(rows: &[RunMetrics]) {
    println!(
        "{:<22} {:<28} {:>6} {:>7} {:>8} {:>8} {:>7} {:>7} {:>6} {:>7} {:>6} {:>6} {:>6} {:>6} {:>5}",
        "model",
        "fixture",
        "dur_s",
        "load_ms",
        "ttfp_med",
        "ttfp_max",
        "lat_med",
        "lat_p95",
        "part/s",
        "stab",
        "retr",
        "WER%",
        "rtf",
        "rss_MB",
        "drops"
    );
    for m in rows {
        println!(
            "{:<22} {:<28} {:>6} {:>7} {:>8} {:>8} {:>7} {:>7} {:>6} {:>7} {:>6} {:>6} {:>6} {:>6} {:>5}",
            m.model,
            m.fixture,
            f(m.duration_s, 1),
            f(m.load_ms, 0),
            f(m.ttfp_med_ms, 0),
            f(m.ttfp_max_ms, 0),
            f(m.partial_latency_med_ms, 0),
            f(m.partial_latency_p95_ms, 0),
            f(m.partials_per_s, 2),
            f(m.stability, 2),
            f(m.mean_retracted_words, 2),
            f(m.wer * 100.0, 1),
            f(m.rtf, 3),
            f(m.peak_rss_mb, 0),
            m.dropped_audio_chunks + m.dropped_events + m.delayed + m.forced_splits,
        );
    }
}

/// One aggregate row per model: means over fixtures, WER pooled over words.
#[must_use]
pub fn aggregate(rows: &[RunMetrics]) -> Vec<RunMetrics> {
    let mut models: Vec<String> = rows.iter().map(|m| m.model.clone()).collect();
    models.dedup();
    models
        .into_iter()
        .map(|model| {
            let rs: Vec<&RunMetrics> = rows.iter().filter(|m| m.model == model).collect();
            let mean = |g: fn(&RunMetrics) -> f64| {
                let vals: Vec<f64> = rs.iter().map(|m| g(m)).filter(|v| !v.is_nan()).collect();
                if vals.is_empty() {
                    f64::NAN
                } else {
                    vals.iter().sum::<f64>() / vals.len() as f64
                }
            };
            let (s, d, i, w): (usize, usize, usize, usize) =
                rs.iter().fold((0, 0, 0, 0), |a, m| {
                    (a.0 + m.wer_s, a.1 + m.wer_d, a.2 + m.wer_i, a.3 + m.wer_n)
                });
            RunMetrics {
                model,
                fixture: format!("ALL ({} runs)", rs.len()),
                duration_s: rs.iter().map(|m| m.duration_s).sum(),
                load_ms: mean(|m| m.load_ms),
                ttfp_med_ms: mean(|m| m.ttfp_med_ms),
                ttfp_max_ms: rs.iter().map(|m| m.ttfp_max_ms).fold(f64::NAN, f64::max),
                partial_latency_med_ms: mean(|m| m.partial_latency_med_ms),
                partial_latency_p95_ms: rs
                    .iter()
                    .map(|m| m.partial_latency_p95_ms)
                    .fold(f64::NAN, f64::max),
                partials_per_s: mean(|m| m.partials_per_s),
                stability: mean(|m| m.stability),
                mean_retracted_words: mean(|m| m.mean_retracted_words),
                wer: if w == 0 {
                    f64::NAN
                } else {
                    (s + d + i) as f64 / w as f64
                },
                wer_s: s,
                wer_d: d,
                wer_i: i,
                wer_n: w,
                rtf: rs.iter().map(|m| m.rtf * m.duration_s).sum::<f64>()
                    / rs.iter().map(|m| m.duration_s).sum::<f64>(),
                peak_rss_mb: rs.iter().map(|m| m.peak_rss_mb).fold(f64::NAN, f64::max),
                dropped_audio_chunks: rs.iter().map(|m| m.dropped_audio_chunks).sum(),
                dropped_events: rs.iter().map(|m| m.dropped_events).sum(),
                delayed: rs.iter().map(|m| m.delayed).sum(),
                forced_splits: rs.iter().map(|m| m.forced_splits).sum(),
                utterances: rs.iter().map(|m| m.utterances).sum(),
                partials: rs.iter().map(|m| m.partials).sum(),
                ..Default::default()
            }
        })
        .collect()
}
