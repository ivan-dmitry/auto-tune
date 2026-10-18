//! Automatically build a parametric EQ that pulls the measured response onto target.
//!
//! Two stages:
//!   1. Greedy - each round drops a filter on the largest remaining error, subtracts
//!      its response from the error, and repeats.
//!   2. Refinement - coordinate search over frequency, gain and Q to lower RMS error.
//!
//! Everything is computed on a log frequency grid, so every octave carries the same
//! weight, which is how hearing works.

use crate::dsp::biquad::{bw_to_q, Coeffs, FilterKind, FilterSpec};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FitConfig {
    pub f_min: f32,
    pub f_max: f32,
    pub max_filters: usize,
    pub max_boost_db: f32,
    pub max_cut_db: f32,
    pub min_q: f32,
    pub max_q: f32,
    /// 1/N octave smoothing applied before fitting. More smoothing, fewer junk filters.
    pub smoothing_frac: f32,
    /// Damping (0..1): 1 corrects the whole error, 0.7 corrects 70 % to stay safe.
    pub damping: f32,
    pub use_low_shelf: bool,
    pub use_high_shelf: bool,
    pub refine_passes: u32,
    /// Points per octave of the computation grid.
    pub ppo: u32,
    /// Extra headroom in dB to leave for the preamp.
    pub preamp_headroom_db: f32,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self {
            f_min: 30.0,
            f_max: 16_000.0,
            max_filters: 10,
            max_boost_db: 6.0,
            max_cut_db: 12.0,
            min_q: 0.5,
            max_q: 8.0,
            smoothing_frac: 6.0,
            damping: 0.9,
            use_low_shelf: true,
            use_high_shelf: true,
            refine_passes: 3,
            ppo: 48,
            preamp_headroom_db: 0.5,
        }
    }
}

impl FitConfig {
    /// Ready-made configurations per measurement type.
    pub fn preset_speaker() -> Self {
        Self {
            f_min: 30.0,
            f_max: 16_000.0,
            max_filters: 10,
            max_boost_db: 6.0,
            max_cut_db: 12.0,
            min_q: 0.5,
            max_q: 8.0,
            smoothing_frac: 6.0,
            damping: 0.9,
            ..Default::default()
        }
    }

    /// Only touch the room-mode region, the safest option with one mic at one spot.
    pub fn preset_room_modes() -> Self {
        Self {
            f_min: 20.0,
            f_max: 300.0,
            max_filters: 6,
            max_boost_db: 3.0,
            max_cut_db: 12.0,
            min_q: 1.0,
            max_q: 12.0,
            smoothing_frac: 12.0,
            damping: 0.85,
            use_high_shelf: false,
            ..Default::default()
        }
    }

    pub fn preset_headphone() -> Self {
        Self {
            f_min: 20.0,
            f_max: 18_000.0,
            max_filters: 12,
            max_boost_db: 8.0,
            max_cut_db: 12.0,
            min_q: 0.4,
            max_q: 6.0,
            smoothing_frac: 12.0,
            damping: 1.0,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // intermediate curves kept for plotting and debugging
pub struct FitResult {
    pub filters: Vec<FilterSpec>,
    pub preamp_db: f32,
    pub grid: Vec<f32>,
    /// Error (target - measured) before EQ, with the overall level offset removed.
    pub error_before: Vec<f32>,
    pub error_after: Vec<f32>,
    pub eq_response: Vec<f32>,
    pub rms_before: f32,
    pub rms_after: f32,
}

fn weights(grid: &[f32], f_min: f32, f_max: f32) -> Vec<f32> {
    // Weight of 1 inside the working range, fading to 0 over half an octave outside.
    const EDGE_OCT: f32 = 0.5;
    grid.iter()
        .map(|&f| {
            let lo = ((f / f_min).log2() / EDGE_OCT).clamp(0.0, 1.0);
            let hi = ((f_max / f).log2() / EDGE_OCT).clamp(0.0, 1.0);
            let t = lo.min(hi);
            0.5 * (1.0 - (std::f32::consts::PI * t).cos())
        })
        .collect()
}

fn weighted_rms(err: &[f32], w: &[f32]) -> f32 {
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (e, wi) in err.iter().zip(w.iter()) {
        num += (*wi as f64) * (*e as f64) * (*e as f64);
        den += *wi as f64;
    }
    if den <= 0.0 {
        0.0
    } else {
        (num / den).sqrt() as f32
    }
}

fn response_on_grid(spec: &FilterSpec, grid: &[f32], sr: f32) -> Vec<f32> {
    let c = Coeffs::design(spec, sr);
    grid.iter().map(|&f| c.response_db(f, sr)).collect()
}

/// Estimate the width in octaves of the error bump around index i.
fn estimate_bw(err: &[f32], grid: &[f32], i: usize) -> f32 {
    let v = err[i];
    let half = v.abs() * 0.5;
    let sign = v.signum();
    let mut lo = i;
    while lo > 0 && err[lo] * sign > half {
        lo -= 1;
    }
    let mut hi = i;
    while hi + 1 < err.len() && err[hi] * sign > half {
        hi += 1;
    }
    let f_lo = grid[lo].max(1e-3);
    let f_hi = grid[hi].max(f_lo * 1.001);
    (f_hi / f_lo).log2().clamp(0.05, 6.0)
}

/// Fit the EQ. measured and target must both lie on grid, in dB.
pub fn fit(
    grid: &[f32],
    measured: &[f32],
    target: &[f32],
    sr: f32,
    cfg: &FitConfig,
) -> FitResult {
    let n = grid.len();
    if n == 0 || measured.len() != n || target.len() != n {
        return FitResult::default();
    }
    let w = weights(grid, cfg.f_min, cfg.f_max);

    // Level alignment: EQ should change the shape, not the overall volume.
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for i in 0..n {
        num += (w[i] as f64) * ((target[i] - measured[i]) as f64);
        den += w[i] as f64;
    }
    let offset = if den > 0.0 { (num / den) as f32 } else { 0.0 };

    let error_before: Vec<f32> = (0..n).map(|i| target[i] - measured[i] - offset).collect();
    let mut err = error_before.clone();
    let rms_before = weighted_rms(&err, &w);

    let mut filters: Vec<FilterSpec> = Vec::new();

    for _ in 0..cfg.max_filters {
        // Find the worst deviation.
        let mut best_i = 0usize;
        let mut best_v = 0.0f32;
        for i in 0..n {
            let s = err[i].abs() * w[i];
            if s > best_v {
                best_v = s;
                best_i = i;
            }
        }
        if best_v < 0.25 {
            break; // under 0.25 dB left, another filter would not help
        }

        let f0 = grid[best_i];
        let v = err[best_i];
        let bw = estimate_bw(&err, grid, best_i);
        let q0 = bw_to_q(bw).clamp(cfg.min_q, cfg.max_q);
        let gain = (v * cfg.damping).clamp(-cfg.max_cut_db, cfg.max_boost_db);

        // Candidates: peaking at a few Qs, plus a shelf if the error sits at an edge.
        let mut candidates: Vec<FilterSpec> = Vec::new();
        for m in [0.6f32, 1.0, 1.7] {
            candidates.push(FilterSpec {
                kind: FilterKind::Peaking,
                freq: f0,
                gain_db: gain,
                q: (q0 * m).clamp(cfg.min_q, cfg.max_q),
                enabled: true,
            });
        }
        let has_ls = filters
            .iter()
            .any(|f| matches!(f.kind, FilterKind::LowShelf));
        let has_hs = filters
            .iter()
            .any(|f| matches!(f.kind, FilterKind::HighShelf));
        if cfg.use_low_shelf && !has_ls && f0 < cfg.f_min * 4.0 {
            for fc in [cfg.f_min * 2.0, cfg.f_min * 3.5, 120.0] {
                candidates.push(FilterSpec {
                    kind: FilterKind::LowShelf,
                    freq: fc,
                    gain_db: gain,
                    q: 0.7,
                    enabled: true,
                });
            }
        }
        if cfg.use_high_shelf && !has_hs && f0 > cfg.f_max * 0.25 {
            for fc in [cfg.f_max * 0.25, cfg.f_max * 0.5, 4000.0] {
                candidates.push(FilterSpec {
                    kind: FilterKind::HighShelf,
                    freq: fc,
                    gain_db: gain,
                    q: 0.7,
                    enabled: true,
                });
            }
        }

        let mut best_spec: Option<FilterSpec> = None;
        let mut best_rms = weighted_rms(&err, &w);
        let mut best_resp: Vec<f32> = Vec::new();
        for c in candidates {
            let resp = response_on_grid(&c, grid, sr);
            let trial: Vec<f32> = err.iter().zip(resp.iter()).map(|(e, r)| e - r).collect();
            let r = weighted_rms(&trial, &w);
            if r < best_rms - 1e-4 {
                best_rms = r;
                best_spec = Some(c);
                best_resp = resp;
            }
        }
        match best_spec {
            Some(s) => {
                for (e, r) in err.iter_mut().zip(best_resp.iter()) {
                    *e -= r;
                }
                filters.push(s);
            }
            None => break, // nothing improved it any further
        }
    }

    // ----- Refinement stage -----
    for _pass in 0..cfg.refine_passes {
        let mut improved = false;
        for idx in 0..filters.len() {
            // Take filter idx back out of the error, try variants, keep the best.
            let cur = filters[idx].clone();
            let cur_resp = response_on_grid(&cur, grid, sr);
            let base: Vec<f32> = err
                .iter()
                .zip(cur_resp.iter())
                .map(|(e, r)| e + r)
                .collect();

            let mut best = cur.clone();
            let mut best_rms = weighted_rms(&err, &w);
            let mut best_resp = cur_resp;

            let f_steps = [1.0f32 / 1.06, 1.0, 1.06];
            let g_steps = [-0.7f32, -0.25, 0.0, 0.25, 0.7];
            let q_steps = [1.0f32 / 1.25, 1.0, 1.25];
            for fs in f_steps {
                for gs in g_steps {
                    for qs in q_steps {
                        if (fs - 1.0).abs() < 1e-6 && gs == 0.0 && (qs - 1.0).abs() < 1e-6 {
                            continue;
                        }
                        let cand = FilterSpec {
                            kind: cur.kind,
                            freq: (cur.freq * fs).clamp(10.0, sr * 0.45),
                            gain_db: (cur.gain_db + gs)
                                .clamp(-cfg.max_cut_db, cfg.max_boost_db),
                            q: (cur.q * qs).clamp(cfg.min_q, cfg.max_q),
                            enabled: true,
                        };
                        let resp = response_on_grid(&cand, grid, sr);
                        let trial: Vec<f32> =
                            base.iter().zip(resp.iter()).map(|(e, r)| e - r).collect();
                        let r = weighted_rms(&trial, &w);
                        if r < best_rms - 1e-4 {
                            best_rms = r;
                            best = cand;
                            best_resp = resp;
                        }
                    }
                }
            }
            if best.freq != cur.freq || best.gain_db != cur.gain_db || best.q != cur.q {
                improved = true;
            }
            err = base
                .iter()
                .zip(best_resp.iter())
                .map(|(e, r)| e - r)
                .collect();
            filters[idx] = best;
        }
        if !improved {
            break;
        }
    }

    // Drop filters whose gain is too small to be worth a band.
    filters.retain(|f| f.gain_db.abs() >= 0.2);
    filters.sort_by(|a, b| a.freq.partial_cmp(&b.freq).unwrap_or(std::cmp::Ordering::Equal));

    let eq_response = crate::dsp::biquad::total_response_db(&filters, grid, sr);
    let error_after: Vec<f32> = error_before
        .iter()
        .zip(eq_response.iter())
        .map(|(e, r)| e - r)
        .collect();
    let rms_after = weighted_rms(&error_after, &w);

    let peak_gain = eq_response.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let preamp_db = -(peak_gain.max(0.0)) - cfg.preamp_headroom_db;

    FitResult {
        filters,
        preamp_db,
        grid: grid.to_vec(),
        error_before,
        error_after,
        eq_response,
        rms_before,
        rms_after,
    }
}

/// Suggest the lowest frequency worth correcting: where the response starts falling
/// more than drop_db below the midband average. Below that, EQ only burns power.
pub fn suggest_low_limit(grid: &[f32], measured: &[f32], drop_db: f32) -> f32 {
    // Reference level: the average from 200 Hz to 2 kHz.
    let mut sum = 0.0f64;
    let mut cnt = 0usize;
    for (f, m) in grid.iter().zip(measured.iter()) {
        if *f >= 200.0 && *f <= 2000.0 {
            sum += *m as f64;
            cnt += 1;
        }
    }
    if cnt == 0 {
        return grid.first().copied().unwrap_or(20.0);
    }
    let reference = (sum / cnt as f64) as f32;
    // Scan down from 200 Hz and stop once it falls past the threshold.
    let mut limit = grid.first().copied().unwrap_or(20.0);
    for i in (0..grid.len()).rev() {
        if grid[i] > 200.0 {
            continue;
        }
        if measured[i] < reference - drop_db {
            limit = grid[i];
            break;
        }
    }
    limit.clamp(15.0, 200.0)
}

/// Work out the preamp needed for any filter set, including hand-edited ones.
pub fn required_preamp_db(filters: &[FilterSpec], sr: f32, headroom_db: f32) -> f32 {
    let grid = crate::dsp::octave::log_grid(10.0, (sr * 0.49).min(24_000.0), 24);
    let resp = crate::dsp::biquad::total_response_db(filters, &grid, sr);
    let peak = resp.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    -(peak.max(0.0)) - headroom_db
}
