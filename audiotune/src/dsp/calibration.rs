//! Calibration: microphone .cal / .frd files, and SPL level calibration.

use crate::dsp::octave::interp_log;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct MicCalibration {
    pub name: String,
    pub freqs: Vec<f32>,
    /// The deviation of the mic from flat, in dB. This is SUBTRACTED from measurements.
    pub db: Vec<f32>,
    pub enabled: bool,
}

impl MicCalibration {
    pub fn is_empty(&self) -> bool {
        self.freqs.is_empty()
    }

    /// Read the "frequency dB phase" format shared by REW, miniDSP UMIK and Dayton.
    pub fn parse(name: &str, text: &str) -> Self {
        let mut freqs = Vec::new();
        let mut db = Vec::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty()
                || l.starts_with('*')
                || l.starts_with('#')
                || l.starts_with("//")
                || l.starts_with('"')
            {
                continue;
            }
            let mut it = l
                .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .filter(|s| !s.is_empty());
            let f = it.next().and_then(|s| s.parse::<f32>().ok());
            let d = it.next().and_then(|s| s.parse::<f32>().ok());
            if let (Some(f), Some(d)) = (f, d) {
                if f > 0.0 && f.is_finite() && d.is_finite() {
                    freqs.push(f);
                    db.push(d);
                }
            }
        }
        // Sort ascending by frequency.
        let mut idx: Vec<usize> = (0..freqs.len()).collect();
        idx.sort_by(|&a, &b| {
            freqs[a]
                .partial_cmp(&freqs[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let sf: Vec<f32> = idx.iter().map(|&i| freqs[i]).collect();
        let sd: Vec<f32> = idx.iter().map(|&i| db[i]).collect();
        Self {
            name: name.to_string(),
            enabled: !sf.is_empty(),
            freqs: sf,
            db: sd,
        }
    }

    /// How many dB to SUBTRACT from a measurement at frequency f.
    pub fn correction_db(&self, f: f32) -> f32 {
        if !self.enabled || self.freqs.is_empty() {
            0.0
        } else {
            interp_log(&self.freqs, &self.db, f)
        }
    }

    /// Apply the calibration to a curve that already carries its frequency axis.
    pub fn apply(&self, freqs: &[f32], db: &mut [f32]) {
        if !self.enabled || self.freqs.is_empty() {
            return;
        }
        for (f, d) in freqs.iter().zip(db.iter_mut()) {
            *d -= self.correction_db(*f);
        }
    }

    /// Some UMIK files start with a Sens Factor line; pull the value out if present.
    pub fn sens_factor_db(text: &str) -> Option<f32> {
        for line in text.lines().take(10) {
            let l = line.trim();
            let lower = l.to_ascii_lowercase();
            if lower.contains("sens") && lower.contains("factor") {
                let digits: String = l
                    .chars()
                    .skip_while(|c| *c != '=')
                    .skip(1)
                    .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
                    .collect();
                if let Ok(v) = digits.trim().parse::<f32>() {
                    return Some(v);
                }
            }
        }
        None
    }
}

/// Conversion from dBFS to dB SPL.
#[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SplCalibration {
    /// The dB SPL that a 0 dBFS input corresponds to.
    pub spl_at_full_scale: f32,
    pub enabled: bool,
}

impl Default for SplCalibration {
    fn default() -> Self {
        Self {
            spl_at_full_scale: 100.0,
            enabled: false,
        }
    }
}

impl SplCalibration {
    /// Calibrate: the app reads measured_dbfs while the SPL meter reads reference_spl.
    pub fn calibrate(&mut self, measured_dbfs: f32, reference_spl: f32) {
        self.spl_at_full_scale = reference_spl - measured_dbfs;
        self.enabled = true;
    }

    pub fn to_spl(&self, dbfs: f32) -> f32 {
        dbfs + self.spl_at_full_scale
    }

    pub fn from_spl(&self, spl: f32) -> f32 {
        spl - self.spl_at_full_scale
    }
}

/// A and C weighting per IEC 61672, for reporting weighted SPL.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Weighting {
    Z,
    A,
    C,
}

impl Weighting {
    pub fn label(&self) -> &'static str {
        match self {
            Weighting::Z => "Z (flat)",
            Weighting::A => "A",
            Weighting::C => "C",
        }
    }

    pub fn db_at(&self, f: f32) -> f32 {
        let f = f.max(1.0);
        let f2 = f * f;
        match self {
            Weighting::Z => 0.0,
            Weighting::A => {
                let num = 12194.0f32.powi(2) * f2 * f2;
                let d1 = f2 + 20.6f32.powi(2);
                let d2 = ((f2 + 107.7f32.powi(2)) * (f2 + 737.9f32.powi(2))).sqrt();
                let d3 = f2 + 12194.0f32.powi(2);
                let ra = num / (d1 * d2 * d3);
                20.0 * ra.max(1e-20).log10() + 2.0
            }
            Weighting::C => {
                let num = 12194.0f32.powi(2) * f2;
                let d1 = f2 + 20.6f32.powi(2);
                let d3 = f2 + 12194.0f32.powi(2);
                let rc = num / (d1 * d3);
                20.0 * rc.max(1e-20).log10() + 0.06
            }
        }
    }
}
