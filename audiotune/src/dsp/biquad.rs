//! Designing and running biquad filters, using the RBJ Audio EQ Cookbook formulas.
//!
//! Used for two things: drawing the EQ response, and filtering audio for A/B listening.

use std::f32::consts::PI;

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum FilterKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    Notch,
    BandPass,
    AllPass,
}

impl FilterKind {
    pub const ALL: [FilterKind; 8] = [
        FilterKind::Peaking,
        FilterKind::LowShelf,
        FilterKind::HighShelf,
        FilterKind::LowPass,
        FilterKind::HighPass,
        FilterKind::Notch,
        FilterKind::BandPass,
        FilterKind::AllPass,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            FilterKind::Peaking => "Peaking (PK)",
            FilterKind::LowShelf => "Low Shelf (LS)",
            FilterKind::HighShelf => "High Shelf (HS)",
            FilterKind::LowPass => "Low Pass (LP)",
            FilterKind::HighPass => "High Pass (HP)",
            FilterKind::Notch => "Notch",
            FilterKind::BandPass => "Band Pass",
            FilterKind::AllPass => "All Pass",
        }
    }

    /// Short name used when exporting Equalizer APO / REW style files.
    pub fn short(&self) -> &'static str {
        match self {
            FilterKind::Peaking => "PK",
            FilterKind::LowShelf => "LSC",
            FilterKind::HighShelf => "HSC",
            FilterKind::LowPass => "LP",
            FilterKind::HighPass => "HP",
            FilterKind::Notch => "NO",
            FilterKind::BandPass => "BP",
            FilterKind::AllPass => "AP",
        }
    }

    pub fn uses_gain(&self) -> bool {
        matches!(
            self,
            FilterKind::Peaking | FilterKind::LowShelf | FilterKind::HighShelf
        )
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FilterSpec {
    pub kind: FilterKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
    pub enabled: bool,
}

impl Default for FilterSpec {
    fn default() -> Self {
        Self {
            kind: FilterKind::Peaking,
            freq: 1000.0,
            gain_db: 0.0,
            q: 1.0,
            enabled: true,
        }
    }
}

impl FilterSpec {
    pub fn peaking(freq: f32, gain_db: f32, q: f32) -> Self {
        Self {
            kind: FilterKind::Peaking,
            freq,
            gain_db,
            q,
            enabled: true,
        }
    }
}

/// Biquad coefficients, already normalised so that a0 = 1.
#[derive(Copy, Clone, Debug)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Default for Coeffs {
    fn default() -> Self {
        Self::identity()
    }
}

impl Coeffs {
    pub fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    pub fn design(spec: &FilterSpec, sr: f32) -> Self {
        if !spec.enabled || sr <= 0.0 {
            return Self::identity();
        }
        // Keep the frequency below Nyquist so the coefficients stay meaningful.
        let f = spec.freq.clamp(1.0, sr * 0.49);
        let q = spec.q.clamp(0.05, 50.0);
        let a = 10f32.powf(spec.gain_db / 40.0);
        let w0 = 2.0 * PI * f / sr;
        let cw = w0.cos();
        let sw = w0.sin();
        let alpha = sw / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match spec.kind {
            FilterKind::Peaking => (
                1.0 + alpha * a,
                -2.0 * cw,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cw,
                1.0 - alpha / a,
            ),
            FilterKind::LowShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cw + s),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cw),
                    a * ((a + 1.0) - (a - 1.0) * cw - s),
                    (a + 1.0) + (a - 1.0) * cw + s,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cw),
                    (a + 1.0) + (a - 1.0) * cw - s,
                )
            }
            FilterKind::HighShelf => {
                let s = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cw + s),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cw),
                    a * ((a + 1.0) + (a - 1.0) * cw - s),
                    (a + 1.0) - (a - 1.0) * cw + s,
                    2.0 * ((a - 1.0) - (a + 1.0) * cw),
                    (a + 1.0) - (a - 1.0) * cw - s,
                )
            }
            FilterKind::LowPass => (
                (1.0 - cw) * 0.5,
                1.0 - cw,
                (1.0 - cw) * 0.5,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cw) * 0.5,
                -(1.0 + cw),
                (1.0 + cw) * 0.5,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
            FilterKind::Notch => (1.0, -2.0 * cw, 1.0, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cw, 1.0 - alpha),
            FilterKind::AllPass => (
                1.0 - alpha,
                -2.0 * cw,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cw,
                1.0 - alpha,
            ),
        };

        if a0.abs() < 1e-20 {
            return Self::identity();
        }
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    /// Magnitude response in dB at frequency f.
    pub fn response_db(&self, f: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * f / sr;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        // Numerator: b0 + b1 e^-jw + b2 e^-2jw
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        let num = (nr * nr + ni * ni).sqrt();
        let den = (dr * dr + di * di).sqrt().max(1e-20);
        crate::dsp::lin_to_db(num / den)
    }

    /// Phase response in degrees.
    pub fn response_phase_deg(&self, f: f32, sr: f32) -> f32 {
        let w = 2.0 * PI * f / sr;
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let nr = self.b0 + self.b1 * c1 + self.b2 * c2;
        let ni = -(self.b1 * s1 + self.b2 * s2);
        let dr = 1.0 + self.a1 * c1 + self.a2 * c2;
        let di = -(self.a1 * s1 + self.a2 * s2);
        (ni.atan2(nr) - di.atan2(dr)).to_degrees()
    }
}

/// One biquad stage with state, in transposed direct form II.
#[derive(Copy, Clone, Debug, Default)]
pub struct Biquad {
    pub c: Coeffs,
    z1: f32,
    z2: f32,
}

impl Biquad {
    pub fn new(c: Coeffs) -> Self {
        Self { c, z1: 0.0, z2: 0.0 }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// A chain of biquads plus a preamp. Allocation-free, so it can run in the callback.
#[derive(Clone, Debug, Default)]
pub struct Bank {
    pub stages: Vec<Biquad>,
    pub preamp: f32,
}

impl Bank {
    pub fn from_specs(specs: &[FilterSpec], preamp_db: f32, sr: f32) -> Self {
        Self {
            stages: specs
                .iter()
                .filter(|s| s.enabled)
                .map(|s| Biquad::new(Coeffs::design(s, sr)))
                .collect(),
            preamp: 10f32.powf(preamp_db / 20.0),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x * self.preamp;
        for st in self.stages.iter_mut() {
            y = st.process(y);
        }
        y
    }

    pub fn reset(&mut self) {
        for st in self.stages.iter_mut() {
            st.reset();
        }
    }
}

/// Combined response in dB of a list of filters at the given frequencies.
pub fn total_response_db(specs: &[FilterSpec], freqs: &[f32], sr: f32) -> Vec<f32> {
    let coeffs: Vec<Coeffs> = specs
        .iter()
        .filter(|s| s.enabled)
        .map(|s| Coeffs::design(s, sr))
        .collect();
    freqs
        .iter()
        .map(|&f| coeffs.iter().map(|c| c.response_db(f, sr)).sum())
        .collect()
}

/// Convert a bandwidth in octaves to the Q of a peaking filter.
pub fn bw_to_q(bw_oct: f32) -> f32 {
    let b = bw_oct.max(0.01);
    let p = 2f32.powf(b);
    p.sqrt() / (p - 1.0)
}

/// Convert Q back to a bandwidth in octaves.
pub fn q_to_bw(q: f32) -> f32 {
    let q = q.max(0.01);
    let x = 1.0 + 1.0 / (2.0 * q * q);
    let y = (x * x - 1.0).max(0.0).sqrt();
    (x + y).log2()
}

impl Default for FilterKind {
    fn default() -> Self {
        FilterKind::Peaking
    }
}
