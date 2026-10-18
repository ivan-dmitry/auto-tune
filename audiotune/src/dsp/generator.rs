//! Test signals: Farina log sweep, pink and white noise, sine, band-limited pink.

use crate::dsp::window::apply_fades;
use std::f32::consts::PI;

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SignalKind {
    PinkNoise,
    WhiteNoise,
    Sine,
    LogSweep,
    /// Pink noise limited to one octave band, for listening to a single band.
    PinkBand,
}

impl SignalKind {
    pub fn label(&self) -> &'static str {
        match self {
            SignalKind::PinkNoise => "Pink noise",
            SignalKind::WhiteNoise => "White noise",
            SignalKind::Sine => "Sine",
            SignalKind::LogSweep => "Log sweep",
            SignalKind::PinkBand => "Pink noise, one band",
        }
    }
}

/// Xorshift pseudo-random generator — good enough for noise and safe to run inside
/// an audio callback.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Default for Rng {
    fn default() -> Self {
        Rng(0x1234_5678)
    }
}

impl Rng {
    pub fn next_f32(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Pink noise via Paul Kellet's filter: within 0.05 dB of -3 dB/octave from 10 Hz up.
#[derive(Clone, Debug, Default)]
pub struct PinkNoise {
    rng: Rng,
    b: [f32; 7],
}

impl PinkNoise {
    #[inline]
    pub fn next(&mut self) -> f32 {
        let white = self.rng.next_f32();
        self.b[0] = 0.99886 * self.b[0] + white * 0.0555179;
        self.b[1] = 0.99332 * self.b[1] + white * 0.0750759;
        self.b[2] = 0.96900 * self.b[2] + white * 0.1538520;
        self.b[3] = 0.86650 * self.b[3] + white * 0.3104856;
        self.b[4] = 0.55000 * self.b[4] + white * 0.5329522;
        self.b[5] = -0.7616 * self.b[5] - white * 0.0168980;
        let out = self.b[0]
            + self.b[1]
            + self.b[2]
            + self.b[3]
            + self.b[4]
            + self.b[5]
            + self.b[6]
            + white * 0.5362;
        self.b[6] = white * 0.115926;
        out * 0.11 // normalise roughly into -1..1
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SweepConfig {
    pub f_start: f32,
    pub f_end: f32,
    pub duration_s: f32,
    /// Extra silence recorded after the sweep so the room's decay is captured.
    pub tail_s: f32,
    pub level_db: f32,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
    /// How many times to repeat and average, to push the noise floor down.
    pub repeats: u32,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            f_start: 20.0,
            f_end: 20_000.0,
            duration_s: 6.0,
            tail_s: 1.5,
            level_db: -12.0,
            fade_in_ms: 20.0,
            fade_out_ms: 60.0,
            repeats: 1,
        }
    }
}

impl SweepConfig {
    pub fn total_samples(&self, sr: f32) -> usize {
        ((self.duration_s + self.tail_s) * sr).round() as usize
    }
}

/// Generate a Farina exponential (log) sine sweep.
///
/// x(t) = sin( K * (exp(t/L) - 1) ), with K = w1*T/ln(w2/w1) and L = T/ln(w2/w1).
pub fn log_sweep(cfg: &SweepConfig, sr: f32) -> Vec<f32> {
    let n = (cfg.duration_s * sr).round().max(1.0) as usize;
    let f1 = cfg.f_start.max(1.0);
    let f2 = cfg.f_end.min(sr * 0.499).max(f1 * 1.01);
    let t_span = n as f32 / sr;
    let lg = (f2 / f1).ln();
    let k = 2.0 * PI * f1 * t_span / lg;
    let amp = 10f32.powf(cfg.level_db / 20.0);

    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / sr;
            (k * ((t / t_span * lg).exp() - 1.0)).sin() * amp
        })
        .collect();

    let fi = (cfg.fade_in_ms * 0.001 * sr) as usize;
    let fo = (cfg.fade_out_ms * 0.001 * sr) as usize;
    apply_fades(&mut out, fi, fo);
    out
}

/// Inverse filter of a log sweep: time-reversed, with an amplitude envelope.
/// Convolving the sweep with this gives a Dirac impulse.
pub fn inverse_filter(sweep: &[f32], cfg: &SweepConfig, sr: f32) -> Vec<f32> {
    let n = sweep.len();
    if n == 0 {
        return Vec::new();
    }
    let f1 = cfg.f_start.max(1.0);
    let f2 = cfg.f_end.min(sr * 0.499).max(f1 * 1.01);
    let lg = (f2 / f1).ln();
    let t_span = n as f32 / sr;

    // A log sweep has ~1/f spectral density, because every octave takes the same
    // amount of time, so its amplitude spectrum goes as f^-0.5. For the product of
    // the two spectra to come out flat, the inverse filter needs f^+0.5, which means
    // its time-domain amplitude must be proportional to the instantaneous frequency:
    // +6 dB/octave. The instantaneous frequency at time t of the original sweep is
    // f1*exp(t/T*ln(f2/f1)), so the envelope, normalised to 1 at the high end, is
    // exp((t/T - 1)*ln(f2/f1)).
    let mut inv: Vec<f32> = (0..n)
        .map(|i| {
            let t = (n - 1 - i) as f32 / sr; // matching time in the original sweep
            let env = ((t / t_span - 1.0) * lg).exp();
            sweep[n - 1 - i] * env
        })
        .collect();

    // Normalise so the deconvolved impulse peaks near 1.0 whatever the sweep level.
    let energy: f32 = sweep
        .iter()
        .zip(inv.iter().rev())
        .map(|(a, b)| a * b)
        .sum::<f32>()
        .abs()
        .max(1e-20);
    let s = 1.0 / energy;
    for v in inv.iter_mut() {
        *v *= s;
    }
    inv
}

/// Plain single-frequency sine.
pub fn sine(freq: f32, level_db: f32, samples: usize, sr: f32) -> Vec<f32> {
    let amp = 10f32.powf(level_db / 20.0);
    let w = 2.0 * PI * freq / sr;
    (0..samples).map(|i| (w * i as f32).sin() * amp).collect()
}

/// A fixed-length block of pink noise, for when a pre-built buffer is needed.
pub fn pink_block(samples: usize, level_db: f32) -> Vec<f32> {
    let amp = 10f32.powf(level_db / 20.0);
    let mut p = PinkNoise::default();
    (0..samples).map(|_| p.next() * amp).collect()
}

/// A streaming source for the audio callback: never allocates while running.
pub enum Source {
    Silence,
    Pink {
        gen: PinkNoise,
        amp: f32,
    },
    White {
        rng: Rng,
        amp: f32,
    },
    Sine {
        phase: f32,
        step: f32,
        amp: f32,
    },
    /// Play back a pre-built buffer: a sweep, a music file, and so on.
    Buffer {
        data: std::sync::Arc<Vec<f32>>,
        pos: usize,
        repeat: bool,
        amp: f32,
    },
    /// Pink noise through a band-pass, to audition one band on its own.
    PinkBand {
        gen: PinkNoise,
        bp: crate::dsp::biquad::Biquad,
        amp: f32,
    },
}

impl Source {
    #[inline]
    pub fn next(&mut self) -> Option<f32> {
        match self {
            Source::Silence => Some(0.0),
            Source::Pink { gen, amp } => Some(gen.next() * *amp),
            Source::White { rng, amp } => Some(rng.next_f32() * *amp),
            Source::Sine { phase, step, amp } => {
                let v = phase.sin() * *amp;
                *phase += *step;
                if *phase > 2.0 * PI {
                    *phase -= 2.0 * PI;
                }
                Some(v)
            }
            Source::Buffer {
                data,
                pos,
                repeat,
                amp,
            } => {
                if data.is_empty() {
                    return None;
                }
                if *pos >= data.len() {
                    if *repeat {
                        *pos = 0;
                    } else {
                        return None;
                    }
                }
                let v = data[*pos] * *amp;
                *pos += 1;
                Some(v)
            }
            Source::PinkBand { gen, bp, amp } => Some(bp.process(gen.next()) * *amp),
        }
    }

    /// Samples played so far out of the total; only meaningful for `Buffer`.
    pub fn progress(&self) -> Option<(usize, usize)> {
        match self {
            Source::Buffer { data, pos, .. } => Some((*pos, data.len())),
            _ => None,
        }
    }
}
