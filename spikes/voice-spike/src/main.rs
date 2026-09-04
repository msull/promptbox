//! CLI for the voice spike: `fixtures`, `run`, `bench`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Args, Parser, Subcommand};

use voice_spike::audio::WavAudio;
use voice_spike::engine::EngineConfig;
use voice_spike::feeder::{FeedOptions, feed};
use voice_spike::metrics::RunMetrics;
use voice_spike::whisper_engine::WhisperEngine;
use voice_spike::{crate_dir, read_reference, report, resolve_model, script_stem};

#[derive(Parser)]
#[command(about = "Milestone 0 voice spike: whisper.cpp streaming feasibility")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print whisper.cpp build/system info.
    Info,
    /// Synthesize WAV fixtures from fixtures/scripts/*.txt with macOS `say`.
    Fixtures {
        #[arg(long, default_value = "Samantha,Daniel")]
        voices: String,
        #[arg(long, default_value = "180,220")]
        rates: String,
        /// Print a per-20ms dBFS profile summary for each fixture.
        #[arg(long)]
        profile: bool,
    },
    /// Stream one WAV through the engine and print the event timeline.
    Run {
        #[arg(long)]
        model: String,
        #[arg(long)]
        wav: PathBuf,
        /// Reference transcript; defaults to the matching fixtures/scripts file.
        #[arg(long)]
        reference: Option<PathBuf>,
        #[command(flatten)]
        tune: Tune,
        /// Stop the session after this much audio (ms) has been pushed.
        #[arg(long)]
        stop_after_ms: Option<u64>,
        /// After stopping, start a new session and continue feeding.
        #[arg(long)]
        restart: bool,
        /// Print only the JSON metrics line (used by `bench`).
        #[arg(long)]
        json: bool,
        /// Suppress the timeline.
        #[arg(long)]
        quiet: bool,
    },
    /// Run every fixture through every model (each run in a subprocess).
    Bench {
        #[arg(long, default_value = "base.en,small.en")]
        models: String,
        /// Substring filter on fixture file names.
        #[arg(long, default_value = "Samantha_180")]
        fixtures: String,
        #[command(flatten)]
        tune: Tune,
    },
}

#[derive(Args, Clone)]
struct Tune {
    /// Feed as fast as the engine accepts instead of real-time pacing.
    #[arg(long)]
    fast: bool,
    #[arg(long, default_value_t = 100)]
    chunk_ms: u64,
    #[arg(long, default_value_t = 1000)]
    step_ms: u64,
    #[arg(long, default_value_t = 10_000)]
    max_window_ms: u64,
    #[arg(long, default_value_t = 700)]
    hangover_ms: u64,
    #[arg(long, default_value_t = -40.0, allow_negative_numbers = true)]
    vad_db: f32,
    #[arg(long, default_value_t = 4)]
    threads: i32,
    /// Vocabulary hint passed as whisper's initial prompt.
    #[arg(long)]
    hint: Option<String>,
    /// Keep decoder context between windows (default: no context).
    #[arg(long)]
    context: bool,
    #[arg(long, default_value_t = 50)]
    audio_queue: usize,
    #[arg(long, default_value_t = 256)]
    event_queue: usize,
}

impl Tune {
    fn engine_config(&self) -> EngineConfig {
        EngineConfig {
            step_ms: self.step_ms,
            max_window_ms: self.max_window_ms,
            hangover_ms: self.hangover_ms,
            vad_db: self.vad_db,
            threads: self.threads,
            no_context: !self.context,
            hint: self.hint.clone(),
            audio_queue: self.audio_queue,
            event_queue: self.event_queue,
            ..EngineConfig::default()
        }
    }

    fn to_args(&self) -> Vec<String> {
        let mut a = vec![
            "--chunk-ms".into(),
            self.chunk_ms.to_string(),
            "--step-ms".into(),
            self.step_ms.to_string(),
            "--max-window-ms".into(),
            self.max_window_ms.to_string(),
            "--hangover-ms".into(),
            self.hangover_ms.to_string(),
            format!("--vad-db={}", self.vad_db),
            "--threads".into(),
            self.threads.to_string(),
            "--audio-queue".into(),
            self.audio_queue.to_string(),
            "--event-queue".into(),
            self.event_queue.to_string(),
        ];
        if self.fast {
            a.push("--fast".into());
        }
        if self.context {
            a.push("--context".into());
        }
        if let Some(h) = &self.hint {
            a.push("--hint".into());
            a.push(h.clone());
        }
        a
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("error")).init();
    whisper_rs::install_logging_hooks();
    match Cli::parse().cmd {
        Cmd::Info => {
            println!("whisper {}", whisper_rs::get_whisper_version());
            println!("{}", whisper_rs::print_system_info());
            Ok(())
        }
        Cmd::Fixtures {
            voices,
            rates,
            profile,
        } => fixtures(&voices, &rates, profile),
        Cmd::Run {
            model,
            wav,
            reference,
            tune,
            stop_after_ms,
            restart,
            json,
            quiet,
        } => run(
            &model,
            &wav,
            reference.as_deref(),
            &tune,
            stop_after_ms,
            restart,
            json,
            quiet,
        ),
        Cmd::Bench {
            models,
            fixtures,
            tune,
        } => bench(&models, &fixtures, &tune),
    }
}

fn fixtures(voices: &str, rates: &str, profile: bool) -> anyhow::Result<()> {
    let dir = crate_dir().join("fixtures");
    let mut scripts: Vec<PathBuf> = std::fs::read_dir(dir.join("scripts"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    scripts.sort();
    for script in &scripts {
        let stem = script
            .file_stem()
            .and_then(|s| s.to_str())
            .context("stem")?;
        for voice in voices.split(',') {
            for rate in rates.split(',') {
                let out = dir.join(format!("{stem}_{voice}_{rate}.wav"));
                let status = Command::new("say")
                    .args(["-v", voice, "-r", rate, "-o"])
                    .arg(&out)
                    .args(["--file-format=WAVE", "--data-format=LEI16@16000", "-f"])
                    .arg(script)
                    .status()
                    .context("run `say`")?;
                if !status.success() {
                    bail!("say failed for {}", out.display());
                }
                let audio = WavAudio::load(&out)?;
                let prof = audio.frame_db_profile();
                let silent = prof.iter().filter(|d| **d < -40.0).count();
                println!(
                    "{:<40} {:>6.1}s  silent frames {:>4}/{:<4}{}",
                    out.file_name().and_then(|s| s.to_str()).unwrap_or_default(),
                    audio.duration_secs(),
                    silent,
                    prof.len(),
                    if profile {
                        format!("  {}", sparkline(&prof))
                    } else {
                        String::new()
                    }
                );
            }
        }
    }
    Ok(())
}

/// One char per 250 ms: '.' silence, ':' quiet, '#' speech.
fn sparkline(db: &[f32]) -> String {
    db.chunks(12)
        .map(|c| {
            let m = c.iter().copied().fold(f32::MIN, f32::max);
            if m < -50.0 {
                '.'
            } else if m < -35.0 {
                ':'
            } else {
                '#'
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn run(
    model: &str,
    wav: &Path,
    reference: Option<&Path>,
    tune: &Tune,
    stop_after_ms: Option<u64>,
    restart: bool,
    json: bool,
    quiet: bool,
) -> anyhow::Result<()> {
    let model_path = resolve_model(model);
    if !model_path.exists() {
        bail!(
            "model not found: {} (see README for download)",
            model_path.display()
        );
    }
    let audio = WavAudio::load(wav)?;
    let reference = if let Some(p) = reference {
        read_reference(p)?
    } else {
        let p = crate_dir()
            .join("fixtures/scripts")
            .join(format!("{}.txt", script_stem(wav)));
        if p.exists() {
            read_reference(&p)?
        } else {
            String::new()
        }
    };

    let mut engine = WhisperEngine::load(&model_path, tune.engine_config())?;
    let opts = FeedOptions {
        chunk_ms: tune.chunk_ms,
        realtime: !tune.fast,
        stop_after_ms,
        restart,
        drain_timeout: Duration::from_secs(120),
    };
    let result = feed(&mut engine, &audio, &opts)?;
    let counters = engine.counters();
    let metrics = voice_spike::metrics::compute(
        model,
        wav.file_stem().and_then(|s| s.to_str()).unwrap_or_default(),
        !tune.fast,
        audio.duration_secs(),
        engine.load_ms,
        &reference,
        &result,
        &counters,
    );
    if json {
        println!("{}", serde_json::to_string(&metrics)?);
        return Ok(());
    }
    if !quiet {
        report::print_timeline(&result);
    }
    if !reference.is_empty() {
        println!("    reference:  {reference}");
    }
    report::print_summary(&metrics);
    Ok(())
}

fn bench(models: &str, filter: &str, tune: &Tune) -> anyhow::Result<()> {
    let dir = crate_dir().join("fixtures");
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .filter(|p| p.to_string_lossy().contains(filter))
        .collect();
    wavs.sort();
    if wavs.is_empty() {
        bail!(
            "no fixtures matching {filter:?} in {}; run `fixtures` first",
            dir.display()
        );
    }
    let exe = std::env::current_exe()?;
    let mut rows: Vec<RunMetrics> = Vec::new();
    for model in models.split(',') {
        for wav in &wavs {
            eprintln!(
                "bench: {model} / {}",
                wav.file_name().and_then(|s| s.to_str()).unwrap_or_default()
            );
            let out = Command::new(&exe)
                .arg("run")
                .args(["--model", model, "--json"])
                .arg("--wav")
                .arg(wav)
                .args(tune.to_args())
                .output()?;
            if !out.status.success() {
                eprintln!("  failed: {}", String::from_utf8_lossy(&out.stderr));
                continue;
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let line = stdout
                .lines()
                .rev()
                .find(|l| l.starts_with('{'))
                .context("no json line")?;
            let m: RunMetrics = serde_json::from_str(line)?;
            report::print_summary(&m);
            rows.push(m);
        }
    }
    println!();
    report::print_table(&rows);
    println!();
    report::print_table(&report::aggregate(&rows));
    Ok(())
}
