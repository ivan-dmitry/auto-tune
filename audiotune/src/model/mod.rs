//! Data types for a measurement and for a whole project (saveable and reloadable).

use crate::dsp::biquad::FilterSpec;
use crate::dsp::calibration::{MicCalibration, SplCalibration};
use crate::dsp::device_eq::DeviceSpec;
use crate::dsp::generator::SweepConfig;
use crate::dsp::measurement::{DecayResult, DistortionResult, FreqResponse, ImpulseResponse};
use crate::dsp::target::TargetCurve;

/// The kind of system being tuned. Drives the defaults and the guided steps.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TuneMode {
    OneSpeaker,
    Stereo,
    Headphone,
}

impl TuneMode {
    pub const ALL: [TuneMode; 3] = [TuneMode::OneSpeaker, TuneMode::Stereo, TuneMode::Headphone];

    pub fn label(&self) -> &'static str {
        match self {
            TuneMode::OneSpeaker => "One speaker",
            TuneMode::Stereo => "Two speakers (stereo)",
            TuneMode::Headphone => "Headphones",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            TuneMode::OneSpeaker => {
                "Mic at the listening position, pointed straight at the speaker. One channel."
            }
            TuneMode::Stereo => {
                "Measures left then right separately, then reports level and timing mismatch."
            }
            TuneMode::Headphone => {
                "Needs a coupler or an in-ear measurement mic. A mic held outside is not usable."
            }
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Measurement {
    pub name: String,
    pub timestamp: String,
    pub channel: String,
    pub sr: f32,
    pub sweep: SweepConfig,
    pub ir: ImpulseResponse,
    pub fr: FreqResponse,
    pub decay: Option<DecayResult>,
    pub distortion: DistortionResult,
    pub snr_db: f32,
    pub peak_dbfs: f32,
    pub clipped: bool,
    pub visible: bool,
    pub color_idx: usize,
}

impl Measurement {
    /// Midband average, used to line measurements up against each other.
    pub fn midband_db(&self) -> f32 {
        self.fr.band_average_db(200.0, 2000.0)
    }
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Project {
    pub version: u32,
    pub mode: Option<TuneMode>,
    pub measurements: Vec<Measurement>,
    pub filters: Vec<FilterSpec>,
    pub preamp_db: f32,
    pub target: Option<TargetCurve>,
    pub mic_cal: Option<MicCalibration>,
    pub spl_cal: Option<SplCalibration>,
    pub device: Option<DeviceSpec>,
    pub note: String,
}

impl Project {
    pub const VERSION: u32 = 1;

    pub fn save(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// Colour palette for overlaid measurement traces.
pub const TRACE_COLORS: [(u8, u8, u8); 8] = [
    (0x4F, 0xC3, 0xF7),
    (0xFF, 0xB3, 0x4D),
    (0x81, 0xC7, 0x84),
    (0xE5, 0x73, 0x73),
    (0xBA, 0x68, 0xC8),
    (0xFF, 0xF1, 0x76),
    (0x4D, 0xB6, 0xAC),
    (0xF0, 0x62, 0x92),
];

pub fn trace_color(i: usize) -> egui::Color32 {
    let (r, g, b) = TRACE_COLORS[i % TRACE_COLORS.len()];
    egui::Color32::from_rgb(r, g, b)
}

/// A short time stamp for measurement names, without pulling in a date library.
pub fn timestamp_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let t = secs % 86_400;
    let days = secs / 86_400;
    format!("{:02}:{:02}:{:02} (d{})", t / 3600, (t % 3600) / 60, t % 60, days)
}
