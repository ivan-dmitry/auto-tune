//! Tab 3 — run a log-sweep measurement and inspect the result.

use super::{plots, style, widgets};
use crate::app::{AudioTuneApp, MeasureState};
use crate::audio::engine::Route;
use crate::model::trace_color;

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum View {
    Magnitude,
    Phase,
    Impulse,
    Decay,
    Distortion,
}

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::SidePanel::left("meas_left")
        .resizable(true)
        .default_width(330.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| controls(app, ui));
        });
    egui::SidePanel::right("meas_right")
        .resizable(true)
        .default_width(300.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| list(app, ui));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| graphs(app, ui));
}

fn controls(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Sweep settings");
    egui::Grid::new("sweepgrid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Range");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.sweep_cfg.f_start)
                        .range(5.0..=500.0)
                        .suffix(" Hz"),
                );
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut app.sweep_cfg.f_end)
                        .range(1000.0..=24_000.0)
                        .suffix(" Hz"),
                );
            });
            ui.end_row();

            ui.label("Length");
            ui.add(
                egui::Slider::new(&mut app.sweep_cfg.duration_s, 1.0..=30.0)
                    .suffix(" s")
                    .step_by(0.5),
            );
            ui.end_row();

            ui.label("Decay tail");
            ui.add(
                egui::Slider::new(&mut app.sweep_cfg.tail_s, 0.2..=6.0)
                    .suffix(" s")
                    .step_by(0.1),
            );
            ui.end_row();

            ui.label("Output level");
            ui.add(
                egui::Slider::new(&mut app.sweep_cfg.level_db, -40.0..=-1.0)
                    .suffix(" dBFS")
                    .step_by(0.5),
            );
            ui.end_row();

            ui.label("Passes");
            ui.add(egui::Slider::new(&mut app.sweep_cfg.repeats, 1..=8));
            ui.end_row();
        });
    widgets::hint(
        ui,
        "A longer sweep and more passes push the noise floor down. Every doubling of the \
         pass count buys about 3 dB of signal-to-noise.",
    );

    ui.add_space(8.0);
    widgets::section(ui, "Run");
    let busy = app.job.is_some();
    let ready = app.engine.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                ready && !busy,
                egui::Button::new(format!("Measure - {}", app.mode.label()))
                    .min_size(egui::vec2(180.0, 28.0)),
            )
            .on_hover_text(app.mode.hint())
            .clicked()
        {
            let routes = app.routes_for_mode();
            app.start_measurement(routes);
        }
        if ui.add_enabled(busy, egui::Button::new("Cancel")).clicked() {
            app.cancel_measurement();
        }
    });
    ui.horizontal(|ui| {
        for (r, name) in [
            (Route::Both, "L+R"),
            (Route::Left, "Left only"),
            (Route::Right, "Right only"),
        ] {
            if ui
                .add_enabled(ready && !busy, egui::Button::new(name))
                .clicked()
            {
                app.start_measurement(vec![(r, name.to_string())]);
            }
        }
    });
    if busy {
        let txt = match &app.mstate {
            MeasureState::Capturing => "Capturing...",
            MeasureState::Countdown { .. } => "Getting ready...",
            MeasureState::Gap { .. } => "Pausing...",
            MeasureState::Idle => "",
        };
        ui.colored_label(style::WARN, txt);
        ui.add(egui::ProgressBar::new(app.recorder.progress()).show_percentage());
    }

    ui.add_space(8.0);
    widgets::section(ui, "Impulse response window");
    widgets::hint(
        ui,
        "A long window keeps the room's reflections, which is what you actually hear at the \
         listening seat. A short window throws them away and shows the speaker alone.",
    );
    egui::Grid::new("irwin")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Before peak");
            ui.add(egui::Slider::new(&mut app.ir_window.pre_ms, 0.5..=20.0).suffix(" ms"));
            ui.end_row();
            ui.label("After peak");
            ui.add(
                egui::Slider::new(&mut app.ir_window.post_ms, 2.0..=1000.0)
                    .logarithmic(true)
                    .suffix(" ms"),
            );
            ui.end_row();
            ui.label("Tail taper");
            ui.add(egui::Slider::new(&mut app.ir_window.taper, 0.0..=0.8));
            ui.end_row();
        });
    ui.horizontal(|ui| {
        if ui.button("Room (300 ms)").clicked() {
            app.ir_window = crate::dsp::measurement::IrWindow::default();
        }
        if ui.button("Gated (6 ms)").clicked() {
            app.ir_window = crate::dsp::measurement::IrWindow::gated(6.0);
        }
        if ui.button("Headphone (80 ms)").clicked() {
            app.ir_window = crate::dsp::measurement::IrWindow::gated(80.0);
        }
    });
    widgets::hint(
        ui,
        &format!(
            "Frequency resolution: {:.1} Hz",
            app.ir_window.resolution_hz()
        ),
    );
    if ui.button("Recompute every measurement").clicked() {
        recompute_all(app);
    }
}

fn recompute_all(app: &mut AudioTuneApp) {
    let win = app.ir_window;
    let cal = app.mic_cal.clone();
    let mut fft = crate::dsp::fft::Fft::new();
    for m in app.measurements.iter_mut() {
        let mut fr = crate::dsp::measurement::ir_to_fr(&m.ir, &win, &mut fft);
        cal.apply(&fr.freqs, &mut fr.mag_db);
        m.fr = fr;
    }
    app.logln("Responses recomputed with the new window.");
}

fn list(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Measurements");
    if app.measurements.is_empty() {
        widgets::hint(ui, "Nothing measured yet. Press Measure on the left.");
    }
    let mut remove: Option<usize> = None;
    let mut select: Option<usize> = None;
    for i in 0..app.measurements.len() {
        let (name, ts, ch, visible, color_idx) = {
            let m = &app.measurements[i];
            (
                m.name.clone(),
                m.timestamp.clone(),
                m.channel.clone(),
                m.visible,
                m.color_idx,
            )
        };
        let selected = app.selected == Some(i);
        egui::Frame::none()
            .fill(if selected {
                egui::Color32::from_rgb(38, 44, 52)
            } else {
                egui::Color32::TRANSPARENT
            })
            .inner_margin(egui::Margin::symmetric(6.0, 4.0))
            .rounding(4.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut v = visible;
                    if ui.checkbox(&mut v, "").changed() {
                        app.measurements[i].visible = v;
                    }
                    ui.colored_label(trace_color(color_idx), "---");
                    if ui.selectable_label(selected, &name).clicked() {
                        select = Some(i);
                    }
                    if ui.small_button("x").clicked() {
                        remove = Some(i);
                    }
                });
                ui.label(
                    egui::RichText::new(format!("{ch} - {ts}"))
                        .small()
                        .color(style::MUTED),
                );
            });
    }
    if let Some(i) = select {
        app.selected = Some(i);
    }
    if let Some(i) = remove {
        app.measurements.remove(i);
        if app.measurements.is_empty() {
            app.selected = None;
        } else {
            app.selected = Some(app.selected.unwrap_or(0).min(app.measurements.len() - 1));
        }
    }

    ui.add_space(8.0);
    if let Some(i) = app.selected {
        if let Some(m) = app.measurements.get(i) {
            widgets::section(ui, "Details");
            let dist = m.ir.distance_m(app.system_latency_ms, 343.0);
            egui::Grid::new("detail")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Delay");
                    ui.monospace(format!("{:.2} ms", m.ir.delay_ms()));
                    ui.end_row();
                    ui.label("Distance");
                    ui.monospace(format!("{dist:.2} m"));
                    ui.end_row();
                    ui.label("Input peak");
                    ui.monospace(format!("{:.1} dBFS", m.peak_dbfs));
                    ui.end_row();
                    ui.label("SNR");
                    ui.monospace(format!("{:.0} dB", m.snr_db));
                    ui.end_row();
                    ui.label("THD");
                    ui.monospace(format!("{:.2} %", m.distortion.thd_percent));
                    ui.end_row();
                    if let Some(d) = &m.decay {
                        ui.label("RT60 (T30)");
                        ui.monospace(format!("{:.3} s", d.t30));
                        ui.end_row();
                        ui.label("T20 / EDT");
                        ui.monospace(format!("{:.3} / {:.3} s", d.t20, d.edt));
                        ui.end_row();
                    }
                });
            if m.clipped {
                ui.colored_label(style::BAD, "Capture clipped - this result cannot be trusted");
            }
            if m.snr_db < 30.0 {
                ui.colored_label(
                    style::WARN,
                    "Low SNR: raise the level, add passes, or reduce background noise",
                );
            }
        }
    }

    // Left / right comparison once both channels have been measured.
    if let Some((li, ri)) = find_lr(app) {
        ui.add_space(8.0);
        widgets::section(ui, "Left vs right");
        let l = &app.measurements[li];
        let r = &app.measurements[ri];
        let dl = l.midband_db() - r.midband_db();
        let dt = l.ir.delay_ms() - r.ir.delay_ms();
        ui.monospace(format!("Level difference: {dl:+.2} dB"));
        ui.monospace(format!(
            "Timing difference: {dt:+.2} ms ({:.1} cm)",
            dt.abs() * 34.3
        ));
        if dl.abs() > 1.0 {
            ui.colored_label(style::WARN, "Rebalance the two channels before EQ");
        }
        if dt.abs() > 0.5 {
            ui.colored_label(
                style::WARN,
                "The speakers are not equidistant - move one or add delay",
            );
        }
    }

    ui.add_space(8.0);
    if ui.button("Export response (.frd)").clicked() {
        export_frd(app);
    }
    if ui.button("Export impulse response (.wav)").clicked() {
        export_ir(app);
    }
}

fn find_lr(app: &AudioTuneApp) -> Option<(usize, usize)> {
    let l = app
        .measurements
        .iter()
        .rposition(|m| m.channel.to_lowercase().contains("left") && m.visible)?;
    let r = app
        .measurements
        .iter()
        .rposition(|m| m.channel.to_lowercase().contains("right") && m.visible)?;
    Some((l, r))
}

fn export_frd(app: &mut AudioTuneApp) {
    let Some(i) = app.selected else { return };
    let Some(m) = app.measurements.get(i) else {
        return;
    };
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("FRD", &["frd", "txt"])
        .set_file_name(format!("{}.frd", sanitize(&m.name)))
        .save_file()
    {
        let grid = crate::dsp::octave::log_grid(10.0, 24_000.0, 48);
        let db = m.fr.smoothed_on_grid(&grid, 12.0);
        match crate::audio::fileio::save_frd(&p, &grid, &db, &format!("AudioTune {}", m.name)) {
            Ok(_) => app.logln(format!("Exported {}", p.display())),
            Err(e) => app.logln(format!("Export failed: {e}")),
        }
    }
}

fn export_ir(app: &mut AudioTuneApp) {
    let Some(i) = app.selected else { return };
    let Some(m) = app.measurements.get(i) else {
        return;
    };
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("WAV", &["wav"])
        .set_file_name(format!("{}_ir.wav", sanitize(&m.name)))
        .save_file()
    {
        // Trim around the peak to keep the file small.
        let start = m.ir.peak_idx.saturating_sub((0.01 * m.sr) as usize);
        let end = (m.ir.peak_idx + (1.0 * m.sr) as usize).min(m.ir.data.len());
        let seg = &m.ir.data[start..end];
        match crate::audio::fileio::save_wav_mono(&p, seg, m.sr as u32) {
            Ok(_) => app.logln(format!("Exported {}", p.display())),
            Err(e) => app.logln(format!("Export failed: {e}")),
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn graphs(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for (v, name) in [
            (View::Magnitude, "Magnitude"),
            (View::Phase, "Phase"),
            (View::Impulse, "Impulse"),
            (View::Decay, "Decay"),
            (View::Distortion, "Distortion"),
        ] {
            ui.selectable_value(&mut app.measure_view, v, name);
        }
        ui.separator();
        if app.measure_view == View::Magnitude {
            ui.label("Smoothing:");
            egui::ComboBox::from_id_salt("mag_smooth")
                .selected_text(if app.meas_smoothing <= 0.0 {
                    "None".into()
                } else {
                    format!("1/{:.0} oct", app.meas_smoothing)
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut app.meas_smoothing, 0.0, "None");
                    for f in [48.0f32, 24.0, 12.0, 6.0, 3.0, 1.0] {
                        ui.selectable_value(&mut app.meas_smoothing, f, format!("1/{f:.0} oct"));
                    }
                });
            ui.checkbox(&mut app.normalize_traces, "Level to 0 dB at midband");
        }
    });
    ui.separator();

    let h = ui.available_height() - 6.0;
    match app.measure_view {
        View::Magnitude => magnitude(app, ui, h),
        View::Phase => phase(app, ui, h),
        View::Impulse => impulse(app, ui, h),
        View::Decay => decay(app, ui, h),
        View::Distortion => distortion(app, ui, h),
    }
}

/// Key covering everything that changes the drawn magnitude traces.
fn mag_key(app: &AudioTuneApp) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    app.meas_smoothing.to_bits().hash(&mut h);
    app.normalize_traces.hash(&mut h);
    app.measurements.len().hash(&mut h);
    for m in &app.measurements {
        m.visible.hash(&mut h);
        m.name.hash(&mut h);
        // The response itself changes when the IR window is recomputed.
        m.fr.mag_db.len().hash(&mut h);
        m.fr.mag_db.first().map(|v| v.to_bits()).hash(&mut h);
        m.fr.mag_db.last().map(|v| v.to_bits()).hash(&mut h);
    }
    h.finish()
}

fn magnitude(app: &mut AudioTuneApp, ui: &mut egui::Ui, h: f32) {
    let key = mag_key(app);
    if app.mag_cache.key != key || app.mag_cache.grid.is_empty() {
        let grid = crate::dsp::octave::log_grid(10.0, 24_000.0, 48);
        let curves = app
            .measurements
            .iter()
            .filter(|m| m.visible)
            .map(|m| {
                let mut db = m.fr.smoothed_on_grid(&grid, app.meas_smoothing);
                if app.normalize_traces {
                    let mid = crate::app::mean_band(&grid, &db, 200.0, 2000.0);
                    for v in db.iter_mut() {
                        *v -= mid;
                    }
                }
                (m.color_idx, m.name.clone(), db)
            })
            .collect();
        app.mag_cache = crate::app::MagCache { key, grid, curves };
    }

    let cache = &app.mag_cache;
    let target = app.target.on_grid(&cache.grid);
    plots::FreqPlot::new("mag")
        .height(h)
        .y_range(-40.0, 15.0)
        .y_label("dB")
        .link_x("freqaxis")
        .show(ui, |pui| {
            for (color, name, db) in &cache.curves {
                plots::line(pui, &cache.grid, db, name, trace_color(*color), 1.8);
            }
            plots::dashed_line(pui, &cache.grid, &target, "Target", style::TARGET);
        });
}

fn phase(app: &AudioTuneApp, ui: &mut egui::Ui, h: f32) {
    plots::FreqPlot::new("phase")
        .height(h)
        .y_range(-180.0, 180.0)
        .y_label("degrees")
        .link_x("freqaxis")
        .show(ui, |pui| {
            for m in app.measurements.iter().filter(|m| m.visible) {
                plots::line(
                    pui,
                    &m.fr.freqs,
                    &m.fr.phase_deg,
                    &m.name,
                    trace_color(m.color_idx),
                    1.2,
                );
            }
        });
}

fn impulse(app: &AudioTuneApp, ui: &mut egui::Ui, h: f32) {
    egui_plot::Plot::new("ir")
        .height(h)
        .x_axis_label("ms around the peak")
        .y_axis_label("amplitude")
        .legend(egui_plot::Legend::default())
        .show(ui, |pui| {
            for m in app.measurements.iter().filter(|m| m.visible) {
                let sr = m.sr;
                let pre = (0.005 * sr) as usize;
                let post = (0.05 * sr) as usize;
                let s = m.ir.peak_idx.saturating_sub(pre);
                let e = (m.ir.peak_idx + post).min(m.ir.data.len());
                let peak = m.ir.peak_value().abs().max(1e-12);
                let pts: Vec<[f64; 2]> = (s..e)
                    .map(|i| {
                        [
                            (i as f64 - m.ir.peak_idx as f64) * 1000.0 / sr as f64,
                            (m.ir.data[i] / peak) as f64,
                        ]
                    })
                    .collect();
                pui.line(
                    egui_plot::Line::new(egui_plot::PlotPoints::from(pts))
                        .name(&m.name)
                        .color(trace_color(m.color_idx)),
                );
            }
        });
}

fn decay(app: &AudioTuneApp, ui: &mut egui::Ui, h: f32) {
    egui_plot::Plot::new("decay")
        .height(h)
        .x_axis_label("seconds")
        .y_axis_label("dB")
        .legend(egui_plot::Legend::default())
        .include_y(-65.0)
        .include_y(2.0)
        .show(ui, |pui| {
            for m in app.measurements.iter().filter(|m| m.visible) {
                let sr = m.sr;
                let tail = &m.ir.data[m.ir.peak_idx.min(m.ir.data.len().saturating_sub(1))..];
                let n = tail.len().min((2.0 * sr) as usize);
                if n < 16 {
                    continue;
                }
                let mut acc = 0.0f64;
                let mut curve = vec![0.0f64; n];
                for i in (0..n).rev() {
                    acc += (tail[i] as f64) * (tail[i] as f64);
                    curve[i] = acc;
                }
                let total = curve[0].max(1e-30);
                let step = (n / 2000).max(1);
                let pts: Vec<[f64; 2]> = (0..n)
                    .step_by(step)
                    .map(|i| {
                        [
                            i as f64 / sr as f64,
                            10.0 * (curve[i] / total).max(1e-8).log10(),
                        ]
                    })
                    .collect();
                pui.line(
                    egui_plot::Line::new(egui_plot::PlotPoints::from(pts))
                        .name(&m.name)
                        .color(trace_color(m.color_idx)),
                );
            }
        });
}

fn distortion(app: &AudioTuneApp, ui: &mut egui::Ui, _h: f32) {
    widgets::section(ui, "Harmonic levels relative to the fundamental");
    widgets::hint(
        ui,
        "Below -40 dB (about 1 %) is normal for a domestic speaker at a sensible level. If it \
         shoots up when you turn it up, the speaker is being pushed past its limit.",
    );
    egui::Grid::new("thd")
        .num_columns(7)
        .striped(true)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.label("Measurement");
            for n in 2..=6 {
                ui.label(format!("H{n}"));
            }
            ui.label("THD");
            ui.end_row();
            for m in app.measurements.iter().filter(|m| m.visible) {
                ui.colored_label(trace_color(m.color_idx), &m.name);
                for k in 0..5 {
                    let v = m.distortion.harmonics_db.get(k).copied().unwrap_or(-120.0);
                    let c = if v > -30.0 {
                        style::BAD
                    } else if v > -45.0 {
                        style::WARN
                    } else {
                        style::OK
                    };
                    ui.colored_label(c, format!("{v:.0} dB"));
                }
                ui.monospace(format!("{:.2} %", m.distortion.thd_percent));
                ui.end_row();
            }
        });
}
