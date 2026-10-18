//! Tab 4 — choose a target, work out the ideal correction, audition it.
//!
//! This tab produces the *ideal* correction as a parametric filter set. Turning that
//! into numbers for an outboard EQ happens on tab 5.

use super::{plots, style, widgets};
use crate::app::AudioTuneApp;
use crate::dsp::biquad::{self, FilterKind, FilterSpec};
use crate::dsp::octave;
use crate::dsp::target::TargetKind;

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::SidePanel::left("eq_left")
        .resizable(true)
        .default_width(340.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| left(app, ui));
        });
    egui::TopBottomPanel::bottom("eq_bottom")
        .resizable(true)
        .default_height(270.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| filter_table(app, ui));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| graph(app, ui));
}

fn left(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Target curve");
    egui::ComboBox::from_id_salt("tkind")
        .width(280.0)
        .selected_text(app.target.kind.label())
        .show_ui(ui, |ui| {
            for k in TargetKind::ALL {
                ui.selectable_value(&mut app.target.kind, k, k.label())
                    .on_hover_text(k.hint());
            }
        });
    widgets::hint(ui, app.target.kind.hint());

    match app.target.kind {
        TargetKind::Tilt => {
            ui.add(
                egui::Slider::new(&mut app.target.tilt_db_oct, -3.0..=1.0)
                    .suffix(" dB/oct")
                    .text("slope"),
            );
        }
        TargetKind::HarmanRoom => {
            ui.add(
                egui::Slider::new(&mut app.target.tilt_db_oct, -2.0..=0.0)
                    .suffix(" dB/oct")
                    .text("upper slope"),
            );
            ui.add(
                egui::Slider::new(&mut app.target.bass_gain_db, 0.0..=10.0)
                    .suffix(" dB")
                    .text("bass lift"),
            );
            widgets::freq_drag(ui, &mut app.target.bass_corner_hz, 40.0..=300.0);
            widgets::hint(ui, "corner frequency of the bass shelf");
        }
        TargetKind::Custom => {
            if ui.button("Load target from file...").clicked() {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("Curve", &["txt", "frd", "csv"])
                    .pick_file()
                {
                    match std::fs::read_to_string(&p) {
                        Ok(t) => {
                            let pts = crate::dsp::target::TargetCurve::load_points(&t);
                            if pts.is_empty() {
                                app.logln("No points could be read.");
                            } else {
                                app.logln(format!("Loaded {} target points.", pts.len()));
                                app.target.points = pts;
                            }
                        }
                        Err(e) => app.logln(format!("Could not read the file: {e}")),
                    }
                }
            }
            widgets::hint(ui, &format!("{} points", app.target.points.len()));
        }
        _ => {}
    }
    ui.add(
        egui::Slider::new(&mut app.target.sub_rolloff_hz, 0.0..=80.0)
            .suffix(" Hz")
            .text("roll off below (0 = off)"),
    );
    widgets::hint(
        ui,
        "Set this to the lowest note the system can really reproduce, so EQ does not try to \
         force the woofer past it.",
    );

    ui.add_space(10.0);
    widgets::section(ui, "Correction settings");
    ui.horizontal(|ui| {
        if ui.button("Speaker").clicked() {
            app.fit_cfg = crate::dsp::autoeq::FitConfig::preset_speaker();
        }
        if ui.button("Room modes only").clicked() {
            app.fit_cfg = crate::dsp::autoeq::FitConfig::preset_room_modes();
        }
        if ui.button("Headphone").clicked() {
            app.fit_cfg = crate::dsp::autoeq::FitConfig::preset_headphone();
        }
    });
    egui::Grid::new("fitcfg")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Correction range");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.fit_cfg.f_min)
                        .range(10.0..=500.0)
                        .suffix(" Hz"),
                );
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut app.fit_cfg.f_max)
                        .range(1000.0..=22_000.0)
                        .suffix(" Hz"),
                );
            });
            ui.end_row();

            ui.label("Max filters");
            ui.add(egui::Slider::new(&mut app.fit_cfg.max_filters, 1..=24));
            ui.end_row();

            ui.label("Max boost");
            ui.add(egui::Slider::new(&mut app.fit_cfg.max_boost_db, 0.0..=12.0).suffix(" dB"));
            ui.end_row();

            ui.label("Max cut");
            ui.add(egui::Slider::new(&mut app.fit_cfg.max_cut_db, 0.0..=24.0).suffix(" dB"));
            ui.end_row();

            ui.label("Q range");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut app.fit_cfg.min_q)
                        .range(0.1..=5.0)
                        .speed(0.05),
                );
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut app.fit_cfg.max_q)
                        .range(1.0..=24.0)
                        .speed(0.1),
                );
            });
            ui.end_row();

            ui.label("Fit smoothing");
            egui::ComboBox::from_id_salt("fitsmooth")
                .selected_text(format!("1/{:.0} oct", app.fit_cfg.smoothing_frac))
                .show_ui(ui, |ui| {
                    for f in [3.0f32, 6.0, 12.0, 24.0, 48.0] {
                        ui.selectable_value(
                            &mut app.fit_cfg.smoothing_frac,
                            f,
                            format!("1/{f:.0} oct"),
                        );
                    }
                });
            ui.end_row();

            ui.label("How hard to correct");
            let pct = app.fit_cfg.damping * 100.0;
            ui.add(
                egui::Slider::new(&mut app.fit_cfg.damping, 0.3..=1.0).text(format!("{pct:.0} %")),
            );
            ui.end_row();
        });
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.fit_cfg.use_low_shelf, "Allow low shelf");
        ui.checkbox(&mut app.fit_cfg.use_high_shelf, "Allow high shelf");
    });

    ui.add_space(8.0);
    let has = app.selected.is_some();
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                has,
                egui::Button::new("Auto-fit correction").min_size(egui::vec2(170.0, 28.0)),
            )
            .clicked()
        {
            app.run_autofit(false);
        }
        if ui
            .add_enabled(
                !app.measurements.is_empty(),
                egui::Button::new("Fit from average"),
            )
            .on_hover_text(
                "Power-average every visible measurement - use this for one shared correction \
                 across both speakers or several seats",
            )
            .clicked()
        {
            app.run_autofit(true);
        }
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(has, egui::Button::new("Suggest low limit"))
            .clicked()
        {
            app.suggest_limits();
        }
        if ui.button("Clear all filters").clicked() {
            app.filters.clear();
            app.fit_result = None;
            app.preamp_db = 0.0;
            app.push_eq();
        }
    });

    if let Some(r) = &app.fit_result {
        ui.add_space(6.0);
        let improve = r.rms_before - r.rms_after;
        let c = if improve > 1.0 {
            style::OK
        } else if improve > 0.2 {
            style::WARN
        } else {
            style::BAD
        };
        ui.colored_label(
            c,
            format!(
                "RMS error {:.2} -> {:.2} dB (improved by {:.2} dB)",
                r.rms_before, r.rms_after, improve
            ),
        );
    }

    ui.add_space(6.0);
    if ui
        .add_enabled(
            has,
            egui::Button::new("Next: numbers for my EQ unit").min_size(egui::vec2(230.0, 26.0)),
        )
        .clicked()
    {
        app.compute_device_settings();
        app.tab = crate::app::Tab::Device;
    }

    ui.add_space(10.0);
    widgets::section(ui, "A/B listening");
    widgets::hint(
        ui,
        "This runs the correction inside the app so you can hear it before dialling anything \
         into the hardware. It does not change your EQ unit.",
    );
    ui.horizontal(|ui| {
        if ui.checkbox(&mut app.eq_enabled, "EQ on").changed() {
            app.push_eq();
        }
        ui.label(format!("preamp {:.1} dB", app.preamp_db));
        if ui.small_button("recompute").clicked() {
            app.recompute_preamp();
            app.push_eq();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Load music (.wav)...").clicked() {
            load_music(app);
        }
        if let Some(m) = &app.music {
            ui.label(format!("{} - {:.0} s", m.name, m.duration_s()));
        }
    });
    ui.horizontal(|ui| {
        let can = app.music.is_some() && app.engine.is_some();
        if ui
            .add_enabled(
                can,
                egui::Button::new(if app.music_playing {
                    "Stop music"
                } else {
                    "Play music"
                }),
            )
            .clicked()
        {
            toggle_music(app);
        }
        if ui
            .add(egui::Slider::new(&mut app.music_gain_db, -40.0..=0.0).text("dBFS"))
            .changed()
            && app.music_playing
        {
            toggle_music(app);
            toggle_music(app);
        }
    });

    ui.add_space(10.0);
    widgets::section(ui, "Export");
    if ui.button("Save the filter list as text").clicked() {
        export_text(app);
    }
    widgets::hint(
        ui,
        "The text file holds both the Equalizer APO style lines and the raw biquad \
         coefficients, for any DSP that takes them.",
    );
}

fn load_music(app: &mut AudioTuneApp) {
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("WAV", &["wav"])
        .pick_file()
    {
        match crate::audio::fileio::load_wav(&p) {
            Ok(mut f) => {
                let target_sr = app.engine.as_ref().map(|e| e.sr_out).unwrap_or(48_000.0);
                if (f.sample_rate as f32 - target_sr).abs() > 1.0 {
                    app.logln(format!(
                        "Converting sample rate {} -> {:.0} Hz...",
                        f.sample_rate, target_sr
                    ));
                    f.frames = crate::audio::fileio::resample_stereo(
                        &f.frames,
                        f.sample_rate as f32,
                        target_sr,
                    );
                    f.sample_rate = target_sr as u32;
                }
                app.logln(format!("Loaded {} ({:.1} s)", f.name, f.duration_s()));
                app.music = Some(f);
            }
            Err(e) => app.logln(format!("Could not load the file: {e}")),
        }
    }
}

fn toggle_music(app: &mut AudioTuneApp) {
    if app.music_playing {
        if let Some(e) = &app.engine {
            e.stop_playback();
        }
        app.music_playing = false;
        return;
    }
    let amp = 10f32.powf(app.music_gain_db / 20.0);
    let data = app.music.as_ref().map(|m| m.frames.clone());
    if let (Some(e), Some(d)) = (&app.engine, data) {
        e.play_stereo(std::sync::Arc::new(d), true, amp);
        app.music_playing = true;
    }
}

fn graph(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    let grid = app.eq_grid();
    let measured = app.measured_curve(&grid);
    let target = app.target.on_grid(&grid);
    let eq = biquad::total_response_db(&app.filters, &grid, app.eq_sr);

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Measured / target / EQ / predicted").strong());
        ui.separator();
        ui.checkbox(&mut app.eq_show_error, "Show remaining error");
    });

    let h = if app.eq_show_error {
        (ui.available_height() - 40.0) * 0.62
    } else {
        ui.available_height() - 12.0
    };

    plots::FreqPlot::new("eqplot")
        .height(h)
        .y_range(-25.0, 15.0)
        .y_label("dB")
        .link_x("freqaxis")
        .show(ui, |pui| {
            plots::dashed_line(pui, &grid, &target, "Target", style::TARGET);
            if let Some(m) = &measured {
                plots::line(pui, &grid, m, "Measured", style::MUTED, 1.6);
                let pred: Vec<f32> = m.iter().zip(eq.iter()).map(|(a, b)| a + b).collect();
                plots::line(pui, &grid, &pred, "Predicted after EQ", style::PREDICT, 2.2);
            }
            plots::line(pui, &grid, &eq, "EQ response", style::EQ_CURVE, 1.6);
            pui.hline(
                egui_plot::HLine::new(0.0)
                    .color(egui::Color32::from_gray(70))
                    .width(1.0_f32),
            );
        });

    if app.eq_show_error {
        let err: Vec<f32> = match &measured {
            Some(m) => (0..grid.len()).map(|i| target[i] - m[i] - eq[i]).collect(),
            None => vec![0.0; grid.len()],
        };
        plots::FreqPlot::new("eqerr")
            .height(ui.available_height() - 8.0)
            .y_range(-8.0, 8.0)
            .y_label("error dB")
            .legend(false)
            .link_x("freqaxis")
            .show(ui, |pui| {
                plots::line(pui, &grid, &err, "Remaining error", style::BAD, 1.5);
                for y in [-3.0f64, 0.0, 3.0] {
                    pui.hline(
                        egui_plot::HLine::new(y)
                            .color(egui::Color32::from_gray(70))
                            .width(1.0_f32),
                    );
                }
            });
    }
}

fn filter_table(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Filter list").strong());
        if ui.button("+ Add").clicked() {
            app.filters.push(FilterSpec::default());
        }
        if ui.button("Sort by frequency").clicked() {
            app.filters.sort_by(|a, b| {
                a.freq
                    .partial_cmp(&b.freq)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        ui.separator();
        ui.label(format!(
            "{} filters - preamp {:.1} dB",
            app.filters.iter().filter(|f| f.enabled).count(),
            app.preamp_db
        ));
    });
    ui.separator();

    let mut changed = false;
    let mut remove: Option<usize> = None;
    egui::Grid::new("filters")
        .num_columns(7)
        .striped(true)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            ui.label("Type");
            ui.label("Frequency");
            ui.label("Gain");
            ui.label("Q");
            ui.label("BW");
            ui.label("");
            ui.end_row();

            for i in 0..app.filters.len() {
                let f = &mut app.filters[i];
                changed |= ui.checkbox(&mut f.enabled, "").changed();
                egui::ComboBox::from_id_salt(("fk", i))
                    .width(120.0)
                    .selected_text(f.kind.short())
                    .show_ui(ui, |ui| {
                        for k in FilterKind::ALL {
                            if ui.selectable_value(&mut f.kind, k, k.label()).clicked() {
                                changed = true;
                            }
                        }
                    });
                let f_speed = (f.freq * 0.01).max(0.2);
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut f.freq)
                            .range(10.0..=22_000.0)
                            .speed(f_speed)
                            .suffix(" Hz"),
                    )
                    .changed();
                let gain_enabled = f.kind.uses_gain();
                changed |= ui
                    .add_enabled(
                        gain_enabled,
                        egui::DragValue::new(&mut f.gain_db)
                            .range(-30.0..=30.0)
                            .speed(0.1)
                            .suffix(" dB"),
                    )
                    .changed();
                changed |= ui
                    .add(egui::DragValue::new(&mut f.q).range(0.1..=30.0).speed(0.02))
                    .changed();
                ui.monospace(format!("{:.2} oct", biquad::q_to_bw(f.q)));
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });

    if let Some(i) = remove {
        app.filters.remove(i);
        changed = true;
    }
    if changed {
        app.recompute_preamp();
        app.push_eq();
    }

    ui.add_space(6.0);
    widgets::hint(
        ui,
        "After hand edits the preamp is recalculated so the combined response never goes \
         above 0 dB, which is what stops it clipping.",
    );
}

fn export_text(app: &mut AudioTuneApp) {
    let Some(p) = rfd::FileDialog::new()
        .add_filter("Text", &["txt"])
        .set_file_name("eq.txt")
        .save_file()
    else {
        return;
    };
    let sr = app.eq_sr;
    let mut s = String::new();
    s.push_str("# AudioTune - parametric EQ\n");
    s.push_str(&format!("# Coefficients computed at {sr:.0} Hz\n"));
    s.push_str(&format!("Preamp: {:.1} dB\n", app.preamp_db));
    for (i, f) in app.filters.iter().filter(|f| f.enabled).enumerate() {
        s.push_str(&format!(
            "Filter {}: ON {} Fc {:.1} Hz Gain {:.2} dB Q {:.3}\n",
            i + 1,
            f.kind.short(),
            f.freq,
            f.gain_db,
            f.q
        ));
    }
    s.push_str("\n# Biquad coefficients (b0 b1 b2 a1 a2), a0 = 1\n");
    for (i, f) in app.filters.iter().filter(|f| f.enabled).enumerate() {
        let c = biquad::Coeffs::design(f, sr);
        s.push_str(&format!(
            "biquad{}: {:.10} {:.10} {:.10} {:.10} {:.10}\n",
            i + 1,
            c.b0,
            c.b1,
            c.b2,
            c.a1,
            c.a2
        ));
    }
    s.push_str("\n# Combined EQ response (Hz, dB)\n");
    let grid = octave::log_grid(20.0, 20_000.0, 12);
    let resp = biquad::total_response_db(&app.filters, &grid, sr);
    for (f, d) in grid.iter().zip(resp.iter()) {
        s.push_str(&format!("{f:.1} {d:.3}\n"));
    }
    match std::fs::write(&p, s) {
        Ok(_) => app.logln(format!("Saved {}", p.display())),
        Err(e) => app.logln(format!("Save failed: {e}")),
    }
}
