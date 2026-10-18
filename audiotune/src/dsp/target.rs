//! Target curves — the shape EQ is trying to pull the measured response onto.

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TargetKind {
    /// Dead flat. For a treated control room, or a close-mic measurement.
    Flat,
    /// A straight slope in dB/octave — the simplest "listening room" target.
    Tilt,
    /// Harman-style in-room: a bass shelf plus a gentle downward tilt. The default
    /// for loudspeakers in a normal room.
    HarmanRoom,
    /// Approximation of the Harman OE 2018 over-ear headphone target.
    HarmanHeadphone,
    /// Cinema X-curve: flat to 2 kHz then -3 dB/octave.
    XCurve,
    /// A curve loaded from a file or edited by hand.
    Custom,
}

impl TargetKind {
    pub const ALL: [TargetKind; 6] = [
        TargetKind::HarmanRoom,
        TargetKind::Flat,
        TargetKind::Tilt,
        TargetKind::HarmanHeadphone,
        TargetKind::XCurve,
        TargetKind::Custom,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            TargetKind::Flat => "Flat",
            TargetKind::Tilt => "Straight tilt",
            TargetKind::HarmanRoom => "Harman in-room (speakers)",
            TargetKind::HarmanHeadphone => "Harman OE (headphones, approx.)",
            TargetKind::XCurve => "X-curve (cinema)",
            TargetKind::Custom => "Custom",
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            TargetKind::Flat => {
                "No colouration at all. Use it close to the speaker, or in a well-treated room."
            }
            TargetKind::Tilt => "A straight line on the log axis, pinned to 0 dB at 1 kHz.",
            TargetKind::HarmanRoom => {
                "Bass lifted by a low shelf, treble easing down — sounds natural in an ordinary room."
            }
            TargetKind::HarmanHeadphone => {
                "Only valid with a coupler or an in-ear measurement mic. A mic held outside \
                 the cup cannot be corrected to this."
            }
            TargetKind::XCurve => "ISO 2969, for large cinema auditoriums.",
            TargetKind::Custom => {
                "Load from a file (freq dB per line) or drag the anchor points by hand."
            }
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TargetCurve {
    pub kind: TargetKind,
    /// Slope in dB/octave, used by Tilt and by the upper part of HarmanRoom.
    pub tilt_db_oct: f32,
    /// How much the low shelf lifts the bass, in dB.
    pub bass_gain_db: f32,
    /// Corner frequency of the bass shelf, in Hz.
    pub bass_corner_hz: f32,
    /// Roll the target off below this frequency so EQ does not fight a driver that
    /// simply cannot go that low. 0 disables it.
    pub sub_rolloff_hz: f32,
    /// (Hz, dB) anchor points for the Custom kind.
    pub points: Vec<(f32, f32)>,
}

impl Default for TargetCurve {
    fn default() -> Self {
        Self {
            kind: TargetKind::HarmanRoom,
            tilt_db_oct: -0.8,
            bass_gain_db: 4.0,
            bass_corner_hz: 105.0,
            sub_rolloff_hz: 0.0,
            points: vec![(20.0, 0.0), (1000.0, 0.0), (20000.0, 0.0)],
        }
    }
}

/// Approximation of the Harman Over-Ear 2018 target, normalised to 0 dB at 500 Hz.
const HARMAN_OE: &[(f32, f32)] = &[
    (20.0, 6.6),
    (30.0, 6.0),
    (40.0, 5.3),
    (50.0, 4.7),
    (60.0, 4.1),
    (80.0, 3.1),
    (100.0, 2.4),
    (150.0, 1.2),
    (200.0, 0.6),
    (300.0, 0.1),
    (400.0, 0.0),
    (500.0, 0.0),
    (700.0, 0.1),
    (1000.0, 0.4),
    (1500.0, 1.5),
    (2000.0, 3.4),
    (2500.0, 5.2),
    (3000.0, 6.5),
    (3500.0, 6.0),
    (4000.0, 4.5),
    (5000.0, 2.0),
    (6000.0, 0.5),
    (7000.0, -0.5),
    (8000.0, -1.5),
    (9000.0, -2.0),
    (10000.0, -2.5),
    (12000.0, -3.5),
    (15000.0, -5.0),
    (20000.0, -8.0),
];

fn table_lookup(table: &[(f32, f32)], f: f32) -> f32 {
    if table.is_empty() {
        return 0.0;
    }
    if f <= table[0].0 {
        return table[0].1;
    }
    let last = table.len() - 1;
    if f >= table[last].0 {
        return table[last].1;
    }
    for i in 0..last {
        let (f0, v0) = table[i];
        let (f1, v1) = table[i + 1];
        if f >= f0 && f <= f1 {
            let t = (f / f0).ln() / (f1 / f0).ln();
            return v0 + t * (v1 - v0);
        }
    }
    table[last].1
}

/// Low-shelf shape on the log axis: 0 dB right at the corner, easing up to
/// `gain_db` an octave and a half below it. The slope is zero at both ends, so it
/// joins the rest of the curve without a kink.
fn low_shelf_shape(f: f32, corner: f32, gain_db: f32) -> f32 {
    const WIDTH_OCT: f32 = 1.5;
    let t = ((corner / f.max(1e-3)).log2() / WIDTH_OCT).clamp(0.0, 1.0);
    gain_db * 0.5 * (1.0 - (std::f32::consts::PI * t).cos())
}

impl TargetCurve {
    /// Target level in dB at frequency f, normalised to 0 dB at 1 kHz.
    pub fn db_at(&self, f: f32) -> f32 {
        let f = f.max(1.0);
        let base = match self.kind {
            TargetKind::Flat => 0.0,
            TargetKind::Tilt => self.tilt_db_oct * (f / 1000.0).log2(),
            TargetKind::HarmanRoom => {
                let tilt = self.tilt_db_oct * (f / 1000.0).log2();
                let tilt_ref = self.tilt_db_oct * (self.bass_corner_hz / 1000.0).log2();
                if f >= self.bass_corner_hz {
                    tilt
                } else {
                    tilt_ref + low_shelf_shape(f, self.bass_corner_hz, self.bass_gain_db)
                }
            }
            TargetKind::HarmanHeadphone => {
                table_lookup(HARMAN_OE, f) - table_lookup(HARMAN_OE, 1000.0)
            }
            TargetKind::XCurve => {
                if f <= 2000.0 {
                    0.0
                } else {
                    -3.0 * (f / 2000.0).log2()
                }
            }
            TargetKind::Custom => {
                let mut pts = self.points.clone();
                pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                table_lookup(&pts, f)
            }
        };
        // Roll off below the limit so EQ does not try to force the sub past its limits.
        let roll = if self.sub_rolloff_hz > 1.0 && f < self.sub_rolloff_hz {
            -24.0 * (self.sub_rolloff_hz / f).log2()
        } else {
            0.0
        };
        base + roll
    }

    pub fn on_grid(&self, grid: &[f32]) -> Vec<f32> {
        grid.iter().map(|&f| self.db_at(f)).collect()
    }

    /// Read a curve from a text file with "frequency  dB" per line, skipping comments.
    pub fn load_points(text: &str) -> Vec<(f32, f32)> {
        let mut pts = Vec::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('*') || l.starts_with('#') || l.starts_with("//") {
                continue;
            }
            let mut it = l.split(|c: char| c.is_whitespace() || c == ',' || c == ';');
            let f = it.find(|s| !s.is_empty()).and_then(|s| s.parse::<f32>().ok());
            let d = it.find(|s| !s.is_empty()).and_then(|s| s.parse::<f32>().ok());
            if let (Some(f), Some(d)) = (f, d) {
                if f > 0.0 && f.is_finite() && d.is_finite() {
                    pts.push((f, d));
                }
            }
        }
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        pts
    }
}
