//! AudioTune — measure a speaker or headphone with a microphone and work out the EQ
//! settings that bring it onto a target curve.
//!
//! Runs on Windows (WASAPI) and Linux (ALSA / JACK) from the same source.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod dsp;
mod model;
mod ui;

fn main() -> eframe::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list-devices" || a == "-l") {
        list_devices();
        return Ok(());
    }
    if args.iter().any(|a| a == "--selftest") {
        selftest();
        return Ok(());
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("AudioTune");
        println!("  (no arguments)    start the graphical application");
        println!("  --list-devices    print every audio host and device cpal can see");
        println!("  --autostart       open the default devices and go straight to the analyser");
        println!("  --selftest        open the real streams and record 1.5 s of ambient noise");
        println!("                    (silent: it never plays anything)");
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1480.0, 940.0])
            .with_min_inner_size([1020.0, 680.0])
            .with_title("AudioTune - acoustic measurement and EQ"),
        ..Default::default()
    };

    eframe::run_native(
        "AudioTune",
        options,
        Box::new(|cc| Ok(Box::new(app::AudioTuneApp::new(cc)))),
    )
}

/// Print every host and device cpal can see, for diagnosis.
fn list_devices() {
    for host in audio::devices::available_hosts() {
        println!("== Host: {} ==", audio::devices::host_label(host));
        let list = audio::devices::enumerate(host);
        println!("-- Inputs ({}):", list.inputs.len());
        for d in &list.inputs {
            print_device(d);
        }
        println!("-- Outputs ({}):", list.outputs.len());
        for d in &list.outputs {
            print_device(d);
        }
    }
}

fn print_device(d: &audio::devices::DeviceInfo) {
    println!(
        "   {}{}  |  up to {} ch  |  default {} Hz / {} ch  |  {} Hz  |  {:?}",
        if d.is_default { "* " } else { "  " },
        d.name,
        d.max_channels,
        d.default_sample_rate,
        d.default_channels,
        d.sample_rates
            .iter()
            .map(|r| r.to_string())
            .collect::<Vec<_>>()
            .join("/"),
        d.formats
    );
}

/// Check the capture path end to end: open the real streams, record ~1.5 s of room
/// noise, analyse it. Plays nothing, so it is safe to run at any time.
fn selftest() {
    use audio::engine::{Engine, EngineConfig};
    let Some(&host) = audio::devices::available_hosts().first() else {
        println!("No audio host available.");
        return;
    };
    let list = audio::devices::enumerate(host);
    let Some(indev) = list.inputs.first() else {
        println!("No input device found.");
        return;
    };
    let Some(outdev) = list.outputs.first() else {
        println!("No output device found.");
        return;
    };
    let sr = indev.default_sample_rate.min(outdev.default_sample_rate);
    println!("Input:  {}\nOutput: {}\nSample rate: {sr} Hz", indev.name, outdev.name);

    let cfg = EngineConfig {
        host,
        input_device: indev.name.clone(),
        output_device: outdev.name.clone(),
        sample_rate_in: sr,
        sample_rate_out: sr,
        input_channels: indev.default_channels.max(1),
        output_channels: outdev.default_channels.max(1),
        input_channel_index: 0,
        buffer_frames: None,
    };
    let engine = match Engine::start(&cfg) {
        Ok(e) => e,
        Err(e) => {
            println!("FAILED to open streams: {e:#}");
            return;
        }
    };
    println!("Streams open. Recording 1.5 s of ambient noise (nothing is played)...");

    let mut analyzer =
        dsp::spectrum::Analyzer::new(engine.sr_in, 8192, dsp::window::WindowKind::Hann);
    analyzer.averaging = dsp::spectrum::Averaging::Infinite;
    let mut captured: Vec<f32> = Vec::new();
    let need = (engine.sr_in * 1.5) as usize;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
    while captured.len() < need && std::time::Instant::now() < deadline {
        while let Ok(b) = engine.in_rx.try_recv() {
            analyzer.push(&b);
            captured.extend_from_slice(&b);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    if captured.is_empty() {
        println!("NO samples arrived - check the OS microphone permission.");
        return;
    }
    let lvl = dsp::spectrum::measure_level(&captured);
    println!(
        "Captured {} samples ({:.2} s). Noise floor: {:.1} dBFS RMS, peak {:.1} dBFS.",
        captured.len(),
        captured.len() as f32 / engine.sr_in,
        lvl.rms_db,
        lvl.peak_db
    );
    let amp = analyzer.amplitude();
    let bands = dsp::octave::band_levels(&amp, engine.sr_in, 8192, 1, 31.5, 16_000.0);
    println!("Noise floor per octave:");
    for (f, d) in bands.centers.iter().zip(bands.db.iter()) {
        println!("  {f:>8.0} Hz  {d:>7.1} dBFS");
    }
    println!("FFT blocks processed: {}", analyzer.blocks);
    let xruns = engine
        .shared
        .xruns
        .load(std::sync::atomic::Ordering::Relaxed);
    println!("Dropped blocks (xruns): {xruns}");
    println!("OK - the capture path works.");
}
