//! Translating a computed correction into the actual knobs of an EXTERNAL hardware EQ.
//!
//! The app does not have to be the thing applying EQ. Far more often the correction
//! has to be dialled into a separate box (a 31-band graphic EQ, a karaoke/DSP
//! processor, a rack parametric). Two things make that non-trivial:
//!
//!   * Graphic EQ bands overlap heavily, so setting each slider to the error
//!     measured at its own centre frequency overshoots badly. This module solves
//!     for the slider set whose *combined* response matches the desired curve.
//!   * Real hardware quantises: a fixed gain step (0.5 dB), a hard limit (±12 dB),
//!     sometimes a fixed list of allowed Q values. Numbers must be snapped to what
//!     the box can actually be set to, and the prediction must use the snapped
//!     values — otherwise the plot lies.

use crate::dsp::biquad::{self, bw_to_q, q_to_bw, FilterKind, FilterSpec};

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum DeviceKind {
    /// Fixed centre frequencies, one gain control each.
    Graphic,
    /// Free frequency / gain / Q per band.
    Parametric,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 2] = [DeviceKind::Graphic, DeviceKind::Parametric];

    pub fn label(&self) -> &'static str {
        match self {
            DeviceKind::Graphic => "Graphic EQ (fixed bands)",
            DeviceKind::Parametric => "Parametric EQ (Fc / gain / Q)",
        }
    }
}

/// The band grid a graphic EQ uses.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum BandLayout {
    /// 31 bands, 20 Hz … 20 kHz.
    ThirdOctave,
    /// 15 bands, 25 Hz … 16 kHz.
    TwoThirdOctave,
    /// 10 bands, one per octave, 31.5 Hz … 16 kHz.
    FullOctave,
    /// The 10-band grid most media players and soft EQs use, where the two end
    /// bands are labelled "<31" and ">16K" because they are shelves.
    PlayerTenBand,
    /// Whatever the unit actually has — set the count and range yourself.
    Custom,
}

impl BandLayout {
    pub const ALL: [BandLayout; 5] = [
        BandLayout::ThirdOctave,
        BandLayout::TwoThirdOctave,
        BandLayout::FullOctave,
        BandLayout::PlayerTenBand,
        BandLayout::Custom,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            BandLayout::ThirdOctave => "31 bands (1/3 octave)",
            BandLayout::TwoThirdOctave => "15 bands (2/3 octave)",
            BandLayout::FullOctave => "10 bands (1 octave)",
            BandLayout::PlayerTenBand => "10 bands, player style (<31 … >16K)",
            BandLayout::Custom => "Custom",
        }
    }

    /// The standard centre frequencies printed on the hardware.
    /// `Custom` has none of its own; the device spec supplies them.
    pub fn centers(&self) -> Vec<f32> {
        match self {
            BandLayout::ThirdOctave => vec![
                20.0, 25.0, 31.5, 40.0, 50.0, 63.0, 80.0, 100.0, 125.0, 160.0, 200.0, 250.0,
                315.0, 400.0, 500.0, 630.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0,
                3150.0, 4000.0, 5000.0, 6300.0, 8000.0, 10000.0, 12500.0, 16000.0, 20000.0,
            ],
            BandLayout::TwoThirdOctave => vec![
                25.0, 40.0, 63.0, 100.0, 160.0, 250.0, 400.0, 630.0, 1000.0, 1600.0, 2500.0,
                4000.0, 6300.0, 10000.0, 16000.0,
            ],
            BandLayout::FullOctave => vec![
                31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
            ],
            BandLayout::PlayerTenBand => vec![
                31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
            ],
            BandLayout::Custom => Vec::new(),
        }
    }

    /// Whether the two outermost bands of this layout behave as shelves.
    pub fn default_shelf_ends(&self) -> bool {
        matches!(self, BandLayout::PlayerTenBand)
    }

}

/// Generate `count` log-spaced centre frequencies between two limits.
pub fn generate_centers(count: usize, f_lo: f32, f_hi: f32) -> Vec<f32> {
    let count = count.clamp(2, MAX_BANDS);
    let f_lo = f_lo.max(5.0);
    let f_hi = f_hi.max(f_lo * 2.0);
    let span = (f_hi / f_lo).log2();
    (0..count)
        .map(|i| {
            let f = f_lo * 2f32.powf(span * i as f32 / (count - 1) as f32);
            // Round to something a front panel would actually print.
            if f < 100.0 {
                (f * 2.0).round() / 2.0
            } else if f < 1000.0 {
                (f / 5.0).round() * 5.0
            } else {
                (f / 50.0).round() * 50.0
            }
        })
        .collect()
}

/// Upper limit on how many bands a unit may have.
pub const MAX_BANDS: usize = 32;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceSpec {
    pub name: String,
    pub kind: DeviceKind,
    pub layout: BandLayout,
    /// Centre frequencies when `layout` is `Custom`.
    #[serde(default)]
    pub custom_centers: Vec<f32>,
    /// Treat the lowest and highest bands as shelves rather than peaking filters.
    /// This is what a control labelled "<31" or ">16K" actually does.
    #[serde(default)]
    pub shelf_ends: bool,
    /// Number of bands available on a parametric device.
    pub parametric_bands: usize,
    pub max_boost_db: f32,
    pub max_cut_db: f32,
    /// Smallest gain increment the device can be set to.
    pub step_db: f32,
    /// Smallest frequency increment, as a fraction of an octave (0 = free).
    pub freq_step_octave: f32,
    /// Report bandwidth in octaves instead of Q (many rack units are labelled that way).
    pub use_bandwidth: bool,
    /// Bandwidth step in octaves, if the device quantises it (0 = free).
    pub bw_step_octave: f32,
}

impl Default for DeviceSpec {
    fn default() -> Self {
        Self {
            name: "DSP1462".into(),
            kind: DeviceKind::Graphic,
            layout: BandLayout::ThirdOctave,
            custom_centers: generate_centers(10, 31.0, 16_000.0),
            shelf_ends: false,
            parametric_bands: 10,
            max_boost_db: 12.0,
            max_cut_db: 12.0,
            step_db: 0.5,
            freq_step_octave: 0.0,
            use_bandwidth: true,
            bw_step_octave: 0.0,
        }
    }
}

impl DeviceSpec {
    /// A 31-band rack graphic, the usual +/-12 dB in 0.5 dB steps.
    pub fn preset_rack_31() -> Self {
        Self {
            name: "31-band graphic".into(),
            kind: DeviceKind::Graphic,
            layout: BandLayout::ThirdOctave,
            shelf_ends: false,
            max_boost_db: 12.0,
            max_cut_db: 12.0,
            step_db: 0.5,
            ..Default::default()
        }
    }

    /// The 10-band equaliser built into media players and soft DSPs: the end bands
    /// are shelves and the range is usually +/-20 dB in 0.1 dB steps.
    pub fn preset_player_10() -> Self {
        Self {
            name: "10-band equalizer".into(),
            kind: DeviceKind::Graphic,
            layout: BandLayout::PlayerTenBand,
            shelf_ends: true,
            max_boost_db: 20.0,
            max_cut_db: 20.0,
            step_db: 0.1,
            ..Default::default()
        }
    }

    /// The centre frequencies this unit actually has.
    pub fn centers(&self) -> Vec<f32> {
        match self.layout {
            BandLayout::Custom => {
                let mut c = self.custom_centers.clone();
                c.retain(|f| f.is_finite() && *f > 0.0);
                c.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                c.truncate(MAX_BANDS);
                c
            }
            other => other.centers(),
        }
    }

    /// Spacing between neighbouring bands, in octaves. This is what sets the Q of
    /// each band filter, because a graphic EQ band is as wide as its spacing.
    pub fn band_width_octaves(&self) -> f32 {
        let c = self.centers();
        if c.len() < 2 {
            return 1.0;
        }
        let mut ratios: Vec<f32> = c.windows(2).map(|w| (w[1] / w[0]).log2()).collect();
        ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ratios[ratios.len() / 2].clamp(0.05, 3.0)
    }

    /// The filter a given band behaves as.
    pub fn band_kind(&self, i: usize, n: usize) -> FilterKind {
        if self.shelf_ends && n >= 3 {
            if i == 0 {
                return FilterKind::LowShelf;
            }
            if i == n - 1 {
                return FilterKind::HighShelf;
            }
        }
        FilterKind::Peaking
    }

    fn band_spec(&self, i: usize, n: usize, fc: f32, gain_db: f32) -> FilterSpec {
        let kind = self.band_kind(i, n);
        let q = if matches!(kind, FilterKind::Peaking) {
            bw_to_q(self.band_width_octaves())
        } else {
            0.7
        };
        FilterSpec {
            kind,
            freq: fc,
            gain_db,
            q,
            enabled: true,
        }
    }

    /// How the front panel labels each band.
    pub fn band_label(&self, i: usize, n: usize, fc: f32) -> String {
        if self.shelf_ends && n >= 3 && i == 0 {
            format!("<{}", fmt_hz(fc))
        } else if self.shelf_ends && n >= 3 && i == n - 1 {
            format!(">{}", fmt_hz(fc))
        } else {
            fmt_hz(fc)
        }
    }
}

fn snap(value: f32, step: f32) -> f32 {
    if step <= 0.0 {
        value
    } else {
        (value / step).round() * step
    }
}

impl DeviceSpec {
    fn clamp_gain(&self, g: f32) -> f32 {
        snap(g, self.step_db).clamp(-self.max_cut_db, self.max_boost_db)
    }

    /// Snap a frequency to the device's frequency grid (log-spaced).
    fn snap_freq(&self, f: f32) -> f32 {
        if self.freq_step_octave <= 0.0 {
            // Most units accept 1 Hz resolution below 1 kHz; keep it readable.
            if f < 100.0 {
                (f * 10.0).round() / 10.0
            } else if f < 1000.0 {
                f.round()
            } else {
                (f / 10.0).round() * 10.0
            }
        } else {
            let n = ((f / 1000.0).log2() / self.freq_step_octave).round();
            1000.0 * 2f32.powf(n * self.freq_step_octave)
        }
    }
}

/// One line the user has to set on the hardware.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BandSetting {
    /// 1-based band number as printed on the device.
    pub index: usize,
    pub freq_hz: f32,
    pub gain_db: f32,
    /// Q, only meaningful for parametric devices.
    pub q: Option<f32>,
    /// Bandwidth in octaves, only meaningful for parametric devices.
    pub bandwidth_oct: Option<f32>,
    /// True when the wanted gain was cut off by the device's limit.
    pub limited: bool,
    /// What this band behaves as. End bands of a player-style EQ are shelves.
    #[serde(default)]
    pub kind: FilterKind,
    /// How the front panel labels it, e.g. "125" or "<31".
    #[serde(default)]
    pub label: String,
    /// The Q actually used when predicting this band's effect.
    #[serde(default)]
    pub effective_q: f32,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DeviceResult {
    pub settings: Vec<BandSetting>,
    /// Response the device will actually produce, on the analysis grid.
    pub achieved: Vec<f32>,
    /// Correction that was asked for, on the same grid.
    pub desired: Vec<f32>,
    pub grid: Vec<f32>,
    pub rms_error_before: f32,
    pub rms_error_after: f32,
    /// Suggested overall level trim so the loudest boost does not clip the device.
    pub output_trim_db: f32,
}

impl DeviceResult {
    pub fn active(&self) -> impl Iterator<Item = &BandSetting> {
        self.settings.iter().filter(|s| s.gain_db.abs() >= 1e-3)
    }

    pub fn any_limited(&self) -> bool {
        self.settings.iter().any(|s| s.limited)
    }

    /// The same settings expressed as biquads, so the app can preview exactly what
    /// the hardware will do, quantisation and shelf ends included.
    pub fn to_filter_specs(&self, _spec: &DeviceSpec) -> Vec<FilterSpec> {
        self.settings
            .iter()
            .filter(|s| s.gain_db.abs() >= 1e-3)
            .map(|s| FilterSpec {
                kind: s.kind,
                freq: s.freq_hz,
                gain_db: s.gain_db,
                q: if s.effective_q > 0.0 {
                    s.effective_q
                } else {
                    s.q.unwrap_or(1.0)
                },
                enabled: true,
            })
            .collect()
    }
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

/// Weight of 1 inside the working range, fading to 0 over half an octave outside it.
fn range_weights(grid: &[f32], f_min: f32, f_max: f32) -> Vec<f32> {
    const EDGE: f32 = 0.5;
    grid.iter()
        .map(|&f| {
            let lo = ((f / f_min).log2() / EDGE).clamp(0.0, 1.0);
            let hi = ((f_max / f).log2() / EDGE).clamp(0.0, 1.0);
            let t = lo.min(hi);
            0.5 * (1.0 - (std::f32::consts::PI * t).cos())
        })
        .collect()
}

/// Solve for the slider positions of a graphic EQ.
///
/// Each band is modelled as a constant-Q peaking filter whose bandwidth equals the
/// band spacing, which is how digital graphic EQs are built. Because neighbouring
/// bands overlap, the gains are solved together by repeated relaxation rather than
/// read off the error curve one band at a time.
pub fn fit_graphic(
    grid: &[f32],
    desired: &[f32],
    spec: &DeviceSpec,
    sr: f32,
    f_min: f32,
    f_max: f32,
) -> DeviceResult {
    let centers = spec.centers();
    let n = grid.len();
    if n == 0 || desired.len() != n || centers.is_empty() {
        return DeviceResult::default();
    }
    let nb = centers.len();
    let weights = range_weights(grid, f_min, f_max);

    // Unit response of every band, precomputed once (+1 dB each). End bands may be
    // shelves rather than peaks, which changes their shape completely.
    let unit: Vec<Vec<f32>> = centers
        .iter()
        .enumerate()
        .map(|(i, &fc)| {
            let c = biquad::Coeffs::design(&spec.band_spec(i, nb, fc, 1.0), sr);
            grid.iter().map(|&f| c.response_db(f, sr)).collect()
        })
        .collect();
    // Energy of each band's shape, for projecting the residual onto it.
    let norms: Vec<f32> = unit
        .iter()
        .map(|u| {
            u.iter()
                .zip(weights.iter())
                .map(|(v, w)| v * v * w)
                .sum::<f32>()
                .max(1e-9)
        })
        .collect();

    let as_specs = |gains: &[f32]| -> Vec<FilterSpec> {
        centers
            .iter()
            .enumerate()
            .zip(gains.iter())
            .filter(|(_, g)| g.abs() > 1e-4)
            .map(|((i, &fc), &g)| spec.band_spec(i, nb, fc, g))
            .collect()
    };

    let mut gains = vec![0.0f32; nb];
    // Adding band curves in dB is only an approximation, because a peaking filter's dB
    // shape does not scale exactly with its gain. So solve in that cheap linear model,
    // then measure how far the real cascade differs and fold that error back into the
    // request. Three or four rounds of this converge to the true optimum.
    let mut request = desired.to_vec();

    for outer in 0..4 {
        let mut current = vec![0.0f32; n];
        for (b, g) in gains.iter().enumerate() {
            if g.abs() > 1e-6 {
                for i in 0..n {
                    current[i] += g * unit[b][i];
                }
            }
        }

        // Gauss-Seidel relaxation: each band is nudged towards the part of the residual
        // its own shape can explain, and the residual is updated as we go.
        for _pass in 0..80 {
            let mut moved = 0.0f32;
            for b in 0..centers.len() {
                let mut num = 0.0f32;
                for i in 0..n {
                    num += (request[i] - current[i]) * unit[b][i] * weights[i];
                }
                let delta = 0.8 * num / norms[b];
                let before = gains[b];
                gains[b] = (gains[b] + delta).clamp(-spec.max_cut_db, spec.max_boost_db);
                let applied = gains[b] - before;
                if applied.abs() > 1e-6 {
                    for i in 0..n {
                        current[i] += applied * unit[b][i];
                    }
                    moved += applied.abs();
                }
            }
            if moved < 1e-3 {
                break;
            }
        }

        if outer == 3 {
            break;
        }
        // Fold the model error back in: if the linear model reads high compared with the
        // real cascade, ask the model for correspondingly more.
        let truth = biquad::total_response_db(&as_specs(&gains), grid, sr);
        let mut worst = 0.0f32;
        for i in 0..n {
            let model_err = current[i] - truth[i];
            request[i] = desired[i] + model_err;
            worst = worst.max(model_err.abs());
        }
        if worst < 0.02 {
            break;
        }
    }

    // Now quantise to what the hardware can be set to, and recompute honestly.
    let mut settings = Vec::with_capacity(centers.len());
    let mut snapped = Vec::with_capacity(centers.len());
    for (b, &fc) in centers.iter().enumerate() {
        let wanted = gains[b];
        let g = spec.clamp_gain(wanted);
        let limited = (wanted.abs() - spec.max_boost_db.max(spec.max_cut_db)).abs() < 1e-3
            || wanted > spec.max_boost_db - 1e-3
            || wanted < -spec.max_cut_db + 1e-3;
        snapped.push(g);
        let model = spec.band_spec(b, nb, fc, g);
        settings.push(BandSetting {
            index: b + 1,
            freq_hz: fc,
            gain_db: g,
            q: None,
            bandwidth_oct: None,
            limited,
            kind: model.kind,
            label: spec.band_label(b, nb, fc),
            effective_q: model.q,
        });
    }

    // What the hardware will really do, computed from the quantised values as a real
    // filter cascade — not as a linear sum of band shapes.
    let achieved = biquad::total_response_db(&as_specs(&snapped), grid, sr);

    let before: Vec<f32> = desired.to_vec();
    let after: Vec<f32> = desired
        .iter()
        .zip(achieved.iter())
        .map(|(d, a)| d - a)
        .collect();
    let peak = achieved.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    DeviceResult {
        settings,
        achieved,
        desired: desired.to_vec(),
        grid: grid.to_vec(),
        rms_error_before: weighted_rms(&before, &weights),
        rms_error_after: weighted_rms(&after, &weights),
        output_trim_db: -(peak.max(0.0)) - 0.5,
    }
}

/// Turn a set of fitted parametric filters into device settings.
///
/// Keeps the strongest bands the device has room for, then snaps frequency, gain and
/// Q to the device's own resolution and reports what that snapped set will do.
pub fn fit_parametric(
    grid: &[f32],
    desired: &[f32],
    filters: &[FilterSpec],
    spec: &DeviceSpec,
    sr: f32,
    f_min: f32,
    f_max: f32,
) -> DeviceResult {
    let weights = range_weights(grid, f_min, f_max);

    // Strongest corrections first — those are the ones worth the limited band count.
    let mut chosen: Vec<FilterSpec> = filters.iter().filter(|f| f.enabled).cloned().collect();
    chosen.sort_by(|a, b| {
        b.gain_db
            .abs()
            .partial_cmp(&a.gain_db.abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chosen.truncate(spec.parametric_bands);
    chosen.sort_by(|a, b| {
        a.freq
            .partial_cmp(&b.freq)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut settings = Vec::new();
    let mut snapped_specs = Vec::new();
    for (i, f) in chosen.iter().enumerate() {
        let wanted = f.gain_db;
        let gain = spec.clamp_gain(wanted);
        let freq = spec.snap_freq(f.freq);
        let bw_raw = q_to_bw(f.q);
        let bw = if spec.bw_step_octave > 0.0 {
            snap(bw_raw, spec.bw_step_octave).max(spec.bw_step_octave)
        } else {
            (bw_raw * 100.0).round() / 100.0
        };
        let q = bw_to_q(bw);
        settings.push(BandSetting {
            index: i + 1,
            freq_hz: freq,
            gain_db: gain,
            q: Some((q * 100.0).round() / 100.0),
            bandwidth_oct: Some(bw),
            limited: wanted > spec.max_boost_db - 1e-3 || wanted < -spec.max_cut_db + 1e-3,
            kind: f.kind,
            label: fmt_hz(freq),
            effective_q: q,
        });
        snapped_specs.push(FilterSpec {
            kind: f.kind,
            freq,
            gain_db: gain,
            q,
            enabled: true,
        });
    }

    let achieved = biquad::total_response_db(&snapped_specs, grid, sr);
    let after: Vec<f32> = desired
        .iter()
        .zip(achieved.iter())
        .map(|(d, a)| d - a)
        .collect();
    let peak = achieved.iter().cloned().fold(f32::NEG_INFINITY, f32::max);

    DeviceResult {
        settings,
        achieved,
        desired: desired.to_vec(),
        grid: grid.to_vec(),
        rms_error_before: weighted_rms(desired, &weights),
        rms_error_after: weighted_rms(&after, &weights),
        output_trim_db: -(peak.max(0.0)) - 0.5,
    }
}

/// Human-readable settings sheet to print or keep next to the rack.
pub fn settings_sheet(res: &DeviceResult, spec: &DeviceSpec, header: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!("{header}\n"));
    s.push_str(&format!("Device: {} · {}\n", spec.name, spec.kind.label()));
    match spec.kind {
        DeviceKind::Graphic => {
            s.push_str(&format!("Layout: {}\n", spec.layout.label()));
        }
        DeviceKind::Parametric => {
            s.push_str(&format!("Bands used: {}\n", res.settings.len()));
        }
    }
    s.push_str(&format!(
        "Limits: +{:.0} / -{:.0} dB in {:.1} dB steps\n",
        spec.max_boost_db, spec.max_cut_db, spec.step_db
    ));
    s.push_str(&format!(
        "Residual error: {:.2} dB RMS before, {:.2} dB after\n",
        res.rms_error_before, res.rms_error_after
    ));
    s.push_str(&format!(
        "Suggested output trim: {:.1} dB\n\n",
        res.output_trim_db
    ));

    match spec.kind {
        DeviceKind::Graphic => {
            s.push_str("Band   Control     Set to\n");
            s.push_str("-----------------------------\n");
            let dec = if spec.step_db < 0.5 { 1 } else { 1 };
            for b in &res.settings {
                let flag = if b.limited { "  (at limit)" } else { "" };
                let name = if b.label.is_empty() {
                    fmt_hz(b.freq_hz)
                } else {
                    b.label.clone()
                };
                s.push_str(&format!(
                    "{:>4}   {:>7}   {:>+6.*} dB{}\n",
                    b.index, name, dec, b.gain_db, flag
                ));
            }
        }
        DeviceKind::Parametric => {
            s.push_str("Band   Freq        Gain       Q      BW(oct)\n");
            s.push_str("---------------------------------------------\n");
            for b in &res.settings {
                let flag = if b.limited { "  (at limit)" } else { "" };
                s.push_str(&format!(
                    "{:>4}   {:>7}   {:>+6.1} dB   {:>5.2}   {:>5.2}{}\n",
                    b.index,
                    fmt_hz(b.freq_hz),
                    b.gain_db,
                    b.q.unwrap_or(1.0),
                    b.bandwidth_oct.unwrap_or(1.0),
                    flag
                ));
            }
        }
    }
    s
}

pub fn fmt_hz(f: f32) -> String {
    if f >= 10_000.0 {
        format!("{:.0}k", f / 1000.0)
    } else if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.01 {
            format!("{k:.0}k")
        } else {
            format!("{k:.2}k")
        }
    } else if f >= 100.0 {
        format!("{f:.0}")
    } else {
        format!("{f:.1}")
    }
}

/// CSV for a spreadsheet or for typing into a controller app.
pub fn settings_csv(res: &DeviceResult, spec: &DeviceSpec) -> String {
    let mut s = String::new();
    match spec.kind {
        DeviceKind::Graphic => {
            s.push_str("band,freq_hz,gain_db\n");
            for b in &res.settings {
                s.push_str(&format!("{},{:.1},{:.1}\n", b.index, b.freq_hz, b.gain_db));
            }
        }
        DeviceKind::Parametric => {
            s.push_str("band,freq_hz,gain_db,q,bandwidth_oct\n");
            for b in &res.settings {
                s.push_str(&format!(
                    "{},{:.1},{:.1},{:.3},{:.3}\n",
                    b.index,
                    b.freq_hz,
                    b.gain_db,
                    b.q.unwrap_or(0.0),
                    b.bandwidth_oct.unwrap_or(0.0)
                ));
            }
        }
    }
    s
}
