//! AudioTune's signal-processing core.
//!
//! Everything in here is plain Rust with no GUI or OS dependency, so it behaves
//! identically on Windows and Linux and can be unit-tested on its own.

// This layer is written as a reusable library: part of its surface is exercised only
// by the test suite, so dead-code warnings here would be noise rather than signal.
#![allow(dead_code)]

pub mod autoeq;
pub mod biquad;
pub mod calibration;
pub mod device_eq;
pub mod fft;
pub mod generator;
pub mod measurement;
pub mod octave;
pub mod spectrum;
pub mod target;
pub mod window;

/// Floor used whenever a log of zero would otherwise be taken.
pub const DB_FLOOR: f32 = -160.0;

#[inline]
pub fn lin_to_db(x: f32) -> f32 {
    if x <= 1e-20 {
        DB_FLOOR
    } else {
        20.0 * x.log10()
    }
}

#[inline]
pub fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[inline]
pub fn power_to_db(p: f32) -> f32 {
    if p <= 1e-40 {
        DB_FLOOR
    } else {
        10.0 * p.log10()
    }
}

#[cfg(test)]
mod tests;
