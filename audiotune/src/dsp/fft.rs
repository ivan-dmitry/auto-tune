//! A thin wrapper over `realfft`: plans are cached by length and the inverse
//! transform is scaled for you.

use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use rustfft::num_complex::Complex32;
use std::collections::HashMap;
use std::sync::Arc;

pub struct Fft {
    planner: RealFftPlanner<f32>,
    fwd: HashMap<usize, Arc<dyn RealToComplex<f32>>>,
    inv: HashMap<usize, Arc<dyn ComplexToReal<f32>>>,
}

impl Default for Fft {
    fn default() -> Self {
        Self::new()
    }
}

impl Fft {
    pub fn new() -> Self {
        Self {
            planner: RealFftPlanner::<f32>::new(),
            fwd: HashMap::new(),
            inv: HashMap::new(),
        }
    }

    fn plan_fwd(&mut self, n: usize) -> Arc<dyn RealToComplex<f32>> {
        let planner = &mut self.planner;
        self.fwd
            .entry(n)
            .or_insert_with(|| planner.plan_fft_forward(n))
            .clone()
    }

    fn plan_inv(&mut self, n: usize) -> Arc<dyn ComplexToReal<f32>> {
        let planner = &mut self.planner;
        self.inv
            .entry(n)
            .or_insert_with(|| planner.plan_fft_inverse(n))
            .clone()
    }

    /// Real FFT to a complex spectrum of n/2 + 1 bins. Short input is zero-padded.
    pub fn forward(&mut self, input: &[f32], n: usize) -> Vec<Complex32> {
        let r2c = self.plan_fwd(n);
        let mut buf = r2c.make_input_vec();
        let m = input.len().min(n);
        buf[..m].copy_from_slice(&input[..m]);
        for v in buf.iter_mut().skip(m) {
            *v = 0.0;
        }
        let mut out = r2c.make_output_vec();
        // realfft only errors on a length mismatch, and lengths always match here.
        let _ = r2c.process(&mut buf, &mut out);
        out
    }

    /// Inverse FFT back to n real samples, already divided by n.
    pub fn inverse(&mut self, spec: &[Complex32], n: usize) -> Vec<f32> {
        let c2r = self.plan_inv(n);
        let mut buf = c2r.make_input_vec();
        let m = spec.len().min(buf.len());
        buf[..m].copy_from_slice(&spec[..m]);
        for v in buf.iter_mut().skip(m) {
            *v = Complex32::new(0.0, 0.0);
        }
        // realfft requires the DC and Nyquist bins to be purely real.
        if let Some(v) = buf.first_mut() {
            v.im = 0.0;
        }
        if n % 2 == 0 {
            if let Some(v) = buf.last_mut() {
                v.im = 0.0;
            }
        }
        let mut out = c2r.make_output_vec();
        let _ = c2r.process(&mut buf, &mut out);
        let scale = 1.0 / n as f32;
        for v in out.iter_mut() {
            *v *= scale;
        }
        out
    }
}

pub fn next_pow2(n: usize) -> usize {
    let mut p = 1usize;
    while p < n {
        p <<= 1;
    }
    p.max(2)
}

/// Centre frequency of bin k for an n-point FFT at sample rate sr.
#[inline]
pub fn bin_freq(k: usize, n: usize, sr: f32) -> f32 {
    k as f32 * sr / n as f32
}
