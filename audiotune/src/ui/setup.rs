//! Tab 1 — pick devices, check the input level, calibrate mic and SPL.

use super::{style, widgets};
use crate::app::AudioTuneApp;
use crate::dsp::calibration::MicCalibration;
use crate::dsp::generator::SignalKind;

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.columns(2, |cols| {
            devices_panel(app, &mut cols[0]);
            calibration_panel(app, &mut cols[1]);
        });
    });
}

fn devices_panel(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Audio devices");

    let locked = app.engine.is_some();
    ui.add_enabled_ui(!locked, |ui| {
        egui::Grid::new("devgrid")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .show(ui, |ui| {
                ui.label("Driver (host)");
                let names: Vec<String> = app
                    .hosts
                    .iter()
                    .map(|h| crate::audio::devices::host_label(*h))
                    .collect();
                let cur = names
                    .get(app.host_idx)
                    .cloned()
                    .unwrap_or_else(|| "?".into());
                egui::ComboBox::from_id_salt("host")
                    .selected_text(cur)
                    .show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_value(&mut app.host_idx, i, n).clicked() {
                                app.refresh_devices();
                            }
                        }
                    });
                ui.end_row();

                ui.label("Microphone / input");
                ui.vertical(|ui| {
                    let names: Vec<String> = app
                        .device_list
                        .inputs
                        .iter()
                        .map(|d| d.name.clone())
                        .collect();
                    egui::ComboBox::from_id_salt("indev")
                        .width(340.0)
                        .selected_text(short(&app.in_dev))
                        .show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut app.in_dev, n.clone(), n);
                            }
                        });
                    if let Some(d) = app.device_list.inputs.iter().find(|d| d.name == app.in_dev) {
                        widgets::hint(
                            ui,
                            &format!(
                                "{} channels, supported rates: {}",
                                d.max_channels,
                                d.sample_rates
                                    .iter()
                                    .map(|r| r.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        );
                    }
                });
                ui.end_row();

                ui.label("Input channels to open");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut app.in_channels).range(1..=32));
                    ui.label("- mic is on channel");
                    ui.add(
                        egui::DragValue::new(&mut app.in_channel_idx)
                            .range(0..=(app.in_channels.saturating_sub(1) as usize)),
                    );
                });
                ui.end_row();

                ui.label("Speakers / output");
                ui.vertical(|ui| {
                    let names: Vec<String> = app
                        .device_list
                        .outputs
                        .iter()
                        .map(|d| d.name.clone())
                        .collect();
                    egui::ComboBox::from_id_salt("outdev")
                        .width(340.0)
                        .selected_text(short(&app.out_dev))
                        .show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut app.out_dev, n.clone(), n);
                            }
                        });
                    if let Some(d) = app.device_list.outputs.iter().find(|d| d.name == app.out_dev) {
                        widgets::hint(ui, &format!("{} channels available", d.max_channels));
                    }
                });
                ui.end_row();

                ui.label("Output channels to open");
                ui.add(egui::DragValue::new(&mut app.out_channels).range(1..=32));
                ui.end_row();

                ui.label("Sample rate");
                let rates = common_rates(app);
                egui::ComboBox::from_id_salt("sr")
                    .selected_text(format!("{} Hz", app.sample_rate))
                    .show_ui(ui, |ui| {
                        for r in rates {
                            ui.selectable_value(&mut app.sample_rate, r, format!("{r} Hz"));
                        }
                    });
                ui.end_row();

                ui.label("Buffer size");
                ui.horizontal(|ui| {
                    let mut fixed = app.buffer_frames.is_some();
                    if ui.checkbox(&mut fixed, "Fixed").changed() {
                        app.buffer_frames = if fixed { Some(512) } else { None };
                    }
                    if let Some(b) = &mut app.buffer_frames {
                        ui.add(egui::DragValue::new(b).range(64..=8192).suffix(" frames"));
                    } else {
                        widgets::hint(ui, "let the driver decide (safest)");
                    }
                });
                ui.end_row();
            });
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("Rescan devices").clicked() {
            app.refresh_devices();
            app.logln("Device list rescanned.");
        }
        if locked {
            if ui.button("Close streams").clicked() {
                app.stop_engine();
            }
        } else if ui.button("Open audio streams").clicked() {
            app.start_engine();
        }
    });

    if let Some(e) = &app.engine_error {
        ui.add_space(4.0);
        ui.colored_label(style::BAD, format!("Error: {e}"));
        widgets::hint(
            ui,
            "Try fewer channels, a different sample rate, or turn off the fixed buffer size.",
        );
    }

    if let Some(e) = &app.engine {
        ui.add_space(4.0);
        ui.monospace(format!(
            "Running - in {:.0} Hz / {} ch, out {:.0} Hz / {} ch",
            e.sr_in, e.ch_in, e.sr_out, e.ch_out
        ));
        // Switchable while running, so you can hunt for the channel the mic is on
        // without closing and reopening the streams.
        if e.ch_in > 1 {
            ui.horizontal(|ui| {
                ui.label("Mic is on input channel");
                if ui
                    .add(
                        egui::DragValue::new(&mut app.in_channel_idx)
                            .range(0..=(e.ch_in.saturating_sub(1) as usize)),
                    )
                    .changed()
                {
                    e.set_input_channel(app.in_channel_idx);
                }
            });
        }
    }

    ui.add_space(10.0);
    widgets::section(ui, "Input level check");
    widgets::hint(
        ui,
        "Clap or talk into the mic. Set the card's gain so peaks land around -12 dBFS while \
         the test signal plays. Never let it touch 0 dBFS.",
    );
    ui.add_space(4.0);
    widgets::level_meter(ui, app.level.rms_db, app.level.peak_db, 400.0);
    ui.monospace(format!(
        "RMS {:.1} dBFS - peak {:.1} dBFS",
        app.level.rms_db, app.level.peak_db
    ));
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let playing = app.test_playing;
        if ui
            .add_enabled(
                app.engine.is_some(),
                egui::Button::new(if playing {
                    "Stop pink noise"
                } else {
                    "Play pink noise"
                }),
            )
            .clicked()
        {
            if playing {
                app.stop_test_signal();
            } else {
                app.signal = SignalKind::PinkNoise;
                app.start_test_signal();
            }
        }
        ui.add(egui::Slider::new(&mut app.signal_level_db, -60.0..=0.0).text("level (dBFS)"));
    });
}

fn calibration_panel(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    widgets::section(ui, "Microphone calibration");
    widgets::hint(
        ui,
        "Measurement mics (UMIK-1, Dayton EMM-6, Behringer ECM8000 and so on) usually ship \
         with a .cal / .frd file describing their own error. Load it and it is subtracted.",
    );
    ui.horizontal(|ui| {
        if ui.button("Load calibration file...").clicked() {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Calibration file", &["cal", "frd", "txt"])
                .pick_file()
            {
                match std::fs::read_to_string(&p) {
                    Ok(text) => {
                        let name = p
                            .file_name()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        let cal = MicCalibration::parse(&name, &text);
                        if cal.is_empty() {
                            app.logln("No usable data points found in that file.");
                        } else {
                            app.logln(format!(
                                "Loaded calibration \"{}\" with {} points ({:.0}-{:.0} Hz).",
                                cal.name,
                                cal.freqs.len(),
                                cal.freqs.first().copied().unwrap_or(0.0),
                                cal.freqs.last().copied().unwrap_or(0.0)
                            ));
                            app.mic_cal = cal;
                        }
                    }
                    Err(e) => app.logln(format!("Could not read the file: {e}")),
                }
            }
        }
        if !app.mic_cal.is_empty() {
            ui.checkbox(&mut app.mic_cal.enabled, "Apply");
            if ui.button("Clear").clicked() {
                app.mic_cal = MicCalibration::default();
            }
        }
    });
    if !app.mic_cal.is_empty() {
        ui.monospace(format!(
            "{} - {} points",
            app.mic_cal.name,
            app.mic_cal.freqs.len()
        ));
        super::plots::FreqPlot::new("calplot")
            .height(140.0)
            .y_range(-10.0, 10.0)
            .y_label("dB")
            .legend(false)
            .show(ui, |pui| {
                super::plots::line(
                    pui,
                    &app.mic_cal.freqs,
                    &app.mic_cal.db,
                    "cal",
                    style::TARGET,
                    1.5,
                );
            });
    }

    ui.add_space(10.0);
    widgets::section(ui, "SPL calibration");
    widgets::hint(
        ui,
        "Needs an SPL meter (or a calibrated phone app). Play pink noise, put the meter next \
         to the mic, type in what it reads, then press Calibrate.",
    );
    ui.horizontal(|ui| {
        ui.label("SPL meter reads:");
        ui.add(
            egui::DragValue::new(&mut app.spl_reference_input)
                .range(30.0..=130.0)
                .suffix(" dB"),
        );
        if ui
            .add_enabled(app.engine.is_some(), egui::Button::new("Calibrate"))
            .clicked()
        {
            let rms = app.level.rms_db;
            let r = app.spl_reference_input;
            app.spl_cal.calibrate(rms, r);
            app.logln(format!(
                "SPL calibrated: 0 dBFS = {:.1} dB SPL.",
                app.spl_cal.spl_at_full_scale
            ));
        }
    });
    ui.horizontal(|ui| {
        ui.checkbox(&mut app.spl_cal.enabled, "Show dB SPL");
        ui.monospace(format!(
            "0 dBFS = {:.1} dB SPL",
            app.spl_cal.spl_at_full_scale
        ));
    });

    ui.add_space(10.0);
    widgets::section(ui, "System latency");
    widgets::hint(
        ui,
        "The card's round-trip delay (output back to input). Subtract it from the measured \
         delay to get the real speaker-to-mic distance. Measure it by patching the output \
         into the input with a cable and running one sweep.",
    );
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut app.system_latency_ms)
                .range(0.0..=2000.0)
                .speed(0.1)
                .suffix(" ms"),
        );
        if let Some(i) = app.selected {
            if let Some(m) = app.measurements.get(i) {
                let d = m.ir.delay_ms();
                if ui
                    .button(format!("Take from selected measurement ({d:.2} ms)"))
                    .clicked()
                {
                    app.system_latency_ms = d;
                    app.logln(format!("System latency set to {d:.2} ms."));
                }
            }
        }
    });

    ui.add_space(10.0);
    widgets::section(ui, "Log");
    // Newest first, so there is nothing to auto-scroll. Sticking a scroll area to
    // the bottom keeps an animation alive and repaints the whole window forever.
    egui::ScrollArea::vertical()
        .id_salt("log")
        .max_height(160.0)
        .show(ui, |ui| {
            for l in app.log.iter().rev().take(80) {
                ui.monospace(l);
            }
        });
}

fn short(s: &str) -> String {
    if s.chars().count() > 44 {
        let t: String = s.chars().take(42).collect();
        format!("{t}...")
    } else if s.is_empty() {
        "- none selected -".into()
    } else {
        s.to_string()
    }
}

/// Sample rates that both the input and the output device support.
fn common_rates(app: &AudioTuneApp) -> Vec<u32> {
    let i = app
        .device_list
        .inputs
        .iter()
        .find(|d| d.name == app.in_dev)
        .map(|d| d.sample_rates.clone())
        .unwrap_or_default();
    let o = app
        .device_list
        .outputs
        .iter()
        .find(|d| d.name == app.out_dev)
        .map(|d| d.sample_rates.clone())
        .unwrap_or_default();
    let mut v: Vec<u32> = if i.is_empty() || o.is_empty() {
        let mut all = i.clone();
        all.extend(o.iter().copied());
        all
    } else {
        i.iter().filter(|r| o.contains(r)).copied().collect()
    };
    if v.is_empty() {
        v = vec![44100, 48000, 96000];
    }
    v.sort_unstable();
    v.dedup();
    v
}
