//! Octave banding, 1/N-octave smoothing and interpolation on a log frequency axis.

use crate::dsp::{lin_to_db, power_to_db};

/// Standard ISO base-2 centre frequencies anchored at 1 kHz. `frac` = 1, 3, 6, 12, 24…
pub fn iso_centers(frac: u32, fmin: f32, fmax: f32) -> Vec<f32> {
    let frac = frac.max(1) as f32;
    let mut out = Vec::new();
    // Pick every n for which 1000 * 2^(n/frac) falls inside [fmin, fmax].
    let n_lo = (frac * (fmin / 1000.0).log2()).ceil() as i32;
    let n_hi = (frac * (fmax / 1000.0).log2()).floor() as i32;
    for n in n_lo..=n_hi {
        out.push(1000.0 * 2f32.powf(n as f32 / frac));
    }
    out
}

/// Evenly spaced log frequency grid, `ppo` points per octave. This is the common
/// axis every comparison in the app is done on.
pub fn log_grid(fmin: f32, fmax: f32, ppo: u32) -> Vec<f32> {
    let fmin = fmin.max(1e-3);
    let octaves = (fmax / fmin).log2();
    let n = (octaves * ppo as f32).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| fmin * 2f32.powf(i as f32 * octaves / n as f32))
        .collect()
}

/// Linear interpolation along the log frequency axis. `freqs` must be ascending.
pub fn interp_log(freqs: &[f32], vals: &[f32], f: f32) -> f32 {
    if freqs.is_empty() {
        return 0.0;
    }
    if f <= freqs[0] {
        return vals[0];
    }
    let last = freqs.len() - 1;
    if f >= freqs[last] {
        return vals[last];
    }
    let mut lo = 0usize;
    let mut hi = last;
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if freqs[mid] <= f {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (f0, f1) = (freqs[lo].max(1e-6), freqs[hi].max(1e-6));
    let t = ((f / f0).ln()) / ((f1 / f0).ln());
    vals[lo] + t * (vals[hi] - vals[lo])
}

/// Move a curve onto a different frequency grid.
pub fn resample_log(freqs: &[f32], vals: &[f32], grid: &[f32]) -> Vec<f32> {
    grid.iter().map(|&f| interp_log(freqs, vals, f)).collect()
}

/// One box average in the power domain, over a window of +/- `half` octaves.
/// Uses a running sum, so the cost does not depend on how wide the window is.
fn box_average_octave(freqs: &[f32], lin: &[f32], half: f32) -> Vec<f32> {
    let n = lin.len();
    let lo_mul = 2f32.powf(-half);
    let hi_mul = 2f32.powf(half);

    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + lin[i] as f64;
    }

    let mut out = vec![0.0f32; n];
    let mut lo = 0usize;
    let mut hi = 0usize;
    for i in 0..n {
        let f = freqs[i];
        let f_lo = f * lo_mul;
        let f_hi = f * hi_mul;
        while lo < n && freqs[lo] < f_lo {
            lo += 1;
        }
        if hi < lo {
            hi = lo;
        }
        while hi + 1 < n && freqs[hi + 1] <= f_hi {
            hi += 1;
        }
        out[i] = if lo > hi {
            lin[i]
        } else {
            ((prefix[hi + 1] - prefix[lo]) / (hi - lo + 1) as f64) as f32
        };
    }
    out
}

/// 1/`frac`-octave smoothing of a dB curve, averaged in the power domain.
/// A larger `frac` smooths less: 1/24 keeps more detail than 1/3.
///
/// Two box passes of half the width give a triangular window spanning the same
/// 1/`frac` octave as a weighted window would, at a cost that does not grow with
/// the window. The obvious weighted implementation is quadratic on a linear FFT
/// axis — near 20 kHz a 1/6-octave window covers hundreds of bins — and measured
/// 400 ms per full spectrum, which is why the live analyser used to crawl.
pub fn smooth_octave(freqs: &[f32], db: &[f32], frac: f32) -> Vec<f32> {
    let n = freqs.len();
    if n == 0 || frac <= 0.0 {
        return db.to_vec();
    }
    let half = 0.25 / frac; // each pass covers half the total width
    let lin: Vec<f32> = db.iter().map(|&d| 10f32.powf(d / 10.0)).collect();
    let pass1 = box_average_octave(freqs, &lin, half);
    let pass2 = box_average_octave(freqs, &pass1, half);
    pass2.iter().map(|&p| power_to_db(p)).collect()
}

pub struct BandLevels {
    pub centers: Vec<f32>,
    pub db: Vec<f32>,
}

/// Sum FFT bin power into 1/`frac`-octave bands.
/// `amp[k]` is the peak amplitude of bin k, already window-normalised.
pub fn band_levels(amp: &[f32], sr: f32, n: usize, frac: u32, fmin: f32, fmax: f32) -> BandLevels {
    let centers = iso_centers(frac, fmin, fmax);
    let bw = 2f32.powf(0.5 / frac as f32);
    let df = sr / n as f32;
    let mut db = Vec::with_capacity(centers.len());
    for &fc in &centers {
        let f_lo = fc / bw;
        let f_hi = fc * bw;
        let k_lo = (f_lo / df).ceil().max(1.0) as usize;
        let k_hi = ((f_hi / df).floor() as usize).min(amp.len().saturating_sub(1));
        let mut p = 0.0f32;
        if k_lo <= k_hi {
            for k in k_lo..=k_hi {
                p += amp[k] * amp[k] * 0.5; // RMS power of that sinusoidal component
            }
        } else {
            // Band narrower than one bin: take the nearest bin.
            let k = ((fc / df).round() as usize).min(amp.len().saturating_sub(1));
            p = amp[k] * amp[k] * 0.5;
        }
        db.push(lin_to_db(p.sqrt()));
    }
    BandLevels { centers, db }
}
