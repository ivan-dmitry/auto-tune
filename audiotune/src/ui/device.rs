//! Tab 5 — the numbers to dial into an external EQ unit.
//!
//! This is the tab that matters when the EQ lives in a separate box (a DSP1462, a
//! 31-band rack graphic, a karaoke processor). The app works out what the box should
//! be set to, quantised to what it can actually do, and shows what the result will be.

use super::{plots, style, widgets};
use crate::app::AudioTuneApp;
use crate::dsp::device_eq::{self, BandLayout, DeviceKind};

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::SidePanel::left("dev_left")
        .resizable(true)
        .default_width(330.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| left(app, ui));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| main_panel(app, ui));
}

fn left(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Your EQ unit");
    ui.horizontal(|ui| {
        ui.label("Preset:");
        if ui
            .button("31-band rack")
            .on_hover_text("1/3 octave, +/-12 dB, 0.5 dB steps")
            .clicked()
        {
            let name = app.device_spec.name.clone();
            app.device_spec = crate::dsp::device_eq::DeviceSpec::preset_rack_31();
            app.device_spec.name = name;
            app.device_result = None;
        }
        if ui
            .button("10-band player")
            .on_hover_text("<31 to >16K, shelved ends, +/-20 dB, 0.1 dB steps")
            .clicked()
        {
            let name = app.device_spec.name.clone();
            app.device_spec = crate::dsp::device_eq::DeviceSpec::preset_player_10();
            app.device_spec.name = name;
            app.device_result = None;
        }
    });
    ui.add_space(4.0);
    egui::Grid::new("devspec")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut app.device_spec.name);
            ui.end_row();

            ui.label("Type");
            egui::ComboBox::from_id_salt("devkind")
                .width(200.0)
                .selected_text(app.device_spec.kind.label())
                .show_ui(ui, |ui| {
                    for k in DeviceKind::ALL {
                        ui.selectable_value(&mut app.device_spec.kind, k, k.label());
                    }
                });
            ui.end_row();

            match app.device_spec.kind {
                DeviceKind::Graphic => {
                    ui.label("Band layout");
                    ui.vertical(|ui| {
                        let before = app.device_spec.layout;
                        egui::ComboBox::from_id_salt("devlayout")
                            .width(220.0)
                            .selected_text(app.device_spec.layout.label())
                            .show_ui(ui, |ui| {
                                for l in BandLayout::ALL {
                                    ui.selectable_value(&mut app.device_spec.layout, l, l.label());
                                }
                            });
                        if app.device_spec.layout != before {
                            app.device_spec.shelf_ends =
                                app.device_spec.layout.default_shelf_ends();
                            app.device_result = None;
                        }
                        ui.checkbox(
                            &mut app.device_spec.shelf_ends,
                            "End bands are shelves (<31 / >16K)",
                        )
                        .on_hover_text(
                            "A control labelled with a < or > is a shelf: it lifts or cuts \
                             everything beyond it, not just one band.",
                        );
                        widgets::hint(
                            ui,
                            &format!("{} bands", app.device_spec.centers().len()),
                        );
                    });
                    ui.end_row();

                    if app.device_spec.layout == BandLayout::Custom {
                        ui.label("Custom bands");
                        ui.vertical(|ui| custom_band_editor(app, ui));
                        ui.end_row();
                    }
                }
                DeviceKind::Parametric => {
                    ui.label("Bands available");
                    ui.add(
                        egui::DragValue::new(&mut app.device_spec.parametric_bands).range(1..=32),
                    );
                    ui.end_row();

                    ui.label("Labelled as");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut app.device_spec.use_bandwidth, true, "Bandwidth");
                        ui.selectable_value(&mut app.device_spec.use_bandwidth, false, "Q");
                    });
                    ui.end_row();
                }
            }

            ui.label("Max boost");
            ui.add(
                egui::DragValue::new(&mut app.device_spec.max_boost_db)
                    .range(0.0..=24.0)
                    .speed(0.5)
                    .suffix(" dB"),
            );
            ui.end_row();

            ui.label("Max cut");
            ui.add(
                egui::DragValue::new(&mut app.device_spec.max_cut_db)
                    .range(0.0..=24.0)
                    .speed(0.5)
                    .suffix(" dB"),
            );
            ui.end_row();

            ui.label("Gain step");
            egui::ComboBox::from_id_salt("devstep")
                .selected_text(format!("{:.1} dB", app.device_spec.step_db))
                .show_ui(ui, |ui| {
                    for s in [0.1f32, 0.25, 0.5, 1.0, 1.5, 2.0] {
                        ui.selectable_value(
                            &mut app.device_spec.step_db,
                            s,
                            format!("{s:.2} dB"),
                        );
                    }
                });
            ui.end_row();
        });
    widgets::hint(
        ui,
        "Set these to match the panel of your unit. Values are rounded to the step it can \
         actually be set to, and the prediction below uses the rounded values, not the ideal \
         ones - so what you see is what you will get.",
    );

    ui.add_space(10.0);
    widgets::section(ui, "What to correct");
    ui.radio_value(&mut app.device_use_average, false, "Selected measurement");
    ui.radio_value(
        &mut app.device_use_average,
        true,
        "Average of all visible measurements",
    );
    widgets::hint(
        ui,
        "Use the average when one EQ has to serve both speakers, or several seats.",
    );
    ui.add_space(4.0);
    widgets::hint(
        ui,
        &format!(
            "Correction range {:.0} - {:.0} Hz, target: {}",
            app.fit_cfg.f_min,
            app.fit_cfg.f_max,
            app.target.kind.label()
        ),
    );

    ui.add_space(8.0);
    let ready = !app.measurements.is_empty();
    if ui
        .add_enabled(
            ready,
            egui::Button::new("Compute settings").min_size(egui::vec2(200.0, 30.0)),
        )
        .clicked()
    {
        app.compute_device_settings();
    }

    if app.device_result.is_some() {
        ui.add_space(6.0);
        if ui
            .button("Audition these exact settings")
            .on_hover_text(
                "Loads the rounded settings into the app's own EQ so you can hear the result \
                 before touching the hardware",
            )
            .clicked()
        {
            app.preview_device_settings();
        }
        ui.horizontal(|ui| {
            if ui.button("Save sheet (.txt)").clicked() {
                export_sheet(app);
            }
            if ui.button("Save CSV").clicked() {
                export_csv(app);
            }
        });
    }

    ui.add_space(10.0);
    widgets::section(ui, "How to transfer it");
    let steps: &[&str] = match app.device_spec.kind {
        DeviceKind::Graphic => &[
            "Set every band on the unit to 0 first, so you start from a known state.",
            "Work down the table below and set each band to the value shown. Bands showing 0.0 stay flat.",
            "If the unit has two channels and you measured one EQ for both, copy the same values to the other channel.",
            "Set the unit's output or level trim by the amount shown, so the boosts cannot clip it.",
            "Play music and listen. If anything sounds wrong, back off the biggest boost or cut first.",
            "Measure again with the EQ in circuit and compare - the new trace should hug the target more closely.",
        ],
        DeviceKind::Parametric => &[
            "Switch every band off or set its gain to 0 first.",
            "For each row set the frequency, then the gain, then the bandwidth or Q.",
            "Enable only as many bands as the table lists; leave the rest bypassed.",
            "Set the unit's output or level trim by the amount shown, so the boosts cannot clip it.",
            "Play music and listen. If anything sounds wrong, back off the biggest boost or cut first.",
            "Measure again with the EQ in circuit and compare - the new trace should hug the target more closely.",
        ],
    };
    for (i, s) in steps.iter().enumerate() {
        ui.label(format!("{}. {}", i + 1, s));
    }
}

/// Build a custom band grid: choose how many and over what range, or edit the
/// centre frequencies one by one.
fn custom_band_editor(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    let mut count = app.device_spec.custom_centers.len().clamp(2, device_eq::MAX_BANDS);
    let first = app
        .device_spec
        .custom_centers
        .first()
        .copied()
        .unwrap_or(31.0);
    let last = app
        .device_spec
        .custom_centers
        .last()
        .copied()
        .unwrap_or(16_000.0);
    let (mut lo, mut hi) = (first, last);

    let mut regenerate = false;
    ui.horizontal(|ui| {
        ui.label("Count");
        regenerate |= ui
            .add(egui::DragValue::new(&mut count).range(2..=device_eq::MAX_BANDS))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("From");
        regenerate |= ui
            .add(
                egui::DragValue::new(&mut lo)
                    .range(10.0..=2000.0)
                    .suffix(" Hz"),
            )
            .changed();
        ui.label("to");
        regenerate |= ui
            .add(
                egui::DragValue::new(&mut hi)
                    .range(1000.0..=22_000.0)
                    .suffix(" Hz"),
            )
            .changed();
    });
    if ui.button("Generate band list").clicked() {
        regenerate = true;
    }
    if regenerate {
        app.device_spec.custom_centers = device_eq::generate_centers(count, lo, hi);
        app.device_result = None;
    }

    ui.collapsing("Edit each frequency", |ui| {
        let mut changed = false;
        let mut remove: Option<usize> = None;
        for i in 0..app.device_spec.custom_centers.len() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{:>2}", i + 1));
                let f = &mut app.device_spec.custom_centers[i];
                let speed = (*f * 0.02).max(0.5);
                changed |= ui
                    .add(
                        egui::DragValue::new(f)
                            .range(10.0..=22_000.0)
                            .speed(speed)
                            .suffix(" Hz"),
                    )
                    .changed();
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            app.device_spec.custom_centers.remove(i);
            changed = true;
        }
        if app.device_spec.custom_centers.len() < device_eq::MAX_BANDS
            && ui.button("+ Add band").clicked()
        {
            let last = app
                .device_spec
                .custom_centers
                .last()
                .copied()
                .unwrap_or(1000.0);
            app.device_spec.custom_centers.push((last * 2.0).min(20_000.0));
            changed = true;
        }
        if changed {
            app.device_result = None;
        }
    });
}

fn main_panel(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    let Some(res) = app.device_result.clone() else {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(
                egui::RichText::new("No settings computed yet")
                    .size(18.0)
                    .color(style::MUTED),
            );
            ui.add_space(8.0);
            ui.label("Measure on tab 3, pick a target on tab 4, then press Compute settings.");
        });
        return;
    };

    // ---- summary line ----
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Set up your {}", app.device_spec.name))
                .strong()
                .size(16.0),
        );
        ui.separator();
        let improve = res.rms_error_before - res.rms_error_after;
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
                "error {:.2} -> {:.2} dB RMS",
                res.rms_error_before, res.rms_error_after
            ),
        );
        ui.separator();
        ui.label(format!("{} band(s) in use", res.active().count()));
        ui.separator();
        ui.colored_label(
            style::WARN,
            format!("output trim {:.1} dB", res.output_trim_db),
        );
    });
    if res.any_limited() {
        ui.colored_label(
            style::BAD,
            "Some bands are pinned at the unit's limit - the room or the speaker placement is \
             the real problem there, not the EQ.",
        );
    }
    ui.separator();

    // ---- split: the settings table on the left, verification plots on the right ----
    egui::SidePanel::left("devtable_panel")
        .resizable(true)
        .default_width(330.0)
        .min_width(240.0)
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("devtable")
                .show(ui, |ui| settings_table(app, ui, &res));
        });
    egui::CentralPanel::default().show_inside(ui, |ui| verification_plot(app, ui, &res));
}

fn settings_table(app: &AudioTuneApp, ui: &mut egui::Ui, res: &device_eq::DeviceResult) {
    let limit = app.device_spec.max_boost_db.max(app.device_spec.max_cut_db);
    match app.device_spec.kind {
        DeviceKind::Graphic => {
            egui::Grid::new("gsettings")
                .num_columns(4)
                .striped(true)
                .spacing([10.0, 3.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Band").strong());
                    ui.label(egui::RichText::new("Control").strong());
                    ui.label(egui::RichText::new("Set to").strong());
                    ui.label("");
                    ui.end_row();
                    for b in &res.settings {
                        ui.monospace(format!("{:>2}", b.index));
                        ui.monospace(if b.label.is_empty() {
                            device_eq::fmt_hz(b.freq_hz)
                        } else {
                            b.label.clone()
                        });
                        let txt = format!("{:+.1} dB", b.gain_db);
                        if b.gain_db.abs() < 0.05 {
                            ui.colored_label(style::MUTED, "  0.0 dB");
                        } else if b.limited {
                            ui.colored_label(style::BAD, txt);
                        } else if b.gain_db > 0.0 {
                            ui.colored_label(style::WARN, txt);
                        } else {
                            ui.colored_label(style::PREDICT, txt);
                        }
                        widgets::gain_bar(ui, b.gain_db, limit, 110.0);
                        ui.end_row();
                    }
                });
        }
        DeviceKind::Parametric => {
            if res.settings.is_empty() {
                widgets::hint(ui, "No band was needed - the response already matches.");
                return;
            }
            egui::Grid::new("psettings")
                .num_columns(5)
                .striped(true)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    ui.label(egui::RichText::new("Band").strong());
                    ui.label(egui::RichText::new("Freq").strong());
                    ui.label(egui::RichText::new("Gain").strong());
                    ui.label(
                        egui::RichText::new(if app.device_spec.use_bandwidth {
                            "BW (oct)"
                        } else {
                            "Q"
                        })
                        .strong(),
                    );
                    ui.label("");
                    ui.end_row();
                    for b in &res.settings {
                        ui.monospace(format!("{:>2}", b.index));
                        ui.monospace(format!("{} Hz", device_eq::fmt_hz(b.freq_hz)));
                        let txt = format!("{:+.1} dB", b.gain_db);
                        if b.limited {
                            ui.colored_label(style::BAD, txt);
                        } else if b.gain_db > 0.0 {
                            ui.colored_label(style::WARN, txt);
                        } else {
                            ui.colored_label(style::PREDICT, txt);
                        }
                        if app.device_spec.use_bandwidth {
                            ui.monospace(format!("{:.2}", b.bandwidth_oct.unwrap_or(1.0)));
                        } else {
                            ui.monospace(format!("{:.2}", b.q.unwrap_or(1.0)));
                        }
                        widgets::gain_bar(ui, b.gain_db, limit, 90.0);
                        ui.end_row();
                    }
                });
        }
    }
    ui.add_space(6.0);
    widgets::hint(
        ui,
        "Orange is a boost, blue is a cut, red means the unit ran out of range.",
    );
}

fn verification_plot(app: &AudioTuneApp, ui: &mut egui::Ui, res: &device_eq::DeviceResult) {
    let grid = &res.grid;
    let h = (ui.available_height() - 60.0) * 0.5;

    ui.label(egui::RichText::new("Does it actually get there?").strong());
    plots::FreqPlot::new("devplot")
        .height(h)
        .y_range(-16.0, 16.0)
        .y_label("dB")
        .link_x("freqaxis")
        .show(ui, |pui| {
            plots::dashed_line(pui, grid, &res.desired, "Correction wanted", style::TARGET);
            plots::line(
                pui,
                grid,
                &res.achieved,
                &format!("What the {} does", app.device_spec.name),
                style::EQ_CURVE,
                2.0,
            );
            pui.hline(
                egui_plot::HLine::new(0.0)
                    .color(egui::Color32::from_gray(70))
                    .width(1.0_f32),
            );
        });

    ui.add_space(4.0);
    ui.label(egui::RichText::new("Predicted result at the mic").strong());
    let measured = app.measured_curve(grid).or_else(|| app.averaged_curve(grid));
    let target = app.target.on_grid(grid);
    plots::FreqPlot::new("devresult")
        .height(h)
        .y_range(-20.0, 12.0)
        .y_label("dB")
        .link_x("freqaxis")
        .show(ui, |pui| {
            plots::dashed_line(pui, grid, &target, "Target", style::TARGET);
            if let Some(m) = &measured {
                plots::line(pui, grid, m, "Before", style::MUTED, 1.5);
                let after: Vec<f32> = m
                    .iter()
                    .zip(res.achieved.iter())
                    .map(|(a, b)| a + b)
                    .collect();
                plots::line(pui, grid, &after, "After", style::PREDICT, 2.2);
            }
        });
}

fn export_sheet(app: &mut AudioTuneApp) {
    let Some(res) = app.device_result.clone() else {
        return;
    };
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("Text", &["txt"])
        .set_file_name(format!("{}_settings.txt", sanitize(&app.device_spec.name)))
        .save_file()
    {
        let header = format!(
            "AudioTune settings sheet - target {}, range {:.0}-{:.0} Hz",
            app.target.kind.label(),
            app.fit_cfg.f_min,
            app.fit_cfg.f_max
        );
        let text = device_eq::settings_sheet(&res, &app.device_spec, &header);
        match std::fs::write(&p, text) {
            Ok(_) => app.logln(format!("Saved {}", p.display())),
            Err(e) => app.logln(format!("Save failed: {e}")),
        }
    }
}

fn export_csv(app: &mut AudioTuneApp) {
    let Some(res) = app.device_result.clone() else {
        return;
    };
    if let Some(p) = rfd::FileDialog::new()
        .add_filter("CSV", &["csv"])
        .set_file_name(format!("{}_settings.csv", sanitize(&app.device_spec.name)))
        .save_file()
    {
        let text = device_eq::settings_csv(&res, &app.device_spec);
        match std::fs::write(&p, text) {
            Ok(_) => app.logln(format!("Saved {}", p.display())),
            Err(e) => app.logln(format!("Save failed: {e}")),
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}
