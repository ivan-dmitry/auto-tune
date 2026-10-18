//! Real-time spectrum analyser (RTA) using Welch's method: 50 % overlap, windowed
//! blocks, FFT, then power averaging.

use crate::dsp::fft::Fft;
use crate::dsp::lin_to_db;
use crate::dsp::window::{coherent_sum, WindowKind};

#[derive(Copy, Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Averaging {
    /// No averaging; every block is a fresh snapshot.
    None,
    /// Exponential moving average with a time constant of `tau` seconds.
    Exponential { tau: f32 },
    /// Unlimited running average, for measuring a steady pink-noise signal.
    Infinite,
    /// Moving average over the last N blocks.
    Moving { blocks: usize },
}

impl Averaging {
    pub fn label(&self) -> &'static str {
        match self {
            Averaging::None => "Instant",
            Averaging::Exponential { .. } => "Exponential",
            Averaging::Infinite => "Infinite",
            Averaging::Moving { .. } => "Moving (N blocks)",
        }
    }
}

pub struct Analyzer {
    pub sr: f32,
    pub fft_size: usize,
    pub window: WindowKind,
    pub averaging: Averaging,
    pub overlap: f32,

    win: Vec<f32>,
    win_sum: f32,
    fft: Fft,

    /// Samples waiting for a full block.
    pending: Vec<f32>,
    /// Averaged power per bin, in squared peak-amplitude units.
    avg: Vec<f32>,
    history: std::collections::VecDeque<Vec<f32>>,
    pub blocks: u64,
    peak: Vec<f32>,
    pub peak_hold: bool,
}

impl Analyzer {
    pub fn new(sr: f32, fft_size: usize, window: WindowKind) -> Self {
        let win = window.make(fft_size);
        let win_sum = coherent_sum(&win);
        Self {
            sr,
            fft_size,
            window,
            averaging: Averaging::Exponential { tau: 1.0 },
            overlap: 0.5,
            win,
            win_sum,
            fft: Fft::new(),
            pending: Vec::with_capacity(fft_size * 2),
            avg: vec![0.0; fft_size / 2 + 1],
            history: Default::default(),
            blocks: 0,
            peak: vec![0.0; fft_size / 2 + 1],
            peak_hold: false,
        }
    }

    pub fn reconfigure(&mut self, sr: f32, fft_size: usize, window: WindowKind) {
        if (self.sr - sr).abs() < 0.01 && self.fft_size == fft_size && self.window == window {
            return;
        }
        self.sr = sr;
        self.fft_size = fft_size;
        self.window = window;
        self.win = window.make(fft_size);
        self.win_sum = coherent_sum(&self.win);
        self.avg = vec![0.0; fft_size / 2 + 1];
        self.peak = vec![0.0; fft_size / 2 + 1];
        self.pending.clear();
        self.history.clear();
        self.blocks = 0;
    }

    pub fn reset(&mut self) {
        self.avg.iter_mut().for_each(|v| *v = 0.0);
        self.peak.iter_mut().for_each(|v| *v = 0.0);
        self.history.clear();
        self.blocks = 0;
    }

    pub fn reset_peak(&mut self) {
        self.peak.iter_mut().for_each(|v| *v = 0.0);
    }

    fn hop(&self) -> usize {
        let h = (self.fft_size as f32 * (1.0 - self.overlap.clamp(0.0, 0.9))) as usize;
        h.max(1)
    }

    /// Feed in new samples; returns how many FFT blocks were processed.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        self.pending.extend_from_slice(samples);
        let n = self.fft_size;
        let hop = self.hop();
        let mut done = 0usize;
        while self.pending.len() >= n {
            let block: Vec<f32> = self
                .pending
                .iter()
                .take(n)
                .zip(self.win.iter())
                .map(|(x, w)| x * w)
                .collect();
            let spec = self.fft.forward(&block, n);
            let scale = 2.0 / self.win_sum;
            let mut power = Vec::with_capacity(spec.len());
            for (k, c) in spec.iter().enumerate() {
                // DC and Nyquist bins are not doubled.
                let s = if k == 0 || (n % 2 == 0 && k == spec.len() - 1) {
                    scale * 0.5
                } else {
                    scale
                };
                let a = c.norm() * s;
                power.push(a * a);
            }
            self.accumulate(power);
            done += 1;
            self.blocks += 1;
            self.pending.drain(..hop);
        }
        // Guard against unbounded growth if the UI stalls.
        if self.pending.len() > n * 64 {
            let cut = self.pending.len() - n * 8;
            self.pending.drain(..cut);
        }
        done
    }

    fn accumulate(&mut self, power: Vec<f32>) {
        if self.peak_hold {
            for (p, v) in self.peak.iter_mut().zip(power.iter()) {
                if *v > *p {
                    *p = *v;
                }
            }
        }
        match self.averaging {
            Averaging::None => self.avg = power,
            Averaging::Infinite => {
                let n = self.blocks as f32 + 1.0;
                for (a, v) in self.avg.iter_mut().zip(power.iter()) {
                    *a += (*v - *a) / n;
                }
            }
            Averaging::Exponential { tau } => {
                let block_dt = self.hop() as f32 / self.sr.max(1.0);
                let alpha = if tau <= 0.0 {
                    1.0
                } else {
                    1.0 - (-block_dt / tau).exp()
                };
                for (a, v) in self.avg.iter_mut().zip(power.iter()) {
                    *a += alpha * (*v - *a);
                }
            }
            Averaging::Moving { blocks } => {
                let blocks = blocks.max(1);
                self.history.push_back(power);
                while self.history.len() > blocks {
                    self.history.pop_front();
                }
                let cnt = self.history.len() as f32;
                for (k, a) in self.avg.iter_mut().enumerate() {
                    let s: f32 = self.history.iter().map(|h| h[k]).sum();
                    *a = s / cnt;
                }
            }
        }
    }

    /// Peak amplitude per bin, linear, where 1.0 is full scale.
    pub fn amplitude(&self) -> Vec<f32> {
        self.avg.iter().map(|p| p.sqrt()).collect()
    }

    pub fn peak_amplitude(&self) -> Vec<f32> {
        self.peak.iter().map(|p| p.sqrt()).collect()
    }

    /// Spectrum in dBFS, where 0 dB is a full-scale sine.
    pub fn magnitude_db(&self) -> Vec<f32> {
        self.avg.iter().map(|p| lin_to_db(p.sqrt())).collect()
    }

    pub fn freqs(&self) -> Vec<f32> {
        let n = self.fft_size;
        (0..self.avg.len())
            .map(|k| k as f32 * self.sr / n as f32)
            .collect()
    }

    pub fn resolution_hz(&self) -> f32 {
        self.sr / self.fft_size as f32
    }
}

/// Instantaneous level of a block of samples.
#[derive(Copy, Clone, Debug, Default)]
pub struct LevelMeter {
    pub rms_db: f32,
    pub peak_db: f32,
    pub clipped: bool,
}

pub fn measure_level(samples: &[f32]) -> LevelMeter {
    if samples.is_empty() {
        return LevelMeter {
            rms_db: -120.0,
            peak_db: -120.0,
            clipped: false,
        };
    }
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    for &s in samples {
        sum += (s as f64) * (s as f64);
        let a = s.abs();
        if a > peak {
            peak = a;
        }
    }
    let rms = (sum / samples.len() as f64).sqrt() as f32;
    LevelMeter {
        rms_db: lin_to_db(rms),
        peak_db: lin_to_db(peak),
        clipped: peak >= 0.999,
    }
}
