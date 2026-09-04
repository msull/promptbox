//! WAV loading and chunking for the feeder.

use std::path::Path;

use anyhow::{Context, bail};

use crate::engine::{AudioChunk, SAMPLE_RATE};

#[derive(Debug, Clone)]
pub struct WavAudio {
    pub samples: Vec<f32>,
}

impl WavAudio {
    /// Loads a 16 kHz 16-bit WAV; stereo is downmixed, anything else is an error.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let mut reader =
            hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
        let spec = reader.spec();
        if spec.sample_rate != SAMPLE_RATE {
            bail!(
                "{}: sample rate {} != {SAMPLE_RATE}",
                path.display(),
                spec.sample_rate
            );
        }
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            bail!("{}: expected 16-bit PCM", path.display());
        }
        let ints: Vec<i16> = reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .with_context(|| format!("read {}", path.display()))?;
        let mut samples = vec![0.0f32; ints.len()];
        whisper_rs::convert_integer_to_float_audio(&ints, &mut samples)
            .map_err(|e| anyhow::anyhow!("convert {}: {e:?}", path.display()))?;
        let samples = match spec.channels {
            1 => samples,
            2 => {
                let mut mono = vec![0.0f32; samples.len() / 2];
                whisper_rs::convert_stereo_to_mono_audio(&samples, &mut mono)
                    .map_err(|e| anyhow::anyhow!("downmix {}: {e:?}", path.display()))?;
                mono
            }
            n => bail!("{}: unsupported channel count {n}", path.display()),
        };
        Ok(Self { samples })
    }

    #[must_use]
    pub fn duration_secs(&self) -> f64 {
        #[allow(clippy::cast_precision_loss)]
        let n = self.samples.len() as f64;
        n / f64::from(SAMPLE_RATE)
    }

    /// Splits into chunks of `chunk_ms`, each tagged with its start offset.
    #[must_use]
    pub fn chunks(&self, chunk_ms: u64) -> Vec<AudioChunk> {
        let n = usize::try_from(chunk_ms * u64::from(SAMPLE_RATE) / 1000).unwrap_or(1600);
        self.samples
            .chunks(n.max(1))
            .enumerate()
            .map(|(i, c)| AudioChunk {
                start_sample: (i * n) as u64,
                samples: c.to_vec(),
            })
            .collect()
    }

    /// Per-20ms RMS in dBFS, for eyeballing silence gaps in fixtures.
    #[must_use]
    pub fn frame_db_profile(&self) -> Vec<f32> {
        self.samples
            .chunks(crate::vad::FRAME_LEN)
            .map(crate::vad::EnergyVad::frame_db)
            .collect()
    }
}
