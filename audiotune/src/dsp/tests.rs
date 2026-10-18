//! Tests for the DSP core. The maths has to be right before any plot can be believed.

use super::*;
use crate::dsp::autoeq::{fit, FitConfig};
use crate::dsp::biquad::{bw_to_q, q_to_bw, Bank, Coeffs, FilterKind, FilterSpec};
use crate::dsp::device_eq::{fit_graphic, fit_parametric, BandLayout, DeviceKind, DeviceSpec};
use crate::dsp::fft::Fft;
use crate::dsp::generator::{inverse_filter, log_sweep, SweepConfig};
use crate::dsp::measurement::{decay_times, deconvolve, harmonic_distortion, ir_to_fr, IrWindow};
use crate::dsp::octave::{interp_log, iso_centers, log_grid, smooth_octave};
use crate::dsp::spectrum::{measure_level, Analyzer, Averaging};
use crate::dsp::target::{TargetCurve, TargetKind};
use crate::dsp::window::WindowKind;

const SR: f32 = 48_000.0;

// ------------------------------------------------------------------- biquad

#[test]
fn peaking_hits_its_gain_at_center() {
    for gain in [-12.0f32, -6.0, 3.0, 9.0] {
        for q in [0.7f32, 2.0, 6.0] {
            for f in [50.0f32, 1000.0, 8000.0] {
                let c = Coeffs::design(&FilterSpec::peaking(f, gain, q), SR);
                let got = c.response_db(f, SR);
                assert!((got - gain).abs() < 0.05, "f={f} q={q} gain={gain} gave {got}");
            }
        }
    }
}

#[test]
fn peaking_is_flat_far_from_center() {
    let c = Coeffs::design(&FilterSpec::peaking(1000.0, 10.0, 4.0), SR);
    assert!(c.response_db(50.0, SR).abs() < 0.3);
    assert!(c.response_db(15_000.0, SR).abs() < 0.5);
}

#[test]
fn shelves_reach_full_gain_at_the_ends() {
    let ls = Coeffs::design(
        &FilterSpec {
            kind: FilterKind::LowShelf,
            freq: 200.0,
            gain_db: 6.0,
            q: 0.7,
            enabled: true,
        },
        SR,
    );
    assert!(
        (ls.response_db(20.0, SR) - 6.0).abs() < 0.3,
        "{}",
        ls.response_db(20.0, SR)
    );
    assert!(ls.response_db(10_000.0, SR).abs() < 0.3);

    let hs = Coeffs::design(
        &FilterSpec {
            kind: FilterKind::HighShelf,
            freq: 4000.0,
            gain_db: -5.0,
            q: 0.7,
            enabled: true,
        },
        SR,
    );
    assert!((hs.response_db(20_000.0, SR) + 5.0).abs() < 0.4);
    assert!(hs.response_db(100.0, SR).abs() < 0.3);
}

#[test]
fn bandwidth_and_q_are_inverse_of_each_other() {
    for bw in [0.1f32, 0.5, 1.0, 2.0, 4.0] {
        let q = bw_to_q(bw);
        assert!((q_to_bw(q) - bw).abs() < 1e-3, "bw={bw} q={q}");
    }
}

#[test]
fn disabled_filter_is_a_passthrough() {
    let spec = FilterSpec {
        enabled: false,
        gain_db: 12.0,
        ..FilterSpec::default()
    };
    let c = Coeffs::design(&spec, SR);
    assert!(c.response_db(1000.0, SR).abs() < 1e-6);
}

/// Running the filter for real must agree with the response formula.
#[test]
fn running_the_filter_matches_its_computed_response() {
    let spec = FilterSpec::peaking(1000.0, 6.0, 2.0);
    let mut bank = Bank::from_specs(&[spec], 0.0, SR);
    let n = 48_000;
    let w = 2.0 * std::f32::consts::PI * 1000.0 / SR;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        out.push(bank.process((w * i as f32).sin()));
    }
    // Skip the first 5000 samples so the transient is gone, then measure RMS.
    let tail = &out[5000..];
    let rms = (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt();
    let got_db = lin_to_db(rms * 2f32.sqrt());
    assert!((got_db - 6.0).abs() < 0.1, "measured {got_db} dB");
}

#[test]
fn preamp_scales_the_signal() {
    let mut bank = Bank::from_specs(&[], -6.0, SR);
    let y = bank.process(1.0);
    assert!((y - db_to_lin(-6.0)).abs() < 1e-6);
}

// ------------------------------------------------------------------- octave

#[test]
fn iso_band_centers_include_the_1k_anchor() {
    let c = iso_centers(3, 20.0, 20_000.0);
    assert!(c.iter().any(|f| (f - 1000.0).abs() < 0.01));
    assert!(c.len() >= 29 && c.len() <= 32, "{} bands", c.len());
    let ratio = c[1] / c[0];
    assert!((ratio - 2f32.powf(1.0 / 3.0)).abs() < 1e-4);
}

#[test]
fn log_grid_is_monotonic_and_covers_the_range() {
    let g = log_grid(20.0, 20_000.0, 24);
    assert!(g.windows(2).all(|w| w[1] > w[0]));
    assert!((g[0] - 20.0).abs() < 0.01);
    assert!((g[g.len() - 1] - 20_000.0).abs() < 1.0);
}

#[test]
fn interpolation_is_linear_on_the_log_axis() {
    let f = vec![100.0f32, 1000.0];
    let v = vec![0.0f32, 10.0];
    // The log midpoint of 100 and 1000 is about 316 Hz.
    let mid = interp_log(&f, &v, 316.227_77);
    assert!((mid - 5.0).abs() < 0.01, "{mid}");
    assert_eq!(interp_log(&f, &v, 10.0), 0.0);
    assert_eq!(interp_log(&f, &v, 10_000.0), 10.0);
}

#[test]
fn smoothing_flattens_a_narrow_spike_but_leaves_the_rest() {
    let g = log_grid(20.0, 20_000.0, 96);
    let mut db = vec![0.0f32; g.len()];
    let i = g.len() / 2;
    db[i] = 20.0;
    let s = smooth_octave(&g, &db, 3.0);
    assert!(s[i] < 10.0, "the spike should come down, it is {}", s[i]);
    assert!(s[10].abs() < 0.01);
}

// ------------------------------------------------------------------ spectrum

#[test]
fn analyzer_reports_a_full_scale_sine_at_0_dbfs() {
    let n = 16_384;
    let mut a = Analyzer::new(SR, n, WindowKind::Hann);
    a.averaging = Averaging::Infinite;
    // Pick a frequency exactly on a bin centre, or scalloping costs up to 1.4 dB.
    let k = 341usize;
    let f = k as f32 * SR / n as f32;
    let w = 2.0 * std::f32::consts::PI * f / SR;
    let sig: Vec<f32> = (0..n * 8).map(|i| (w * i as f32).sin()).collect();
    a.push(&sig);
    let db = a.magnitude_db();
    let peak = db[k - 2..=k + 2].iter().cloned().fold(f32::MIN, f32::max);
    assert!(peak.abs() < 0.1, "peak {peak} dBFS at {f} Hz");
    // Away from the peak it must fall off; Hann leaks very little.
    assert!(db[k + 20] < -70.0, "leakage {} dB", db[k + 20]);
}

#[test]
fn analyzer_sees_the_right_frequency() {
    let n = 8192;
    let mut a = Analyzer::new(SR, n, WindowKind::Hann);
    a.averaging = Averaging::Infinite;
    let f = 3000.0f32;
    let w = 2.0 * std::f32::consts::PI * f / SR;
    let sig: Vec<f32> = (0..n * 4).map(|i| (w * i as f32).sin() * 0.5).collect();
    a.push(&sig);
    let db = a.magnitude_db();
    let (mut best, mut bi) = (f32::MIN, 0usize);
    for (i, v) in db.iter().enumerate() {
        if *v > best {
            best = *v;
            bi = i;
        }
    }
    let got = bi as f32 * SR / n as f32;
    assert!((got - f).abs() < SR / n as f32 * 1.5, "found {got} Hz");
    // Amplitude 0.5 is about -6 dBFS.
    assert!((best + 6.02).abs() < 0.6, "{best} dBFS");
}

#[test]
fn level_meter_matches_a_known_sine() {
    let w = 2.0 * std::f32::consts::PI * 1000.0 / SR;
    let sig: Vec<f32> = (0..48_000).map(|i| (w * i as f32).sin() * 0.5).collect();
    let l = measure_level(&sig);
    // RMS of a 0.5 amplitude sine is 0.3536, which is -9.03 dBFS.
    assert!((l.rms_db + 9.03).abs() < 0.05, "{}", l.rms_db);
    assert!((l.peak_db + 6.02).abs() < 0.1, "{}", l.peak_db);
    assert!(!l.clipped);
}

// --------------------------------------------------------------- sweep / IR

#[test]
fn sweep_stays_inside_full_scale_and_has_the_right_length() {
    let cfg = SweepConfig {
        duration_s: 2.0,
        level_db: -6.0,
        ..SweepConfig::default()
    };
    let s = log_sweep(&cfg, SR);
    assert_eq!(s.len(), (2.0 * SR) as usize);
    let peak = s.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(peak <= db_to_lin(-6.0) + 1e-3, "peak {peak}");
    // Both ends must be faded so it cannot click.
    assert!(s[0].abs() < 1e-3);
    assert!(s[s.len() - 1].abs() < 1e-2);
}

/// The sweep deconvolved against its own inverse filter must give a Dirac impulse.
#[test]
fn deconvolving_the_sweep_gives_a_clean_impulse() {
    let cfg = SweepConfig {
        f_start: 20.0,
        f_end: 20_000.0,
        duration_s: 2.0,
        tail_s: 0.0,
        level_db: -6.0,
        ..SweepConfig::default()
    };
    let sweep = log_sweep(&cfg, SR);
    let inv = inverse_filter(&sweep, &cfg, SR);
    let mut fft = Fft::new();
    let ir = deconvolve(&sweep, &inv, SR, &mut fft);

    // The impulse lands at the end of the inverse filter, so the delay is zero.
    assert!(ir.delay_samples.abs() <= 2, "delay {} samples", ir.delay_samples);
    let peak = ir.peak_value().abs();
    let mut side = 0.0f32;
    for (i, v) in ir.data.iter().enumerate() {
        if (i as i64 - ir.peak_idx as i64).abs() > 64 {
            side = side.max(v.abs());
        }
    }
    assert!(peak > side * 30.0, "peak {peak} does not stand out from {side}");
}

/// Inject a known delay into the recording; the app must measure the same delay back.
#[test]
fn measured_delay_matches_the_delay_we_injected() {
    let cfg = SweepConfig {
        duration_s: 1.5,
        tail_s: 0.0,
        level_db: -6.0,
        ..SweepConfig::default()
    };
    let sweep = log_sweep(&cfg, SR);
    let inv = inverse_filter(&sweep, &cfg, SR);
    let delay = 1234usize;
    let mut rec = vec![0.0f32; delay];
    rec.extend_from_slice(&sweep);
    rec.extend(std::iter::repeat(0.0).take(2048));

    let mut fft = Fft::new();
    let ir = deconvolve(&rec, &inv, SR, &mut fft);
    assert!(
        (ir.delay_samples - delay as i64).abs() <= 2,
        "measured {} samples, injected {delay}",
        ir.delay_samples
    );
}

/// Push the sweep through a known filter; the measured response must recover it.
#[test]
fn round_trip_recovers_a_known_filter_response() {
    let cfg = SweepConfig {
        f_start: 20.0,
        f_end: 20_000.0,
        duration_s: 4.0,
        tail_s: 0.3,
        level_db: -6.0,
        fade_in_ms: 20.0,
        fade_out_ms: 60.0,
        repeats: 1,
    };
    let sweep = log_sweep(&cfg, SR);
    let inv = inverse_filter(&sweep, &cfg, SR);

    // Pretend speaker: a +6 dB bump at 1 kHz and an -8 dB dip at 120 Hz.
    let specs = vec![
        FilterSpec::peaking(1000.0, 6.0, 2.0),
        FilterSpec::peaking(120.0, -8.0, 3.0),
    ];
    let mut bank = Bank::from_specs(&specs, 0.0, SR);
    let mut rec: Vec<f32> = vec![0.0; 512]; // a little delay, as in real life
    for &s in &sweep {
        rec.push(bank.process(s));
    }
    rec.extend(std::iter::repeat(0.0).take((0.3 * SR) as usize));

    let mut fft = Fft::new();
    let ir = deconvolve(&rec, &inv, SR, &mut fft);
    let fr = ir_to_fr(&ir, &IrWindow::default(), &mut fft);
    assert!(!fr.is_empty());

    let grid = log_grid(40.0, 16_000.0, 24);
    let measured = fr.smoothed_on_grid(&grid, 24.0);
    let mid: f32 = {
        let v: Vec<f32> = grid
            .iter()
            .zip(measured.iter())
            .filter(|(f, _)| **f >= 400.0 && **f <= 600.0)
            .map(|(_, d)| *d)
            .collect();
        v.iter().sum::<f32>() / v.len().max(1) as f32
    };
    let expect_mid = biquad::total_response_db(&specs, &[500.0], SR)[0];
    let truth = biquad::total_response_db(&specs, &grid, SR);

    let mut worst = 0.0f32;
    for i in 0..grid.len() {
        let got = measured[i] - mid + expect_mid;
        let err = (got - truth[i]).abs();
        if err > worst {
            worst = err;
        }
    }
    assert!(worst < 2.0, "worst deviation {worst:.2} dB");
}

#[test]
fn decay_time_matches_a_synthetic_exponential_tail() {
    let rt60 = 0.5f32;
    let n = (2.0 * SR) as usize;
    let mut data = vec![0.0f32; n];
    let mut rng = crate::dsp::generator::Rng::default();
    for i in 0..n {
        let t = i as f32 / SR;
        let env = 10f32.powf(-60.0 * t / rt60 / 20.0);
        data[i] = rng.next_f32() * env;
    }
    data[0] = 1.0;
    let ir = crate::dsp::measurement::ImpulseResponse {
        sr: SR,
        data,
        peak_idx: 0,
        delay_samples: 0,
    };
    let d = decay_times(&ir).expect("should produce a result");
    assert!((d.t30 - rt60).abs() < 0.06, "T30 = {} (wanted {rt60})", d.t30);
    assert!(d.t30_fit > 0.98, "fit quality {}", d.t30_fit);
}

#[test]
fn distortion_is_near_zero_for_a_linear_path() {
    let cfg = SweepConfig {
        duration_s: 3.0,
        tail_s: 0.2,
        level_db: -6.0,
        ..SweepConfig::default()
    };
    let sweep = log_sweep(&cfg, SR);
    let inv = inverse_filter(&sweep, &cfg, SR);
    let mut rec = sweep.clone();
    rec.extend(std::iter::repeat(0.0).take((0.2 * SR) as usize));
    let mut fft = Fft::new();
    let ir = deconvolve(&rec, &inv, SR, &mut fft);
    let d = harmonic_distortion(&ir, &cfg, 5);
    assert!(d.thd_percent < 1.0, "THD {}%", d.thd_percent);
}

#[test]
fn distortion_rises_when_the_path_clips() {
    let cfg = SweepConfig {
        duration_s: 3.0,
        tail_s: 0.2,
        level_db: -6.0,
        ..SweepConfig::default()
    };
    let sweep = log_sweep(&cfg, SR);
    let inv = inverse_filter(&sweep, &cfg, SR);
    // Hard clipping generates odd harmonics.
    let mut rec: Vec<f32> = sweep.iter().map(|s| (s * 4.0).clamp(-0.5, 0.5)).collect();
    rec.extend(std::iter::repeat(0.0).take((0.2 * SR) as usize));
    let mut fft = Fft::new();
    let ir = deconvolve(&rec, &inv, SR, &mut fft);
    let d = harmonic_distortion(&ir, &cfg, 5);
    assert!(d.thd_percent > 5.0, "THD only {}%", d.thd_percent);
    // Symmetric clipping makes the 3rd harmonic stand out.
    assert!(d.harmonics_db[1] > -30.0, "H3 = {} dB", d.harmonics_db[1]);
}

// -------------------------------------------------------------------- target

#[test]
fn every_target_is_zero_db_at_1k() {
    for kind in TargetKind::ALL {
        let t = TargetCurve {
            kind,
            ..TargetCurve::default()
        };
        assert!(
            t.db_at(1000.0).abs() < 0.01,
            "{:?} gives {} dB at 1 kHz",
            kind,
            t.db_at(1000.0)
        );
    }
}

#[test]
fn harman_room_target_is_continuous_and_lifts_the_bass() {
    let t = TargetCurve {
        kind: TargetKind::HarmanRoom,
        tilt_db_oct: -0.8,
        bass_gain_db: 4.0,
        bass_corner_hz: 105.0,
        ..TargetCurve::default()
    };
    assert!(t.db_at(30.0) > t.db_at(1000.0) + 3.0);
    assert!(t.db_at(10_000.0) < t.db_at(1000.0) - 1.0);
    // No step at the corner frequency.
    let g = log_grid(20.0, 20_000.0, 192);
    for w in g.windows(2) {
        let jump = (t.db_at(w[1]) - t.db_at(w[0])).abs();
        assert!(jump < 0.35, "{jump} dB step between {} and {}", w[0], w[1]);
    }
}

#[test]
fn tilt_target_has_the_requested_slope() {
    let t = TargetCurve {
        kind: TargetKind::Tilt,
        tilt_db_oct: -1.0,
        ..TargetCurve::default()
    };
    assert!((t.db_at(2000.0) + 1.0).abs() < 0.01);
    assert!((t.db_at(500.0) - 1.0).abs() < 0.01);
}

#[test]
fn target_points_parse_from_text() {
    let text = "* comment\n20 3.5\n100, 1.0\n1000;0\ngarbage\n";
    let pts = TargetCurve::load_points(text);
    assert_eq!(pts.len(), 3);
    assert_eq!(pts[0], (20.0, 3.5));
    assert_eq!(pts[2], (1000.0, 0.0));
}

// -------------------------------------------------------------------- autoeq

/// Auto EQ has to pull a badly deformed response close to flat.
#[test]
fn autofit_flattens_a_distorted_response() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![
        FilterSpec::peaking(60.0, 9.0, 2.5),
        FilterSpec::peaking(300.0, -6.0, 1.5),
        FilterSpec::peaking(4000.0, 5.0, 3.0),
    ];
    let measured = biquad::total_response_db(&broken, &grid, SR);
    let target = vec![0.0f32; grid.len()];

    let cfg = FitConfig {
        f_min: 30.0,
        f_max: 16_000.0,
        max_filters: 8,
        ..FitConfig::default()
    };
    let res = fit(&grid, &measured, &target, SR, &cfg);
    assert!(!res.filters.is_empty());
    assert!(
        res.rms_after < res.rms_before * 0.4,
        "before {:.2} dB, after {:.2} dB",
        res.rms_before,
        res.rms_after
    );
    assert!(res.rms_after < 1.0, "{:.2} dB left", res.rms_after);
    assert!(res.preamp_db <= 0.0);
}

#[test]
fn autofit_does_nothing_when_the_response_is_already_on_target() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let measured = vec![0.0f32; grid.len()];
    let target = vec![0.0f32; grid.len()];
    let res = fit(&grid, &measured, &target, SR, &FitConfig::default());
    assert!(
        res.filters.is_empty(),
        "produced {} pointless filters",
        res.filters.len()
    );
}

#[test]
fn autofit_respects_the_gain_limits() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![FilterSpec::peaking(80.0, -20.0, 2.0)];
    let measured = biquad::total_response_db(&broken, &grid, SR);
    let cfg = FitConfig {
        max_boost_db: 3.0,
        max_cut_db: 3.0,
        ..FitConfig::default()
    };
    let res = fit(&grid, &measured, &vec![0.0; grid.len()], SR, &cfg);
    for f in &res.filters {
        assert!(
            f.gain_db <= 3.001 && f.gain_db >= -3.001,
            "filter exceeds the limit: {} dB",
            f.gain_db
        );
        assert!(f.q >= cfg.min_q - 1e-6 && f.q <= cfg.max_q + 1e-6);
    }
}

#[test]
fn autofit_stays_inside_the_frequency_range() {
    let grid = log_grid(20.0, 20_000.0, 48);
    // The error sits at 18 Hz, outside the allowed range, so nothing should go there.
    let broken = vec![FilterSpec::peaking(18.0, 12.0, 3.0)];
    let measured = biquad::total_response_db(&broken, &grid, SR);
    let cfg = FitConfig {
        f_min: 100.0,
        f_max: 10_000.0,
        use_low_shelf: false,
        use_high_shelf: false,
        ..FitConfig::default()
    };
    let res = fit(&grid, &measured, &vec![0.0; grid.len()], SR, &cfg);
    for f in &res.filters {
        assert!(f.freq > 40.0, "filter at {} Hz, outside the range", f.freq);
    }
}

#[test]
fn suggested_low_limit_finds_where_the_bass_rolls_off() {
    let grid = log_grid(15.0, 20_000.0, 48);
    let specs = vec![FilterSpec {
        kind: FilterKind::HighPass,
        freq: 60.0,
        gain_db: 0.0,
        q: 0.7,
        enabled: true,
    }];
    let measured = biquad::total_response_db(&specs, &grid, SR);
    let limit = autoeq::suggest_low_limit(&grid, &measured, 10.0);
    assert!(
        limit > 25.0 && limit < 60.0,
        "suggested {limit} Hz for a speaker cut off at 60 Hz"
    );
}

// ---------------------------------------------------------- external EQ unit

fn graphic_spec() -> DeviceSpec {
    DeviceSpec {
        name: "TestEQ".into(),
        kind: DeviceKind::Graphic,
        layout: BandLayout::ThirdOctave,
        max_boost_db: 12.0,
        max_cut_db: 12.0,
        step_db: 0.5,
        ..DeviceSpec::default()
    }
}

#[test]
fn graphic_layouts_have_the_expected_band_counts() {
    assert_eq!(BandLayout::ThirdOctave.centers().len(), 31);
    assert_eq!(BandLayout::TwoThirdOctave.centers().len(), 15);
    assert_eq!(BandLayout::FullOctave.centers().len(), 10);
    assert_eq!(BandLayout::PlayerTenBand.centers().len(), 10);
    // Custom has no fixed grid of its own; the device spec supplies one.
    assert!(BandLayout::Custom.centers().is_empty());
    // Every fixed layout is ascending and includes 1 kHz.
    for l in BandLayout::ALL {
        if l == BandLayout::Custom {
            continue;
        }
        let c = l.centers();
        assert!(c.windows(2).all(|w| w[1] > w[0]), "{:?} is not ascending", l);
        assert!(
            c.iter().any(|f| (*f - 1000.0).abs() < 1.0),
            "{:?} has no 1 kHz band",
            l
        );
    }
}

#[test]
fn graphic_solver_produces_settings_within_the_units_limits() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![
        FilterSpec::peaking(80.0, 8.0, 3.0),
        FilterSpec::peaking(2500.0, -5.0, 2.0),
    ];
    // What we want the EQ to do is the opposite of the fault.
    let desired: Vec<f32> = biquad::total_response_db(&broken, &grid, SR)
        .iter()
        .map(|v| -v)
        .collect();
    let spec = graphic_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 30.0, 16_000.0);

    assert_eq!(res.settings.len(), 31);
    for s in &res.settings {
        assert!(
            s.gain_db <= spec.max_boost_db + 1e-6 && s.gain_db >= -spec.max_cut_db - 1e-6,
            "band {} at {} dB is outside the limits",
            s.index,
            s.gain_db
        );
        // Every value must land on the unit's gain step.
        let steps = s.gain_db / spec.step_db;
        assert!(
            (steps - steps.round()).abs() < 1e-4,
            "band {} = {} dB is not a multiple of {}",
            s.index,
            s.gain_db,
            spec.step_db
        );
    }
}

/// The whole point of solving rather than copying: the combined response of the
/// overlapping bands must actually match the requested correction.
#[test]
fn graphic_solver_beats_naive_per_band_copying() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![
        FilterSpec::peaking(80.0, 7.0, 3.0),
        FilterSpec::peaking(1600.0, -6.0, 2.5),
        FilterSpec::peaking(6300.0, 4.0, 2.0),
    ];
    let desired: Vec<f32> = biquad::total_response_db(&broken, &grid, SR)
        .iter()
        .map(|v| -v)
        .collect();
    let spec = graphic_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 30.0, 16_000.0);

    // Naive approach: set each slider to the error at its own centre frequency.
    let band_q = bw_to_q(spec.band_width_octaves());
    let naive: Vec<FilterSpec> = spec
        .centers()
        .iter()
        .map(|&fc| {
            let g = crate::dsp::octave::interp_log(&grid, &desired, fc)
                .clamp(-spec.max_cut_db, spec.max_boost_db);
            FilterSpec::peaking(fc, g, band_q)
        })
        .collect();
    let naive_resp = biquad::total_response_db(&naive, &grid, SR);

    let inside = |f: f32| f >= 30.0 && f <= 16_000.0;
    let err = |resp: &[f32]| -> f32 {
        let mut s = 0.0f64;
        let mut n = 0usize;
        for i in 0..grid.len() {
            if inside(grid[i]) {
                let e = (desired[i] - resp[i]) as f64;
                s += e * e;
                n += 1;
            }
        }
        (s / n.max(1) as f64).sqrt() as f32
    };
    let solved_err = err(&res.achieved);
    let naive_err = err(&naive_resp);
    assert!(
        solved_err < naive_err * 0.6,
        "solver {solved_err:.2} dB vs naive {naive_err:.2} dB - not enough of a win"
    );
    assert!(solved_err < 1.5, "solver still off by {solved_err:.2} dB");
    assert!(
        res.rms_error_after < res.rms_error_before,
        "reported error did not improve"
    );
}

#[test]
fn graphic_solver_leaves_a_flat_response_alone() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let desired = vec![0.0f32; grid.len()];
    let res = fit_graphic(&grid, &desired, &graphic_spec(), SR, 30.0, 16_000.0);
    for s in &res.settings {
        assert!(
            s.gain_db.abs() < 1e-6,
            "band {} moved to {} dB for no reason",
            s.index,
            s.gain_db
        );
    }
    assert_eq!(res.active().count(), 0);
}

#[test]
fn output_trim_cancels_the_largest_boost() {
    let grid = log_grid(20.0, 20_000.0, 48);
    // Ask for a broad +6 dB lift, which the unit will deliver as boosts.
    let desired = vec![6.0f32; grid.len()];
    let res = fit_graphic(&grid, &desired, &graphic_spec(), SR, 30.0, 16_000.0);
    let peak = res.achieved.iter().cloned().fold(f32::MIN, f32::max);
    assert!(peak > 1.0, "expected real boost, got {peak}");
    assert!(
        res.output_trim_db <= -peak + 1e-3,
        "trim {} does not cover a peak of {}",
        res.output_trim_db,
        peak
    );
}

#[test]
fn parametric_settings_are_snapped_and_capped() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![
        FilterSpec::peaking(75.0, 9.0, 3.3),
        FilterSpec::peaking(900.0, -7.0, 1.7),
        FilterSpec::peaking(5500.0, 4.0, 2.2),
    ];
    let measured = biquad::total_response_db(&broken, &grid, SR);
    let desired: Vec<f32> = measured.iter().map(|v| -v).collect();
    let fitted = fit(
        &grid,
        &measured,
        &vec![0.0; grid.len()],
        SR,
        &FitConfig::default(),
    );

    let spec = DeviceSpec {
        name: "ParaEQ".into(),
        kind: DeviceKind::Parametric,
        parametric_bands: 4,
        max_boost_db: 6.0,
        max_cut_db: 6.0,
        step_db: 0.5,
        bw_step_octave: 0.1,
        ..DeviceSpec::default()
    };
    let res = fit_parametric(
        &grid,
        &desired,
        &fitted.filters,
        &spec,
        SR,
        30.0,
        16_000.0,
    );

    assert!(res.settings.len() <= 4, "used {} bands", res.settings.len());
    assert!(!res.settings.is_empty());
    for s in &res.settings {
        assert!(s.gain_db.abs() <= 6.0 + 1e-6, "gain {} out of range", s.gain_db);
        let steps = s.gain_db / spec.step_db;
        assert!((steps - steps.round()).abs() < 1e-4, "gain not on the step grid");
        let bw = s.bandwidth_oct.expect("bandwidth reported");
        let bsteps = bw / spec.bw_step_octave;
        assert!(
            (bsteps - bsteps.round()).abs() < 1e-3,
            "bandwidth {bw} not on the 0.1 octave grid"
        );
        assert!(s.q.expect("q reported") > 0.0);
    }
    // Bands come out ordered by frequency, which is how they get typed in.
    assert!(res
        .settings
        .windows(2)
        .all(|w| w[0].freq_hz <= w[1].freq_hz));
}

#[test]
fn device_settings_convert_back_to_filters_for_preview() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![FilterSpec::peaking(125.0, 6.0, 3.0)];
    let desired: Vec<f32> = biquad::total_response_db(&broken, &grid, SR)
        .iter()
        .map(|v| -v)
        .collect();
    let spec = graphic_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 30.0, 16_000.0);
    let filters = res.to_filter_specs(&spec);
    assert!(!filters.is_empty());
    // Replaying those filters must reproduce the response the app predicted.
    let replay = biquad::total_response_db(&filters, &grid, SR);
    let mut worst = 0.0f32;
    for i in 0..grid.len() {
        if grid[i] >= 30.0 && grid[i] <= 16_000.0 {
            worst = worst.max((replay[i] - res.achieved[i]).abs());
        }
    }
    assert!(worst < 0.05, "preview differs from the prediction by {worst} dB");
}

#[test]
fn settings_sheet_lists_every_band() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let desired: Vec<f32> = grid.iter().map(|f| if *f < 200.0 { -4.0 } else { 0.0 }).collect();
    let spec = graphic_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 30.0, 16_000.0);
    let sheet = crate::dsp::device_eq::settings_sheet(&res, &spec, "test");
    assert!(sheet.contains("TestEQ"));
    assert!(sheet.contains("31 bands"));
    // One line per band, plus the header block.
    assert_eq!(sheet.lines().filter(|l| l.contains(" dB")).count() >= 31, true);

    let csv = crate::dsp::device_eq::settings_csv(&res, &spec);
    assert_eq!(csv.lines().count(), 32, "header plus 31 rows");
    assert!(csv.starts_with("band,freq_hz,gain_db"));
}

// --------------------------------------------------------------- calibration

#[test]
fn mic_calibration_is_subtracted_from_the_measurement() {
    let cal = crate::dsp::calibration::MicCalibration::parse(
        "test.cal",
        "* mic\n100 2.0\n1000 0.0\n10000 -3.0\n",
    );
    assert!(cal.enabled);
    let freqs = vec![100.0f32, 1000.0, 10_000.0];
    let mut db = vec![50.0f32, 50.0, 50.0];
    cal.apply(&freqs, &mut db);
    assert!((db[0] - 48.0).abs() < 1e-4);
    assert!((db[1] - 50.0).abs() < 1e-4);
    assert!((db[2] - 53.0).abs() < 1e-4);
}

#[test]
fn spl_calibration_round_trips() {
    let mut cal = crate::dsp::calibration::SplCalibration::default();
    cal.calibrate(-20.0, 75.0);
    assert!((cal.spl_at_full_scale - 95.0).abs() < 1e-4);
    assert!((cal.to_spl(-20.0) - 75.0).abs() < 1e-4);
    assert!((cal.from_spl(75.0) + 20.0).abs() < 1e-4);
}

#[test]
fn a_weighting_matches_the_standard_table() {
    use crate::dsp::calibration::Weighting;
    // Table values from IEC 61672, tolerance 0.6 dB.
    for (f, expect) in [
        (31.5f32, -39.4f32),
        (125.0, -16.1),
        (1000.0, 0.0),
        (4000.0, 1.0),
        (8000.0, -1.1),
    ] {
        let got = Weighting::A.db_at(f);
        assert!(
            (got - expect).abs() < 0.6,
            "A at {f} Hz = {got}, table says {expect}"
        );
    }
    assert!(Weighting::Z.db_at(50.0).abs() < 1e-9);
}

// ------------------------------------------------------------------ utility

#[test]
fn db_conversions_round_trip() {
    for db in [-60.0f32, -12.0, 0.0, 6.0] {
        assert!((lin_to_db(db_to_lin(db)) - db).abs() < 1e-3);
    }
    assert_eq!(lin_to_db(0.0), DB_FLOOR);
}

#[test]
fn resampling_preserves_a_sine() {
    use crate::audio::fileio::resample;
    let f = 1000.0f32;
    let src_sr = 44_100.0f32;
    let w = 2.0 * std::f32::consts::PI * f / src_sr;
    let sig: Vec<f32> = (0..44_100).map(|i| (w * i as f32).sin()).collect();
    let out = resample(&sig, src_sr, 48_000.0);
    assert!((out.len() as i64 - 48_000).abs() < 4, "{} samples", out.len());
    // Ignore both ends, where the sinc kernel is truncated, then check the RMS.
    let core = &out[2000..out.len() - 2000];
    let rms = (core.iter().map(|v| v * v).sum::<f32>() / core.len() as f32).sqrt();
    assert!((rms - 0.7071).abs() < 0.02, "RMS {rms}");
    let peak = core.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(peak < 1.02, "peak {peak} - aliasing");
}

#[test]
fn window_functions_are_sane() {
    for k in WindowKind::ALL {
        let w = k.make(1024);
        assert_eq!(w.len(), 1024);
        assert!(w.iter().all(|v| v.is_finite()));
        let mean = w.iter().sum::<f32>() / 1024.0;
        assert!(mean > 0.15 && mean <= 1.001, "{:?} mean {mean}", k);
    }
    // Hann starts at zero and peaks in the middle.
    let h = WindowKind::Hann.make(1000);
    assert!(h[0].abs() < 1e-6);
    assert!((h[500] - 1.0).abs() < 1e-3);
}

// ------------------------------------------- player-style and custom layouts

fn player_spec() -> DeviceSpec {
    DeviceSpec::preset_player_10()
}

#[test]
fn player_layout_matches_the_labels_on_screen() {
    let spec = player_spec();
    let c = spec.centers();
    assert_eq!(c.len(), 10);
    assert_eq!(c[0], 31.0);
    assert_eq!(c[9], 16_000.0);
    assert!(spec.shelf_ends);
    assert_eq!(spec.max_boost_db, 20.0);
    assert!((spec.step_db - 0.1).abs() < 1e-6);
    // The end controls read as "<31" and ">16k", the rest as plain frequencies.
    assert_eq!(spec.band_label(0, 10, c[0]), "<31.0");
    assert_eq!(spec.band_label(9, 10, c[9]), ">16k");
    assert_eq!(spec.band_label(4, 10, c[4]), "500");
}

#[test]
fn shelved_end_bands_are_modelled_as_shelves() {
    let spec = player_spec();
    assert_eq!(spec.band_kind(0, 10), FilterKind::LowShelf);
    assert_eq!(spec.band_kind(9, 10), FilterKind::HighShelf);
    assert_eq!(spec.band_kind(3, 10), FilterKind::Peaking);

    // Turning the setting off makes every band a peak again.
    let mut flat_ends = spec.clone();
    flat_ends.shelf_ends = false;
    assert_eq!(flat_ends.band_kind(0, 10), FilterKind::Peaking);
}

/// The point of modelling "<31" as a shelf: it has to keep working below 31 Hz.
#[test]
fn a_shelved_bottom_band_still_lifts_20_hz() {
    let grid = log_grid(15.0, 20_000.0, 48);
    let spec = player_spec();
    // Ask for a 6 dB lift below 60 Hz and nothing elsewhere.
    let desired: Vec<f32> = grid
        .iter()
        .map(|f| if *f < 60.0 { 6.0 } else { 0.0 })
        .collect();
    let res = fit_graphic(&grid, &desired, &spec, SR, 20.0, 16_000.0);

    let at = |f: f32| crate::dsp::octave::interp_log(&res.grid, &res.achieved, f);
    assert!(at(20.0) > 3.5, "20 Hz only got {:.2} dB", at(20.0));
    assert!(at(31.0) > 3.5, "31 Hz only got {:.2} dB", at(31.0));
    // And it must not spill far up the band.
    assert!(at(1000.0).abs() < 1.5, "1 kHz moved by {:.2} dB", at(1000.0));

    // With peaking end bands instead, 20 Hz is left behind - which is why the
    // shelf model matters.
    let mut peaks = spec.clone();
    peaks.shelf_ends = false;
    let res2 = fit_graphic(&grid, &desired, &peaks, SR, 20.0, 16_000.0);
    let at2 = |f: f32| crate::dsp::octave::interp_log(&res2.grid, &res2.achieved, f);
    assert!(
        at(20.0) > at2(20.0),
        "shelf {:.2} dB should beat peak {:.2} dB at 20 Hz",
        at(20.0),
        at2(20.0)
    );
}

#[test]
fn ten_band_player_eq_can_follow_a_smooth_tilt() {
    let grid = log_grid(20.0, 20_000.0, 48);
    // A gentle +1 dB/octave tilt, the kind of thing 10 bands can actually do.
    let desired: Vec<f32> = grid.iter().map(|f| 1.0 * (f / 1000.0).log2()).collect();
    let spec = player_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 31.0, 16_000.0);
    assert!(
        res.rms_error_after < 1.0,
        "10 bands left {:.2} dB of error on a smooth tilt",
        res.rms_error_after
    );
    assert_eq!(res.settings.len(), 10);
    // Values land on the 0.1 dB step this kind of EQ uses.
    for s in &res.settings {
        let steps = s.gain_db / 0.1;
        assert!(
            (steps - steps.round()).abs() < 1e-3,
            "band {} = {} dB is not a multiple of 0.1",
            s.index,
            s.gain_db
        );
        assert!(s.gain_db.abs() <= 20.0 + 1e-6);
    }
}

#[test]
fn custom_layout_generates_and_caps_at_32_bands() {
    // Asking for more than the maximum is clamped, not rejected.
    let c = crate::dsp::device_eq::generate_centers(50, 20.0, 20_000.0);
    assert_eq!(c.len(), crate::dsp::device_eq::MAX_BANDS);
    assert!(c.windows(2).all(|w| w[1] > w[0]), "must be ascending");

    let c8 = crate::dsp::device_eq::generate_centers(8, 40.0, 16_000.0);
    assert_eq!(c8.len(), 8);
    assert!((c8[0] - 40.0).abs() < 1.0);
    assert!((c8[7] - 16_000.0).abs() < 100.0);

    let mut spec = DeviceSpec {
        layout: BandLayout::Custom,
        custom_centers: crate::dsp::device_eq::generate_centers(32, 20.0, 20_000.0),
        ..DeviceSpec::default()
    };
    assert_eq!(spec.centers().len(), 32);
    // Out-of-order or junk entries are cleaned up rather than breaking the solve.
    spec.custom_centers = vec![1000.0, 100.0, -5.0, 10_000.0];
    let c = spec.centers();
    assert_eq!(c, vec![100.0, 1000.0, 10_000.0]);
}

#[test]
fn custom_layout_solves_like_a_standard_one() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let broken = vec![FilterSpec::peaking(250.0, 6.0, 2.0)];
    let desired: Vec<f32> = biquad::total_response_db(&broken, &grid, SR)
        .iter()
        .map(|v| -v)
        .collect();
    let spec = DeviceSpec {
        name: "Custom20".into(),
        kind: DeviceKind::Graphic,
        layout: BandLayout::Custom,
        custom_centers: crate::dsp::device_eq::generate_centers(20, 25.0, 18_000.0),
        shelf_ends: false,
        max_boost_db: 12.0,
        max_cut_db: 12.0,
        step_db: 0.5,
        ..DeviceSpec::default()
    };
    let res = fit_graphic(&grid, &desired, &spec, SR, 30.0, 16_000.0);
    assert_eq!(res.settings.len(), 20);
    assert!(
        res.rms_error_after < res.rms_error_before * 0.5,
        "{:.2} -> {:.2} dB",
        res.rms_error_before,
        res.rms_error_after
    );
    // The band nearest 250 Hz should carry most of the cut.
    let worst = res
        .settings
        .iter()
        .min_by(|a, b| {
            (a.gain_db)
                .partial_cmp(&b.gain_db)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap();
    assert!(
        (worst.freq_hz / 250.0).log2().abs() < 0.6,
        "the biggest cut landed at {} Hz, not near 250 Hz",
        worst.freq_hz
    );
}

#[test]
fn band_width_follows_the_actual_spacing() {
    let third = DeviceSpec::preset_rack_31();
    assert!((third.band_width_octaves() - 1.0 / 3.0).abs() < 0.03);
    let ten = DeviceSpec::preset_player_10();
    assert!((ten.band_width_octaves() - 1.0).abs() < 0.05);
}

/// A shelved layout must survive the round trip into the app's own EQ preview.
#[test]
fn player_settings_preview_matches_the_prediction() {
    let grid = log_grid(20.0, 20_000.0, 48);
    let desired: Vec<f32> = grid
        .iter()
        .map(|f| if *f < 100.0 { 4.0 } else { -1.0 })
        .collect();
    let spec = player_spec();
    let res = fit_graphic(&grid, &desired, &spec, SR, 31.0, 16_000.0);
    let filters = res.to_filter_specs(&spec);
    assert!(filters
        .iter()
        .any(|f| matches!(f.kind, FilterKind::LowShelf)));
    let replay = biquad::total_response_db(&filters, &grid, SR);
    let mut worst = 0.0f32;
    for i in 0..grid.len() {
        if grid[i] >= 25.0 && grid[i] <= 18_000.0 {
            worst = worst.max((replay[i] - res.achieved[i]).abs());
        }
    }
    assert!(worst < 0.05, "preview differs by {worst} dB");
}

/// Guard against the smoothing going quadratic again. On a linear FFT axis a
/// 1/6-octave window near 20 kHz covers hundreds of bins, and the live analyser
/// calls this on every new block.
#[test]
fn smoothing_a_full_spectrum_is_fast() {
    let n = 8193; // bins of a 16384-point FFT
    let freqs: Vec<f32> = (0..n).map(|k| k as f32 * SR / 16384.0).collect();
    let db: Vec<f32> = (0..n).map(|k| ((k % 37) as f32) * 0.3 - 60.0).collect();

    let t0 = std::time::Instant::now();
    let rounds = 50;
    let mut sink = 0.0f32;
    for _ in 0..rounds {
        let s = smooth_octave(&freqs, &db, 6.0);
        sink += s[n / 2];
    }
    let per_call = t0.elapsed().as_secs_f64() / rounds as f64;
    assert!(sink.is_finite());
    assert!(
        per_call < 0.02,
        "smoothing took {:.1} ms per call, far too slow to run live",
        per_call * 1000.0
    );
    println!("smooth_octave: {:.2} ms per full spectrum", per_call * 1000.0);
}
