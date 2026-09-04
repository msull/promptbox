//! Microphone capture. The device callback does bounded work only: it
//! copies samples into a lock-free ring buffer and counts what did not fit.
//! A worker thread drains the ring, downmixes, resamples to 16 kHz mono,
//! measures level, and sends 20 ms chunks tagged with monotonically
//! increasing sample offsets over a bounded channel. Dropped samples show up
//! as offset jumps, which the speech engine reports as `AudioGap`.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};

use crate::adapters::speech::vad::EnergyVad;
use crate::ports::engine::{AudioChunk, SAMPLE_RATE};

const CHUNK_SAMPLES: usize = 320; // 20 ms at 16 kHz
const RING_SECONDS: usize = 2;
const CHANNEL_CHUNKS: usize = 100; // 2 s of 20 ms chunks

/// Shared counters and the latest input level.
#[derive(Debug)]
pub struct MicStats {
    /// Device samples (all channels) the callback could not enqueue.
    pub dropped_device_samples: AtomicU64,
    /// Chunks the worker could not send because the app fell behind.
    pub dropped_chunks: AtomicU64,
    /// Latest 20 ms level in dBFS, stored as f32 bits.
    level_bits: AtomicU32,
    pub error: Mutex<Option<String>>,
}

impl Default for MicStats {
    fn default() -> Self {
        Self {
            dropped_device_samples: AtomicU64::new(0),
            dropped_chunks: AtomicU64::new(0),
            level_bits: AtomicU32::new((-120.0f32).to_bits()),
            error: Mutex::new(None),
        }
    }
}

impl MicStats {
    #[must_use]
    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Relaxed))
    }

    fn set_level_db(&self, db: f32) {
        self.level_bits.store(db.to_bits(), Ordering::Relaxed);
    }
}

pub struct MicCapture {
    _stream: Option<cpal::Stream>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    stats: Arc<MicStats>,
    pub device_name: String,
}

impl MicCapture {
    /// Opens the default input device and starts streaming chunks into the
    /// returned receiver.
    pub fn start() -> Result<(Self, Receiver<AudioChunk>), String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no default input device".to_owned())?;
        let device_name = device
            .description()
            .map_or_else(|_| "unknown".to_owned(), |d| d.name().to_owned());
        let supported = device
            .default_input_config()
            .map_err(|e| format!("input config: {e}"))?;
        let config: cpal::StreamConfig = supported.config();
        let channels = usize::from(config.channels);
        let rate = config.sample_rate;

        let stats = Arc::new(MicStats::default());
        let ring = HeapRb::<f32>::new(rate as usize * channels * RING_SECONDS);
        let (mut prod, cons) = ring.split();
        let (tx, rx) = sync_channel::<AudioChunk>(CHANNEL_CHUNKS);

        let cb_stats = Arc::clone(&stats);
        let err_stats = Arc::clone(&stats);
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let written = prod.push_slice(data);
                    if written < data.len() {
                        cb_stats
                            .dropped_device_samples
                            .fetch_add((data.len() - written) as u64, Ordering::Relaxed);
                    }
                },
                move |e| {
                    *err_stats.error.lock().expect("mic error mutex") = Some(e.to_string());
                },
                None,
            )
            .map_err(|e| format!("open input stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("start input stream: {e}"))?;

        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            cons,
            tx,
            stats: Arc::clone(&stats),
            stop: Arc::clone(&stop),
            channels,
            resampler: Resampler::new(rate, SAMPLE_RATE),
            chunker: Chunker::default(),
            scratch: vec![0.0; rate as usize * channels / 50],
            seen_dropped: 0,
        };
        let handle = std::thread::Builder::new()
            .name("mic-capture".into())
            .spawn(move || worker.run())
            .map_err(|e| e.to_string())?;

        Ok((
            Self {
                _stream: Some(stream),
                stop,
                worker: Some(handle),
                stats,
                device_name,
            },
            rx,
        ))
    }

    /// Dev/test source: plays a 16 kHz mono WAV through the same chunking
    /// path, paced in real time, then goes silent. Enabled in the app with
    /// `PROMPTBOX_FAKE_MIC=<path.wav>`.
    pub fn from_wav(path: &std::path::Path) -> Result<(Self, Receiver<AudioChunk>), String> {
        let mut reader =
            hound::WavReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec = reader.spec();
        if spec.sample_rate != SAMPLE_RATE || spec.channels != 1 || spec.bits_per_sample != 16 {
            return Err(format!("{}: need 16 kHz mono 16-bit", path.display()));
        }
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.map(|v| f32::from(v) / 32768.0))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let stats = Arc::new(MicStats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = sync_channel::<AudioChunk>(CHANNEL_CHUNKS);
        let (worker_stats, worker_stop) = (Arc::clone(&stats), Arc::clone(&stop));
        let handle = std::thread::Builder::new()
            .name("wav-capture".into())
            .spawn(move || {
                let mut chunker = Chunker::default();
                let start = std::time::Instant::now();
                let mut fed = 0usize;
                while !worker_stop.load(Ordering::Relaxed) {
                    let due = (start.elapsed().as_millis() as usize * SAMPLE_RATE as usize / 1000)
                        .min(samples.len() + CHUNK_SAMPLES * 500);
                    let end = due.min(samples.len());
                    let mut buf: Vec<f32> = samples[fed.min(end)..end].to_vec();
                    // Past the end of the file, keep feeding silence.
                    buf.resize(due - fed, 0.0);
                    fed = due;
                    for chunk in chunker.push(&buf) {
                        worker_stats.set_level_db(EnergyVad::frame_db(&chunk.samples));
                        if tx.try_send(chunk).is_err() {
                            worker_stats.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
            .map_err(|e| e.to_string())?;
        Ok((
            Self {
                _stream: None,
                stop,
                worker: Some(handle),
                stats,
                device_name: format!("wav:{}", path.display()),
            },
            rx,
        ))
    }

    #[must_use]
    pub fn stats(&self) -> &MicStats {
        &self.stats
    }
}

impl Drop for MicCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

struct Worker<C: Consumer<Item = f32>> {
    cons: C,
    tx: SyncSender<AudioChunk>,
    stats: Arc<MicStats>,
    stop: Arc<AtomicBool>,
    channels: usize,
    resampler: Resampler,
    chunker: Chunker,
    scratch: Vec<f32>,
    seen_dropped: u64,
}

impl<C: Consumer<Item = f32>> Worker<C> {
    fn run(mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            let n = self.cons.pop_slice(&mut self.scratch);
            if n == 0 {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            // Samples the callback dropped never reach us; advance the
            // output offset so the gap is visible downstream.
            let dropped = self.stats.dropped_device_samples.load(Ordering::Relaxed);
            if dropped > self.seen_dropped {
                let frames = (dropped - self.seen_dropped) / self.channels as u64;
                self.chunker.skip(self.resampler.output_len_for(frames));
                self.seen_dropped = dropped;
            }
            let mono = downmix(&self.scratch[..n], self.channels);
            let out = self.resampler.process(&mono);
            for chunk in self.chunker.push(&out) {
                self.stats.set_level_db(EnergyVad::frame_db(&chunk.samples));
                match self.tx.try_send(chunk) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        self.stats.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }
    }
}

/// Averages interleaved channels into mono.
#[must_use]
pub fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
        .collect()
}

/// Linear-interpolation resampler with carry-over between calls. Good
/// enough for speech at 44.1/48 kHz -> 16 kHz; replace if quality matters.
#[derive(Debug)]
pub struct Resampler {
    ratio: f64, // input samples per output sample
    pos: f64,   // fractional read position into `pending`
    pending: Vec<f32>,
}

impl Resampler {
    #[must_use]
    pub fn new(from_hz: u32, to_hz: u32) -> Self {
        Self {
            ratio: f64::from(from_hz) / f64::from(to_hz),
            pos: 0.0,
            pending: Vec::new(),
        }
    }

    /// How many output samples `input_frames` input frames become.
    #[must_use]
    pub fn output_len_for(&self, input_frames: u64) -> u64 {
        (input_frames as f64 / self.ratio).round() as u64
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.pending.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.ratio) as usize + 1);
        while (self.pos as usize) + 1 < self.pending.len() {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(self.pending[i] * (1.0 - frac) + self.pending[i + 1] * frac);
            self.pos += self.ratio;
        }
        let consumed = self.pos as usize;
        self.pending.drain(..consumed.min(self.pending.len()));
        self.pos -= consumed as f64;
        out
    }
}

/// Groups samples into fixed 20 ms chunks with running sample offsets.
#[derive(Debug, Default)]
pub struct Chunker {
    next_start: u64,
    partial: Vec<f32>,
}

impl Chunker {
    /// Records that `n` output samples were lost so offsets jump past them.
    pub fn skip(&mut self, n: u64) {
        if n > 0 {
            self.partial.clear();
            self.next_start += n;
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<AudioChunk> {
        self.partial.extend_from_slice(samples);
        let mut out = Vec::new();
        while self.partial.len() >= CHUNK_SAMPLES {
            let rest = self.partial.split_off(CHUNK_SAMPLES);
            let samples = std::mem::replace(&mut self.partial, rest);
            out.push(AudioChunk {
                start_sample: self.next_start,
                samples,
            });
            self.next_start += CHUNK_SAMPLES as u64;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_channels() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.2, 0.4], 1), vec![0.2, 0.4]);
    }

    #[test]
    fn resampler_halves_sample_count_across_calls() {
        let mut r = Resampler::new(32_000, 16_000);
        let input: Vec<f32> = (0..64).map(|i| i as f32).collect();
        let mut out = Vec::new();
        for part in input.chunks(7) {
            out.extend(r.process(part));
        }
        assert!((31..=32).contains(&out.len()), "got {}", out.len());
        // Interpolated values stay monotonic and on the input line.
        for w in out.windows(2) {
            assert!((w[1] - w[0] - 2.0).abs() < 1e-3);
        }
        assert_eq!(r.output_len_for(48_000), 24_000);
    }

    #[test]
    fn chunker_emits_fixed_chunks_and_skips_offsets_on_loss() {
        let mut c = Chunker::default();
        assert!(c.push(&[0.0; 300]).is_empty());
        let chunks = c.push(&[0.0; 700]);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].start_sample, 0);
        assert_eq!(chunks[2].start_sample, 640);
        c.skip(1000);
        let chunks = c.push(&[0.0; 320]);
        assert_eq!(chunks[0].start_sample, 960 + 1000);
    }
}
