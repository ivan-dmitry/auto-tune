//! Tab 2 — live spectrum analysis, with the test signal generator alongside.

use super::{plots, style, widgets};
use crate::app::{AudioTuneApp, RtaView};
use crate::audio::engine::Route;
use crate::dsp::calibration::Weighting;
use crate::dsp::generator::SignalKind;
use crate::dsp::octave;
use crate::dsp::spectrum::Averaging;
use crate::dsp::window::WindowKind;

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::SidePanel::left("rta_left")
        .resizable(true)
        .default_width(320.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| controls(app, ui));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| plot(app, ui));
}

fn controls(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Signal generator");
    egui::Grid::new("gen")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Signal");
            egui::ComboBox::from_id_salt("sig")
                .selected_text(app.signal.label())
                .show_ui(ui, |ui| {
                    for s in [
                        SignalKind::PinkNoise,
                        SignalKind::WhiteNoise,
                        SignalKind::Sine,
                        SignalKind::PinkBand,
                        SignalKind::LogSweep,
                    ] {
                        ui.selectable_value(&mut app.signal, s, s.label());
                    }
                });
            ui.end_row();

            ui.label("Level");
            ui.add(
                egui::Slider::new(&mut app.signal_level_db, -60.0..=0.0)
                    .suffix(" dBFS")
                    .step_by(0.5),
            );
            ui.end_row();

            if app.signal == SignalKind::Sine {
                ui.label("Frequency");
                widgets::freq_drag(ui, &mut app.signal_freq, 10.0..=22_000.0);
                ui.end_row();
            }
            if app.signal == SignalKind::PinkBand {
                ui.label("Band centre");
                widgets::freq_drag(ui, &mut app.band_center, 20.0..=18_000.0);
                ui.end_row();
            }

            ui.label("Play to");
            egui::ComboBox::from_id_salt("route")
                .selected_text(app.route.label())
                .show_ui(ui, |ui| {
                    for r in [Route::Both, Route::Left, Route::Right] {
                        ui.selectable_value(&mut app.route, r, r.label());
                    }
                    for c in 0..app.out_channels.min(8) {
                        let r = Route::Channel(c as usize);
                        ui.selectable_value(&mut app.route, r, r.label());
                    }
                });
            ui.end_row();
        });

    ui.horizontal(|ui| {
        let on = app.test_playing;
        if ui
            .add_enabled(
                app.engine.is_some(),
                egui::Button::new(if on { "Stop" } else { "Play" })
                    .min_size(egui::vec2(90.0, 0.0)),
            )
            .clicked()
        {
            if on {
                app.stop_test_signal();
            } else {
                app.start_test_signal();
            }
        }
        if ui
            .button("Apply signal change")
            .on_hover_text("Switch to the new selection without stopping first")
            .clicked()
            && app.test_playing
        {
            app.start_test_signal();
        }
    });

    ui.add_space(8.0);
    widgets::section(ui, "Analysis");
    egui::Grid::new("ana")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("FFT size");
            egui::ComboBox::from_id_salt("fftsize")
                .selected_text(format!("{}", app.fft_size))
                .show_ui(ui, |ui| {
                    for n in [2048usize, 4096, 8192, 16384, 32768, 65536] {
                        ui.selectable_value(&mut app.fft_size, n, n.to_string());
                    }
                });
            ui.end_row();

            ui.label("Window");
            egui::ComboBox::from_id_salt("win")
                .selected_text(app.window.label())
                .show_ui(ui, |ui| {
                    for w in WindowKind::ALL {
                        ui.selectable_value(&mut app.window, w, w.label());
                    }
                });
            ui.end_row();

            ui.label("Averaging");
            ui.vertical(|ui| {
                let mut kind = match app.averaging {
                    Averaging::None => 0,
                    Averaging::Exponential { .. } => 1,
                    Averaging::Infinite => 2,
                    Averaging::Moving { .. } => 3,
                };
                egui::ComboBox::from_id_salt("avg")
                    .selected_text(app.averaging.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut kind, 0, "Instant");
                        ui.selectable_value(&mut kind, 1, "Exponential");
                        ui.selectable_value(&mut kind, 2, "Infinite");
                        ui.selectable_value(&mut kind, 3, "Moving (N blocks)");
                    });
                app.averaging = match kind {
                    0 => Averaging::None,
                    2 => Averaging::Infinite,
                    3 => Averaging::Moving { blocks: 32 },
                    _ => Averaging::Exponential { tau: app.avg_tau },
                };
                if let Averaging::Exponential { .. } = app.averaging {
                    if ui
                        .add(
                            egui::Slider::new(&mut app.avg_tau, 0.05..=10.0)
                                .logarithmic(true)
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        app.averaging = Averaging::Exponential { tau: app.avg_tau };
                    }
                }
            });
            ui.end_row();

            ui.label("View");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut app.rta_view, RtaView::Spectrum, "Spectrum");
                ui.selectable_value(&mut app.rta_view, RtaView::Bands, "Octave bands");
            });
            ui.end_row();

            if app.rta_view == RtaView::Bands {
                ui.label("Band width");
                egui::ComboBox::from_id_salt("bandfrac")
                    .selected_text(format!("1/{}", app.band_frac))
                    .show_ui(ui, |ui| {
                        for f in [1u32, 3, 6, 12, 24] {
                            ui.selectable_value(&mut app.band_frac, f, format!("1/{f} octave"));
                        }
                    });
                ui.end_row();
            } else {
                ui.label("Smoothing");
                ui.horizontal(|ui| {
                    let mut smooth = app.smoothing_frac;
                    egui::ComboBox::from_id_salt("smooth")
                        .selected_text(if smooth <= 0.0 {
                            "None".to_string()
                        } else {
                            format!("1/{smooth:.0} oct")
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut smooth, 0.0, "None");
                            for f in [48.0f32, 24.0, 12.0, 6.0, 3.0] {
                                ui.selectable_value(&mut smooth, f, format!("1/{f:.0} oct"));
                            }
                        });
                    app.smoothing_frac = smooth;
                });
                ui.end_row();
            }
        });

    ui.horizontal(|ui| {
        ui.label("Overlap:");
        for (v, name) in [(0.0f32, "0 %"), (0.5, "50 %"), (0.75, "75 %"), (0.875, "87 %")] {
            ui.selectable_value(&mut app.overlap, v, name);
        }
    });
    widgets::hint(
        ui,
        "More overlap means the display updates more often for the same FFT size. It costs \
         a little CPU and does not change the resolution.",
    );
    ui.horizontal(|ui| {
        ui.label("Weighting:");
        for w in [Weighting::Z, Weighting::A, Weighting::C] {
            ui.selectable_value(&mut app.weighting, w, w.label());
        }
    });
    widgets::hint(
        ui,
        "A weighting is for reporting noise levels to standard. Leave it on Z while tuning EQ.",
    );
    ui.checkbox(&mut app.show_peak_hold, "Peak hold");
    if app.rta_view == RtaView::Spectrum {
        ui.checkbox(
            &mut app.pink_compensate,
            "Pink compensation (+3 dB/oct) - pink noise reads flat",
        );
    }
    ui.horizontal(|ui| {
        if ui.button("Clear average").clicked() {
            app.analyzer.reset();
        }
        if ui.button("Clear peaks").clicked() {
            app.analyzer.reset_peak();
        }
    });

    ui.add_space(6.0);
    widgets::hint(
        ui,
        &format!(
            "Resolution: {:.2} Hz per bin - {} blocks processed",
            app.analyzer.resolution_hz(),
            app.analyzer.blocks
        ),
    );

    ui.add_space(8.0);
    widgets::section(ui, "Against target");
    ui.checkbox(&mut app.rta_show_target, "Draw the target curve on the plot");
    widgets::hint(
        ui,
        "Play pink noise, turn pink compensation on, then adjust the EQ until the measured \
         trace hugs the target. This is the quickest way to tune band by band by eye.",
    );
}

/// One number that changes whenever anything affecting the drawn curve changes.
fn view_key(app: &AudioTuneApp) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.fft_size.hash(&mut h);
    (app.window as u8).hash(&mut h);
    (app.rta_view == RtaView::Bands).hash(&mut h);
    app.band_frac.hash(&mut h);
    app.smoothing_frac.to_bits().hash(&mut h);
    app.pink_compensate.hash(&mut h);
    app.show_peak_hold.hash(&mut h);
    (app.weighting as u8).hash(&mut h);
    app.spl_cal.enabled.hash(&mut h);
    app.spl_cal.spl_at_full_scale.to_bits().hash(&mut h);
    (app.analyzer.sr as u32).hash(&mut h);
    h.finish()
}

fn plot(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    let key = view_key(app);
    let blocks = app.analyzer.blocks;
    if app.rta_cache.is_stale(key, blocks) {
        rebuild_cache(app, key, blocks);
    }

    let y_center = app.rta_cache.y_center;
    let (y_lo, y_hi) = (y_center - 50.0, y_center + 25.0);

    // Target drawn on top, lined up with the measured midband level.
    let target: Option<(Vec<f32>, Vec<f32>)> = if app.rta_show_target {
        let grid = octave::log_grid(20.0, 20_000.0, 24);
        let t = app.target.on_grid(&grid);
        let mid_meas =
            crate::app::mean_band(&app.rta_cache.freqs, &app.rta_cache.db, 200.0, 2000.0);
        let mid_t = crate::app::mean_band(&grid, &t, 200.0, 2000.0);
        let shifted: Vec<f32> = t.iter().map(|v| v - mid_t + mid_meas).collect();
        Some((grid, shifted))
    } else {
        None
    };

    let cache = &app.rta_cache;
    plots::FreqPlot::new("rta")
        .height(ui.available_height() - 6.0)
        .y_range(y_lo as f64, y_hi as f64)
        .y_label(cache.y_label.clone())
        .link_x("freqaxis")
        .show(ui, |pui| {
            if let Some((g, t)) = &target {
                plots::dashed_line(pui, g, t, "Target", style::TARGET);
            }
            if let Some(p) = &cache.peak {
                plots::line(pui, &cache.freqs, p, "Peak hold", style::MUTED, 1.0);
            }
            plots::line(pui, &cache.freqs, &cache.db, "Live", style::PREDICT, 1.8);
            if cache.is_bands {
                let pts: Vec<[f64; 2]> = cache
                    .freqs
                    .iter()
                    .zip(cache.db.iter())
                    .filter(|(f, _)| **f >= 16.0 && **f <= 20_000.0)
                    .map(|(f, d)| [(*f as f64).log10(), *d as f64])
                    .collect();
                pui.points(
                    egui_plot::Points::new(egui_plot::PlotPoints::from(pts))
                        .radius(2.5_f32)
                        .color(style::PREDICT)
                        .name("Bands"),
                );
            }
        });
}

fn rebuild_cache(app: &mut AudioTuneApp, key: u64, blocks: u64) {
    let sr = app.analyzer.sr;
    let freqs = app.analyzer.freqs();
    let amp = app.analyzer.amplitude();

    let (curve_f, curve_db, y_label) = match app.rta_view {
        RtaView::Bands => {
            let bl = octave::band_levels(&amp, sr, app.fft_size, app.band_frac, 16.0, 20_000.0);
            (bl.centers, bl.db, format!("dBFS per 1/{} band", app.band_frac))
        }
        RtaView::Spectrum => {
            let mut db: Vec<f32> = amp.iter().map(|a| crate::dsp::lin_to_db(*a)).collect();
            if app.pink_compensate {
                for (f, d) in freqs.iter().zip(db.iter_mut()) {
                    if *f > 1.0 {
                        *d += 10.0 * (*f / 1000.0).log10();
                    }
                }
            }
            if app.smoothing_frac > 0.0 {
                db = octave::smooth_octave(&freqs, &db, app.smoothing_frac);
            }
            (freqs.clone(), db, "dBFS".to_string())
        }
    };

    // Apply A or C weighting if the user asked for it.
    let (curve_db, y_label) = if app.weighting != Weighting::Z {
        let w = app.weighting;
        (
            curve_f
                .iter()
                .zip(curve_db.iter())
                .map(|(f, d)| d + w.db_at(*f))
                .collect::<Vec<f32>>(),
            format!("{y_label} ({} weighted)", w.label()),
        )
    } else {
        (curve_db, y_label)
    };

    // Convert to dB SPL once calibrated.
    let (curve_db, y_label) = if app.spl_cal.enabled {
        (
            curve_db.iter().map(|d| app.spl_cal.to_spl(*d)).collect(),
            format!("{y_label} -> dB SPL"),
        )
    } else {
        (curve_db, y_label)
    };

    let y_center = median(&curve_db);

    let peak_db: Option<Vec<f32>> = if app.show_peak_hold {
        let pa = app.analyzer.peak_amplitude();
        match app.rta_view {
            RtaView::Spectrum => {
                let mut d: Vec<f32> = pa.iter().map(|a| crate::dsp::lin_to_db(*a)).collect();
                if app.pink_compensate {
                    for (f, v) in freqs.iter().zip(d.iter_mut()) {
                        if *f > 1.0 {
                            *v += 10.0 * (*f / 1000.0).log10();
                        }
                    }
                }
                Some(finish_curve(app, &freqs, d))
            }
            RtaView::Bands => {
                let bl = octave::band_levels(&pa, sr, app.fft_size, app.band_frac, 16.0, 20_000.0);
                Some(finish_curve(app, &bl.centers, bl.db))
            }
        }
    } else {
        None
    };

    // Thin the curve here, once per new block, rather than on every repaint.
    let is_bands = app.rta_view == RtaView::Bands;
    let (f_out, db_out, peak_out) = if is_bands {
        (curve_f, curve_db, peak_db)
    } else {
        match &peak_db {
            Some(p) => {
                let (f, mut cs) = plots::decimate_multi(&curve_f, &[&curve_db, p], 1400);
                let peak = cs.pop();
                let db = cs.pop().unwrap_or_default();
                (f, db, peak)
            }
            None => {
                let (f, mut cs) = plots::decimate_multi(&curve_f, &[&curve_db], 1400);
                (f, cs.pop().unwrap_or_default(), None)
            }
        }
    };

    app.rta_cache = crate::app::RtaCache {
        key,
        blocks,
        freqs: f_out,
        db: db_out,
        peak: peak_out,
        y_label,
        y_center,
        is_bands,
    };
}

/// Apply weighting and SPL offset to a secondary curve so it matches the main one.
fn finish_curve(app: &AudioTuneApp, freqs: &[f32], mut db: Vec<f32>) -> Vec<f32> {
    if app.weighting != Weighting::Z {
        for (f, v) in freqs.iter().zip(db.iter_mut()) {
            *v += app.weighting.db_at(*f);
        }
    }
    if app.spl_cal.enabled {
        for v in db.iter_mut() {
            *v = app.spl_cal.to_spl(*v);
        }
    }
    db
}

fn median(v: &[f32]) -> f32 {
    let mut x: Vec<f32> = v
        .iter()
        .copied()
        .filter(|a| a.is_finite() && *a > -150.0)
        .collect();
    if x.is_empty() {
        return -40.0;
    }
    x.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    x[x.len() / 2]
}
