//! Reading and writing WAV, plus rate conversion (windowed sinc, done once on load).

use anyhow::{anyhow, Result};
use std::path::Path;

pub struct AudioFile {
    pub frames: Vec<[f32; 2]>,
    pub sample_rate: u32,
    pub name: String,
}

impl AudioFile {
    pub fn duration_s(&self) -> f32 {
        self.frames.len() as f32 / self.sample_rate.max(1) as f32
    }
}

/// Load a WAV file (PCM 8/16/24/32-bit or float) as stereo f32.
pub fn load_wav(path: &Path) -> Result<AudioFile> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let ch = spec.channels.max(1) as usize;
    let max = (1i64 << (spec.bits_per_sample.saturating_sub(1))) as f32;

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|v| v as f32 / max)
            .collect(),
    };

    let frames: Vec<[f32; 2]> = samples
        .chunks(ch)
        .map(|f| {
            let l = f.first().copied().unwrap_or(0.0);
            let r = if ch > 1 { f[1] } else { l };
            [l, r]
        })
        .collect();

    if frames.is_empty() {
        return Err(anyhow!("The file contains no audio data"));
    }
    Ok(AudioFile {
        frames,
        sample_rate: spec.sample_rate,
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    })
}

/// Write a mono array to a 32-bit float WAV, for a capture or an impulse response.
pub fn save_wav_mono(path: &Path, data: &[f32], sample_rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in data {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

/// Blackman window for the sinc kernel.
fn blackman(i: usize, n: usize) -> f32 {
    if n <= 1 {
        return 1.0;
    }
    let x = 2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32;
    0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos()
}

#[inline]
fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-6 {
        1.0
    } else {
        let p = std::f32::consts::PI * x;
        p.sin() / p
    }
}

/// Rate conversion by windowed sinc interpolation. It runs offline, so quality wins.
pub fn resample(input: &[f32], from_hz: f32, to_hz: f32) -> Vec<f32> {
    if input.is_empty() || (from_hz - to_hz).abs() < 0.5 {
        return input.to_vec();
    }
    let ratio = to_hz / from_hz;
    let out_len = (input.len() as f64 * ratio as f64).round() as usize;
    // When going down in rate the cutoff must come down too, or it aliases.
    let cutoff = if ratio < 1.0 { ratio } else { 1.0 } * 0.95;
    const TAPS: usize = 32; // half kernel width, in source samples
    let n_win = TAPS * 2 + 1;

    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src = i as f32 / ratio;
        let center = src.floor() as i64;
        let mut acc = 0.0f32;
        let mut norm = 0.0f32;
        for k in -(TAPS as i64)..=(TAPS as i64) {
            let idx = center + k;
            if idx < 0 || idx as usize >= input.len() {
                continue;
            }
            let dist = src - idx as f32;
            let w = blackman((k + TAPS as i64) as usize, n_win);
            let h = cutoff * sinc(cutoff * dist) * w;
            acc += input[idx as usize] * h;
            norm += h;
        }
        out.push(if norm.abs() > 1e-9 { acc / norm } else { acc });
    }
    out
}

/// Rate conversion for a stereo signal.
pub fn resample_stereo(frames: &[[f32; 2]], from_hz: f32, to_hz: f32) -> Vec<[f32; 2]> {
    if frames.is_empty() || (from_hz - to_hz).abs() < 0.5 {
        return frames.to_vec();
    }
    let l: Vec<f32> = frames.iter().map(|f| f[0]).collect();
    let r: Vec<f32> = frames.iter().map(|f| f[1]).collect();
    let lr = resample(&l, from_hz, to_hz);
    let rr = resample(&r, from_hz, to_hz);
    lr.into_iter().zip(rr).map(|(a, b)| [a, b]).collect()
}

/// Export a (frequency, dB) curve as a REW / FRD style text file.
pub fn save_frd(path: &Path, freqs: &[f32], db: &[f32], header: &str) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::File::create(path)?;
    writeln!(f, "* {header}")?;
    writeln!(f, "* Freq(Hz) SPL(dB)")?;
    for (fr, d) in freqs.iter().zip(db.iter()) {
        writeln!(f, "{fr:.3} {d:.4}")?;
    }
    Ok(())
}
