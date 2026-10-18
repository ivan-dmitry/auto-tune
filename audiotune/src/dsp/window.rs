//! Window functions for spectral analysis and for gating impulse responses.

use std::f32::consts::PI;

#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum WindowKind {
    Rectangular,
    Hann,
    Hamming,
    Blackman,
    BlackmanHarris,
    FlatTop,
    /// Tukey 25% — flat in the middle, tapered at both ends. Good for gating an IR.
    Tukey25,
}

impl WindowKind {
    pub const ALL: [WindowKind; 7] = [
        WindowKind::Rectangular,
        WindowKind::Hann,
        WindowKind::Hamming,
        WindowKind::Blackman,
        WindowKind::BlackmanHarris,
        WindowKind::FlatTop,
        WindowKind::Tukey25,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            WindowKind::Rectangular => "Rectangular",
            WindowKind::Hann => "Hann",
            WindowKind::Hamming => "Hamming",
            WindowKind::Blackman => "Blackman",
            WindowKind::BlackmanHarris => "Blackman-Harris",
            WindowKind::FlatTop => "Flat Top",
            WindowKind::Tukey25 => "Tukey 25%",
        }
    }

    /// Build the coefficients for a window of length `n`, in periodic form, which is
    /// the correct one for spectral analysis.
    pub fn make(&self, n: usize) -> Vec<f32> {
        if n == 0 {
            return Vec::new();
        }
        let nf = n as f32;
        (0..n)
            .map(|i| {
                let t = i as f32 / nf; // 0..1, periodic
                let x = 2.0 * PI * t;
                match self {
                    WindowKind::Rectangular => 1.0,
                    WindowKind::Hann => 0.5 - 0.5 * x.cos(),
                    WindowKind::Hamming => 0.54 - 0.46 * x.cos(),
                    WindowKind::Blackman => 0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos(),
                    WindowKind::BlackmanHarris => {
                        0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos()
                            - 0.01168 * (3.0 * x).cos()
                    }
                    WindowKind::FlatTop => {
                        0.21557895 - 0.41663158 * x.cos() + 0.277263158 * (2.0 * x).cos()
                            - 0.083578947 * (3.0 * x).cos()
                            + 0.006947368 * (4.0 * x).cos()
                    }
                    WindowKind::Tukey25 => tukey(t, 0.25),
                }
            })
            .collect()
    }
}

fn tukey(t: f32, alpha: f32) -> f32 {
    if alpha <= 0.0 {
        return 1.0;
    }
    let a2 = alpha / 2.0;
    if t < a2 {
        0.5 * (1.0 + (PI * (t / a2 - 1.0)).cos())
    } else if t > 1.0 - a2 {
        0.5 * (1.0 + (PI * ((t - 1.0) / a2 + 1.0)).cos())
    } else {
        1.0
    }
}

/// Sum of the window coefficients, used to normalise spectrum amplitudes back to
/// the units of the input signal.
pub fn coherent_sum(w: &[f32]) -> f32 {
    let s: f32 = w.iter().sum();
    if s.abs() < 1e-12 {
        1.0
    } else {
        s
    }
}

/// Rising half of a Hann window, length `n`. Used as a fade-in for sweeps and IRs.
pub fn half_hann_rise(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            if n <= 1 {
                1.0
            } else {
                0.5 - 0.5 * (PI * i as f32 / (n - 1) as f32).cos()
            }
        })
        .collect()
}

/// Apply half-Hann fades at both ends of a buffer so it cannot click.
pub fn apply_fades(buf: &mut [f32], fade_in: usize, fade_out: usize) {
    let n = buf.len();
    let fi = fade_in.min(n);
    let fo = fade_out.min(n.saturating_sub(fi));
    if fi > 1 {
        let w = half_hann_rise(fi);
        for i in 0..fi {
            buf[i] *= w[i];
        }
    }
    if fo > 1 {
        let w = half_hann_rise(fo);
        for i in 0..fo {
            buf[n - 1 - i] *= w[i];
        }
    }
}
