//! The interface: top bar, status bar and the tab contents.

pub mod correction;
pub mod device;
pub mod guide;
pub mod measure;
pub mod plots;
pub mod rta;
pub mod setup;
pub mod style;
pub mod widgets;

use crate::app::{AudioTuneApp, MeasureState, Tab};
use crate::model::TuneMode;

pub fn draw(app: &mut AudioTuneApp, ctx: &egui::Context) {
    egui::TopBottomPanel::top("topbar")
        .exact_height(46.0)
        .show(ctx, |ui| top_bar(app, ui));
    egui::TopBottomPanel::bottom("statusbar")
        .exact_height(56.0)
        .show(ctx, |ui| status_bar(app, ui));
    egui::CentralPanel::default().show(ctx, |ui| match app.tab {
        Tab::Setup => setup::show(app, ui),
        Tab::Rta => rta::show(app, ui),
        Tab::Measure => measure::show(app, ui),
        Tab::Correction => correction::show(app, ui),
        Tab::Device => device::show(app, ui),
        Tab::Guide => guide::show(app, ui),
    });
}

fn top_bar(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    ui.horizontal_centered(|ui| {
        ui.heading("AudioTune");
        ui.separator();
        for t in Tab::ALL {
            if ui.selectable_label(app.tab == t, t.label()).clicked() {
                app.tab = t;
            }
        }
        ui.separator();
        ui.label("System:");
        egui::ComboBox::from_id_salt("mode")
            .selected_text(app.mode.label())
            .show_ui(ui, |ui| {
                for m in TuneMode::ALL {
                    if ui
                        .selectable_value(&mut app.mode, m, m.label())
                        .on_hover_text(m.hint())
                        .clicked()
                    {
                        apply_mode_defaults(app, m);
                    }
                }
            });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if app.engine.is_some() {
                if ui
                    .button("Close streams")
                    .on_hover_text("Disconnect from the sound card")
                    .clicked()
                {
                    app.stop_engine();
                }
                ui.colored_label(style::OK, "* running");
            } else {
                if ui.button("Open audio streams").clicked() {
                    app.start_engine();
                }
                ui.colored_label(style::MUTED, "o not open");
            }
            ui.separator();
            if ui.button("Save project...").clicked() {
                save_project(app);
            }
            if ui.button("Open project...").clicked() {
                open_project(app);
            }
        });
    });
}

fn apply_mode_defaults(app: &mut AudioTuneApp, m: TuneMode) {
    match m {
        TuneMode::OneSpeaker | TuneMode::Stereo => {
            app.fit_cfg = crate::dsp::autoeq::FitConfig::preset_speaker();
            app.target.kind = crate::dsp::target::TargetKind::HarmanRoom;
            app.ir_window = crate::dsp::measurement::IrWindow::default();
        }
        TuneMode::Headphone => {
            app.fit_cfg = crate::dsp::autoeq::FitConfig::preset_headphone();
            app.target.kind = crate::dsp::target::TargetKind::HarmanHeadphone;
            // Headphones have no room reflections, so a long window means nothing.
            app.ir_window = crate::dsp::measurement::IrWindow::gated(80.0);
        }
    }
    app.logln(format!("Switched to: {}", m.label()));
}

fn status_bar(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("Input:");
                widgets::level_meter(ui, app.level.rms_db, app.level.peak_db, 220.0);
                let spl = if app.spl_cal.enabled {
                    format!("  ({:.1} dB SPL)", app.spl_cal.to_spl(app.level.rms_db))
                } else {
                    String::new()
                };
                ui.monospace(format!(
                    "{:>6.1} dBFS RMS / {:>6.1} peak{}",
                    app.level.rms_db, app.level.peak_db, spl
                ));
                if app.level.clipped {
                    ui.colored_label(style::BAD, "CLIP");
                }
            });
            ui.horizontal(|ui| {
                match &app.mstate {
                    MeasureState::Idle => {}
                    MeasureState::Countdown { .. } => {
                        ui.colored_label(style::WARN, "Getting ready...");
                    }
                    MeasureState::Gap { .. } => {
                        ui.colored_label(style::WARN, "Pause between passes...");
                    }
                    MeasureState::Capturing => {
                        ui.colored_label(style::WARN, "MEASURING - keep quiet");
                        ui.add(
                            egui::ProgressBar::new(app.recorder.progress())
                                .desired_width(200.0)
                                .text(format!(
                                    "{:.1} / {:.1} s",
                                    app.recorder.seconds(),
                                    app.recorder.target_seconds()
                                )),
                        );
                    }
                }
                if let Some(last) = app.log.last() {
                    ui.colored_label(style::MUTED, last);
                }
            });
        });
    });
}

fn save_project(app: &mut AudioTuneApp) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("AudioTune project", &["atune", "json"])
        .set_file_name("project.atune")
        .save_file()
    {
        match app.project().save(&path) {
            Ok(_) => app.logln(format!("Saved {}", path.display())),
            Err(e) => app.logln(format!("Save failed: {e}")),
        }
    }
}

fn open_project(app: &mut AudioTuneApp) {
    if let Some(path) = rfd::FileDialog::new()
        .add_filter("AudioTune project", &["atune", "json"])
        .pick_file()
    {
        match crate::model::Project::load(&path) {
            Ok(p) => app.load_project(p),
            Err(e) => app.logln(format!("Open failed: {e}")),
        }
    }
}
