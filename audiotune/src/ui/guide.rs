//! Guide tab — the measurement workflow and how to move the result onto hardware.

use super::widgets;
use crate::app::AudioTuneApp;
use crate::dsp::device_eq::DeviceKind;
use crate::model::TuneMode;

pub fn show(app: &mut AudioTuneApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.set_max_width(1000.0);

        ui.heading("The workflow");
        ui.add_space(4.0);
        step(ui, 1, "Prepare the microphone", &[
            "Use a measurement mic (UMIK-1, Dayton EMM-6, Behringer ECM8000). A headset or webcam mic gives you a rough idea at best, because it is coloured and usually noise-gated.",
            "Turn off every OS-side effect: noise suppression, echo cancellation, AGC, microphone boost. On Windows: Sound, Recording, Properties, Enhancements, disable all. On Linux: unload the PulseAudio or PipeWire echo-cancel module.",
            "If the mic came with a .cal file, load it on the Setup tab.",
        ]);
        step(ui, 2, "Open the streams and set levels", &[
            "Setup tab: choose the input and output device and give them the same sample rate. 48 kHz is plenty.",
            "Play pink noise and set the card's gain so the input peaks around -12 dBFS. Never let it hit 0 dBFS.",
            "If you want real SPL figures, calibrate against an SPL meter.",
        ]);
        step(ui, 3, "Put the mic in the right place", &[
            "At the listening position, at ear height, pointed straight at the speaker.",
            "On a stand, not in your hand. Stand out of the path between speaker and mic.",
            "Measuring one speaker means muting the other. Measuring both at once gives you cancellation dips from the path difference, and the curve is then meaningless.",
        ]);
        step(ui, 4, "Run the sweep", &[
            "Sweep tab: 20 Hz to 20 kHz. Six seconds is enough in a normal room; add length or passes if the room is noisy.",
            "Stay quiet for the whole sweep. Air conditioning, fans and fridges all pollute the low end.",
            "Check afterwards: the input did not clip, SNR is above 40 dB, THD is under about 1 %. That is a good capture.",
        ]);
        step(ui, 5, "Choose the IR window", &[
            "Correcting for the listening seat: use the long window (300 ms), because the room's reflections are part of what you hear.",
            "Judging the speaker on its own, without the room: use Gated 6 ms - but then anything below roughly 200 Hz is no longer trustworthy.",
        ]);
        step(ui, 6, "Work out the correction", &[
            "Correction tab: pick a target. Speakers in a room go with Harman in-room; a treated room can take Flat.",
            "Set a sensible correction range. Press Suggest low limit so the EQ does not try to force frequencies the speaker cannot produce.",
            "Press Auto-fit correction, then adjust by hand if you want to.",
            "Turn EQ on, play music you know well, toggle it on and off. Your ears decide, not the graph.",
        ]);
        step(ui, 7, "Transfer it to your EQ unit", &[
            "EQ settings tab: tell the app what your unit is - graphic or parametric, how many bands, its gain limit and step size.",
            "Press Compute settings. The app solves for the band values whose combined effect matches the correction, and rounds them to what the unit can be set to.",
            "Read the table and set the unit. The right-hand plots show what that will actually achieve, using the rounded values.",
            "Save the sheet as a text or CSV file if you want it beside the rack.",
        ]);
        step(ui, 8, "Verify", &[
            "With the EQ in circuit, run the sweep again. The new trace should sit closer to the target than the old one.",
            "If some region got worse, reduce How hard to correct, or use fewer bands, and repeat.",
        ]);

        ui.add_space(12.0);
        ui.heading("Why a graphic EQ needs solving, not copying");
        bullets(ui, &[
            "The bands of a graphic EQ overlap. Setting each slider to the error measured at its own centre frequency stacks the neighbours on top of each other and overshoots, often by several dB.",
            "This app solves for all bands together, so their combined curve matches the correction. That is why the numbers in the table are smaller than the raw error you see on the Correction tab.",
            "Rounding matters too. On a 0.5 dB unit, a wanted -3.2 dB becomes -3.0 dB, and the prediction is drawn from -3.0, not -3.2.",
        ]);

        ui.add_space(12.0);
        ui.heading("Per system type");
        ui.add_space(4.0);
        for m in TuneMode::ALL {
            let sel = app.mode == m;
            egui::Frame::none()
                .fill(if sel {
                    egui::Color32::from_rgb(34, 42, 50)
                } else {
                    egui::Color32::from_rgb(28, 30, 34)
                })
                .inner_margin(egui::Margin::same(10.0))
                .rounding(6.0)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(m.label()).strong().size(15.0));
                        if !sel && ui.small_button("Use this mode").clicked() {
                            app.mode = m;
                        }
                    });
                    widgets::hint(ui, m.hint());
                    for line in mode_notes(m) {
                        ui.label(format!("- {line}"));
                    }
                });
            ui.add_space(6.0);
        }

        ui.add_space(12.0);
        ui.heading("Reading the result");
        bullets(ui, &[
            "A large hump below 200 Hz is usually a room mode, not a speaker fault. Cutting it works; boosting it does not, and wastes amplifier power.",
            "Deep narrow dips are usually reflection cancellation. EQ almost never fixes those - moving the speaker or the seat works far better.",
            "RT60 in a domestic listening room wants to be around 0.3 to 0.5 s. Higher than that means a reverberant room, which EQ cannot cure; that needs absorption.",
            "Above 5 kHz the measurement depends heavily on how the mic is aimed. Do not apply sharp corrections up there on the strength of one graph.",
            "Prefer cutting to boosting. Every 3 dB of boost doubles the power the amplifier has to deliver.",
        ]);

        ui.add_space(12.0);
        ui.heading("Limits worth knowing");
        bullets(ui, &[
            "EQ changes magnitude. It does not change the room's decay time, and it does not reduce the speaker's distortion.",
            "One measurement at one point is only valid at that point. To cover a whole sofa, measure a few positions and use Fit from average.",
            "The Harman headphone target only means something measured through a proper coupler. A measurement mic held in front of a headphone is not usable for it.",
            "The app measures the card's loopback delay together with the acoustic delay. Enter the system latency on the Setup tab if you want a real distance figure.",
        ]);

        ui.add_space(12.0);
        ui.heading("Current EQ unit");
        widgets::hint(
            ui,
            &format!(
                "{} - {}{}. Limits +{:.0} / -{:.0} dB in {:.1} dB steps.",
                app.device_spec.name,
                app.device_spec.kind.label(),
                match app.device_spec.kind {
                    DeviceKind::Graphic => format!(", {}", app.device_spec.layout.label()),
                    DeviceKind::Parametric =>
                        format!(", {} bands", app.device_spec.parametric_bands),
                },
                app.device_spec.max_boost_db,
                app.device_spec.max_cut_db,
                app.device_spec.step_db
            ),
        );
        widgets::hint(
            ui,
            "If those do not match your unit, change them on the EQ settings tab - the table \
             and the prediction both depend on them.",
        );
        ui.add_space(20.0);
    });
}

fn mode_notes(m: TuneMode) -> &'static [&'static str] {
    match m {
        TuneMode::OneSpeaker => &[
            "Set Play to so it matches the channel the speaker is actually wired to.",
            "A useful pair of measurements: close up (30-50 cm on axis) to see the speaker itself, then at the seat to see what the room does to it.",
        ],
        TuneMode::Stereo => &[
            "The app measures left then right, then reports the level and timing mismatch.",
            "More than 1 dB of level mismatch: rebalance the volume or trim before you EQ anything.",
            "More than 0.5 ms of timing mismatch (about 17 cm): move a speaker so the distances match, or add delay at the source.",
            "Prefer one shared EQ via Fit from average, and only correct a channel separately in the bass if you really have to.",
        ],
        TuneMode::Headphone => &[
            "A coupler, or an in-ear measurement mic, is mandatory. Without one, use the live analyser to compare by ear instead.",
            "Set the IR window to 80 ms; headphones have no room reflections.",
            "Re-seat the headphone and measure several times - above 3 kHz the result shifts a lot with placement.",
        ],
    }
}

fn step(ui: &mut egui::Ui, n: u32, title: &str, lines: &[&str]) {
    egui::Frame::none()
        .fill(egui::Color32::from_rgb(28, 30, 34))
        .inner_margin(egui::Margin::same(10.0))
        .rounding(6.0)
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(format!("Step {n} - {title}"))
                    .strong()
                    .size(15.0),
            );
            for l in lines {
                ui.label(format!("- {l}"));
            }
        });
    ui.add_space(6.0);
}

fn bullets(ui: &mut egui::Ui, lines: &[&str]) {
    for l in lines {
        ui.label(format!("- {l}"));
    }
}
