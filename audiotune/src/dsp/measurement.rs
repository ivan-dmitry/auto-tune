//! Log-sweep measurement: deconvolve to an impulse response, then derive the
//! frequency response, RT60 decay times and harmonic distortion.

use crate::dsp::fft::{next_pow2, Fft};
use crate::dsp::generator::SweepConfig;
use crate::dsp::octave::{interp_log, resample_log, smooth_octave};
use crate::dsp::window::half_hann_rise;
use crate::dsp::{lin_to_db, power_to_db};
use rustfft::num_complex::Complex32;

/// A deconvolved impulse response. The part before the peak is kept, because the
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ImpulseResponse {
    pub sr: f32,
    pub data: Vec<f32>,
    /// Position of the main peak, which is the direct sound.
    pub peak_idx: usize,
    /// System plus acoustic delay in samples (peak_idx - inverse filter length + 1).
    pub delay_samples: i64,
}

impl ImpulseResponse {
    pub fn delay_ms(&self) -> f32 {
        self.delay_samples as f32 * 1000.0 / self.sr
    }

    /// Convert the delay to a distance in metres, after removing the loopback delay.
    pub fn distance_m(&self, system_latency_ms: f32, speed_of_sound: f32) -> f32 {
        ((self.delay_ms() - system_latency_ms) / 1000.0) * speed_of_sound
    }

    pub fn peak_value(&self) -> f32 {
        self.data.get(self.peak_idx).copied().unwrap_or(0.0)
    }
}

/// Deconvolution: linear convolution of the recording with the inverse sweep filter.
pub fn deconvolve(recorded: &[f32], inverse: &[f32], sr: f32, fft: &mut Fft) -> ImpulseResponse {
    if recorded.is_empty() || inverse.is_empty() {
        return ImpulseResponse {
            sr,
            data: vec![0.0],
            peak_idx: 0,
            delay_samples: 0,
        };
    }
    let n = next_pow2(recorded.len() + inverse.len());
    let a = fft.forward(recorded, n);
    let b = fft.forward(inverse, n);
    let prod: Vec<Complex32> = a.iter().zip(b.iter()).map(|(x, y)| x * y).collect();
    let mut data = fft.inverse(&prod, n);
    data.truncate(recorded.len() + inverse.len() - 1);

    let mut peak_idx = 0usize;
    let mut peak = 0.0f32;
    for (i, v) in data.iter().enumerate() {
        let a = v.abs();
        if a > peak {
            peak = a;
            peak_idx = i;
        }
    }
    let delay_samples = peak_idx as i64 - (inverse.len() as i64 - 1);
    ImpulseResponse {
        sr,
        data,
        peak_idx,
        delay_samples,
    }
}

/// Average several captures of the same sweep to lower the noise floor (+3 dB per doubling).
pub fn average_captures(captures: &[Vec<f32>]) -> Vec<f32> {
    if captures.is_empty() {
        return Vec::new();
    }
    let len = captures.iter().map(|c| c.len()).min().unwrap_or(0);
    let k = captures.len() as f32;
    (0..len)
        .map(|i| captures.iter().map(|c| c[i]).sum::<f32>() / k)
        .collect()
}

#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct IrWindow {
    /// How much to keep before the peak, in ms.
    pub pre_ms: f32,
    /// How much to keep after the peak, in ms. This sets the frequency resolution,
    /// which is roughly 1000 / post_ms Hz.
    pub post_ms: f32,
    /// Fraction of the tail that is tapered (0..1) with a half Hann.
    pub taper: f32,
}

impl Default for IrWindow {
    fn default() -> Self {
        // 300 ms gives about 3.3 Hz resolution, enough to see room modes.
        Self {
            pre_ms: 5.0,
            post_ms: 300.0,
            taper: 0.25,
        }
    }
}

impl IrWindow {
    pub fn resolution_hz(&self) -> f32 {
        1000.0 / (self.pre_ms + self.post_ms).max(1e-3)
    }

    /// A short gated window: drops room reflections and leaves only the speaker.
    pub fn gated(ms: f32) -> Self {
        Self {
            pre_ms: 1.0,
            post_ms: ms,
            taper: 0.35,
        }
    }
}

/// Frequency response on the linear frequency grid of the FFT.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct FreqResponse {
    pub freqs: Vec<f32>,
    pub mag_db: Vec<f32>,
    pub phase_deg: Vec<f32>,
    pub sr: f32,
}

impl FreqResponse {
    pub fn is_empty(&self) -> bool {
        self.freqs.is_empty()
    }

    /// The curve smoothed to 1/frac octave and resampled onto a log grid.
    pub fn smoothed_on_grid(&self, grid: &[f32], frac: f32) -> Vec<f32> {
        if self.is_empty() {
            return vec![0.0; grid.len()];
        }
        let on_grid = resample_log(&self.freqs, &self.mag_db, grid);
        if frac <= 0.0 {
            on_grid
        } else {
            smooth_octave(grid, &on_grid, frac)
        }
    }

    pub fn at(&self, f: f32) -> f32 {
        interp_log(&self.freqs, &self.mag_db, f)
    }

    /// Average level in dB across a band, used to set the 0 dB reference.
    pub fn band_average_db(&self, f_lo: f32, f_hi: f32) -> f32 {
        let mut sum = 0.0f64;
        let mut cnt = 0usize;
        for (f, d) in self.freqs.iter().zip(self.mag_db.iter()) {
            if *f >= f_lo && *f <= f_hi {
                sum += 10f64.powf(*d as f64 / 10.0);
                cnt += 1;
            }
        }
        if cnt == 0 {
            0.0
        } else {
            power_to_db((sum / cnt as f64) as f32)
        }
    }

    /// Shift the whole curve by db.
    pub fn offset(&mut self, db: f32) {
        for v in self.mag_db.iter_mut() {
            *v += db;
        }
    }
}

/// Gate a window around the IR peak, then FFT it into a frequency response.
pub fn ir_to_fr(ir: &ImpulseResponse, win: &IrWindow, fft: &mut Fft) -> FreqResponse {
    let sr = ir.sr;
    let pre = (win.pre_ms * 0.001 * sr).round().max(1.0) as usize;
    let post = (win.post_ms * 0.001 * sr).round().max(8.0) as usize;
    let start = ir.peak_idx.saturating_sub(pre);
    let end = (ir.peak_idx + post).min(ir.data.len());
    if end <= start {
        return FreqResponse::default();
    }
    let mut seg: Vec<f32> = ir.data[start..end].to_vec();

    // Short taper on the left to avoid a step, longer taper on the right.
    let left = (pre / 4).max(4).min(seg.len() / 4);
    let right = ((seg.len() as f32) * win.taper.clamp(0.0, 0.9)) as usize;
    if left > 1 {
        let w = half_hann_rise(left);
        for i in 0..left {
            seg[i] *= w[i];
        }
    }
    if right > 1 {
        let w = half_hann_rise(right);
        let n = seg.len();
        for i in 0..right {
            seg[n - 1 - i] *= w[i];
        }
    }

    // Generous zero padding so the curve looks smooth on a log axis.
    let nfft = next_pow2(seg.len() * 4).max(16384);
    let spec = fft.forward(&seg, nfft);
    let freqs: Vec<f32> = (0..spec.len())
        .map(|k| k as f32 * sr / nfft as f32)
        .collect();
    let mag_db: Vec<f32> = spec.iter().map(|c| lin_to_db(c.norm())).collect();

    // Remove the linear phase introduced by the window offset, so phase is readable.
    let mut phase_deg = Vec::with_capacity(spec.len());
    let shift = pre as f32;
    for (k, c) in spec.iter().enumerate() {
        let ph = c.im.atan2(c.re) + 2.0 * std::f32::consts::PI * k as f32 * shift / nfft as f32;
        phase_deg.push(wrap_deg(ph.to_degrees()));
    }

    FreqResponse {
        freqs,
        mag_db,
        phase_deg,
        sr,
    }
}

fn wrap_deg(mut d: f32) -> f32 {
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

#[derive(Copy, Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DecayResult {
    pub edt: f32,
    pub t20: f32,
    pub t30: f32,
    /// Correlation of the T30 regression: the closer to 1, the more trustworthy.
    pub t30_fit: f32,
}

/// Decay times via Schroeder backward integration.
pub fn decay_times(ir: &ImpulseResponse) -> Option<DecayResult> {
    let sr = ir.sr;
    let start = ir.peak_idx;
    if start + (0.05 * sr) as usize >= ir.data.len() {
        return None;
    }
    let tail = &ir.data[start..];
    // Backward energy integration.
    let mut acc = 0.0f64;
    let mut curve: Vec<f64> = vec![0.0; tail.len()];
    for i in (0..tail.len()).rev() {
        acc += (tail[i] as f64) * (tail[i] as f64);
        curve[i] = acc;
    }
    let total = curve[0];
    if total <= 0.0 {
        return None;
    }
    let db: Vec<f32> = curve
        .iter()
        .map(|&v| (10.0 * (v / total).max(1e-20).log10()) as f32)
        .collect();

    let find = |target: f32| -> Option<usize> { db.iter().position(|&v| v <= target) };
    let regress = |lo_db: f32, hi_db: f32| -> Option<(f32, f32)> {
        let i0 = find(lo_db)?;
        let i1 = find(hi_db)?;
        if i1 <= i0 + 8 {
            return None;
        }
        // Linear regression of dB against time.
        let n = (i1 - i0) as f64;
        let (mut sx, mut sy, mut sxy, mut sxx, mut syy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for i in i0..i1 {
            let x = i as f64 / sr as f64;
            let y = db[i] as f64;
            sx += x;
            sy += y;
            sxy += x * y;
            sxx += x * x;
            syy += y * y;
        }
        let den = n * sxx - sx * sx;
        if den.abs() < 1e-18 {
            return None;
        }
        let slope = (n * sxy - sx * sy) / den; // dB/s, negative
        if slope >= -1e-6 {
            return None;
        }
        let r_num = n * sxy - sx * sy;
        let r_den = ((n * sxx - sx * sx) * (n * syy - sy * sy)).sqrt().max(1e-18);
        Some(((-60.0 / slope) as f32, (r_num / r_den).abs() as f32))
    };

    let edt = regress(0.0, -10.0).map(|r| r.0).unwrap_or(0.0);
    let t20 = regress(-5.0, -25.0).map(|r| r.0).unwrap_or(0.0);
    let (t30, fit) = regress(-5.0, -35.0).unwrap_or((0.0, 0.0));
    Some(DecayResult {
        edt,
        t20,
        t30,
        t30_fit: fit,
    })
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DistortionResult {
    /// Level of each harmonic in dB relative to the fundamental; index 0 is the 2nd.
    pub harmonics_db: Vec<f32>,
    /// Total THD in percent, estimated across the whole sweep range.
    pub thd_percent: f32,
}

/// Estimate harmonic distortion from the impulse response of a log sweep.
///
/// In a log sweep the nth harmonic arrives ahead of the main impulse by exactly
/// dt = T * ln(n) / ln(f2/f1), so each one can be gated out on its own.
pub fn harmonic_distortion(
    ir: &ImpulseResponse,
    cfg: &SweepConfig,
    max_order: usize,
) -> DistortionResult {
    let sr = ir.sr;
    let f1 = cfg.f_start.max(1.0);
    let f2 = cfg.f_end.min(sr * 0.499).max(f1 * 1.01);
    let lg = (f2 / f1).ln();
    let t = cfg.duration_s;

    let energy_around = |center: i64, half: usize| -> f64 {
        let lo = (center - half as i64).max(0) as usize;
        let hi = ((center + half as i64) as usize).min(ir.data.len());
        if lo >= hi {
            return 0.0;
        }
        ir.data[lo..hi]
            .iter()
            .map(|&v| (v as f64) * (v as f64))
            .sum()
    };

    let peak = ir.peak_idx as i64;
    // Half window: less than half the gap to the next harmonic, so they do not mix.
    let d2 = (t * (2f32).ln() / lg * sr) as i64;
    let half = ((d2 as f32 * 0.4).max(64.0)) as usize;
    let fund = energy_around(peak, half).max(1e-30);

    let mut harmonics_db = Vec::new();
    let mut harm_energy = 0.0f64;
    for n in 2..=max_order.max(2) {
        let dt = (t * (n as f32).ln() / lg * sr) as i64;
        let e = energy_around(peak - dt, half);
        harm_energy += e;
        harmonics_db.push(power_to_db((e / fund) as f32));
    }
    let thd = (harm_energy / fund).max(0.0).sqrt() * 100.0;
    DistortionResult {
        harmonics_db,
        thd_percent: thd as f32,
    }
}

/// Estimate the signal-to-noise ratio of a measurement: energy around the IR peak
/// against energy far into the tail, where only noise remains.
pub fn estimate_snr_db(ir: &ImpulseResponse) -> f32 {
    let n = ir.data.len();
    if n < 1000 {
        return 0.0;
    }
    let sig_hi = (ir.peak_idx + (0.05 * ir.sr) as usize).min(n);
    let sig: f64 = ir.data[ir.peak_idx..sig_hi]
        .iter()
        .map(|&v| (v as f64) * (v as f64))
        .sum();
    let noise_start = n - n / 8;
    let noise: f64 = ir.data[noise_start..]
        .iter()
        .map(|&v| (v as f64) * (v as f64))
        .sum::<f64>()
        / ((n - noise_start) as f64)
        * ((sig_hi - ir.peak_idx) as f64);
    if noise <= 0.0 {
        return 99.0;
    }
    (10.0 * (sig / noise).max(1e-12).log10()) as f32
}
