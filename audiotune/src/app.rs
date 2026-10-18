//! Application state and the main loop.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::audio::devices::{self, DeviceList};
use crate::audio::engine::{Engine, EngineConfig, Route};
use crate::audio::fileio::AudioFile;
use crate::audio::recorder::{RecordState, Recorder};
use crate::dsp::autoeq::{self, FitConfig, FitResult};
use crate::dsp::biquad::FilterSpec;
use crate::dsp::calibration::{MicCalibration, SplCalibration, Weighting};
use crate::dsp::device_eq::{self, DeviceKind, DeviceResult, DeviceSpec};
use crate::dsp::fft::Fft;
use crate::dsp::generator::{self, PinkNoise, Rng, SignalKind, Source, SweepConfig};
use crate::dsp::measurement::{self, IrWindow};
use crate::dsp::octave;
use crate::dsp::spectrum::{Analyzer, Averaging, LevelMeter};
use crate::dsp::target::TargetCurve;
use crate::dsp::window::WindowKind;
use crate::model::{timestamp_now, Measurement, Project, TuneMode};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Tab {
    Setup,
    Rta,
    Measure,
    Correction,
    Device,
    Guide,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Setup,
        Tab::Rta,
        Tab::Measure,
        Tab::Correction,
        Tab::Device,
        Tab::Guide,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Setup => "1 - Setup",
            Tab::Rta => "2 - Live analyser",
            Tab::Measure => "3 - Sweep measurement",
            Tab::Correction => "4 - Correction",
            Tab::Device => "5 - EQ settings",
            Tab::Guide => "? - Guide",
        }
    }
}

/// How the live analyser draws its data.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RtaView {
    /// Full FFT spectrum.
    Spectrum,
    /// One value per octave band.
    Bands,
}

pub struct MeasureJob {
    pub routes: Vec<(Route, String)>,
    pub route_idx: usize,
    pub rep: u32,
    pub captures: Vec<Vec<f32>>,
    pub sweep_out: Arc<Vec<f32>>,
    pub inverse_in: Vec<f32>,
    pub cfg: SweepConfig,
    pub deadline: Instant,
}

pub enum MeasureState {
    Idle,
    Countdown { until: Instant },
    Capturing,
    Gap { until: Instant },
}

/// Cached smoothed traces for the measurement magnitude plot. Recomputing a
/// 32k-point response onto a log grid for every visible trace, every frame, is
/// the sort of thing that makes a UI feel heavy for no reason.
#[derive(Default)]
pub struct MagCache {
    pub key: u64,
    pub grid: Vec<f32>,
    /// (colour index, trace name, curve on the grid)
    pub curves: Vec<(usize, String, Vec<f32>)>,
}

/// Cached display curve for the live analyser.
#[derive(Default)]
pub struct RtaCache {
    /// Hash of every setting that changes the curve.
    pub key: u64,
    /// Number of FFT blocks the analyser had processed when this was built.
    pub blocks: u64,
    pub freqs: Vec<f32>,
    pub db: Vec<f32>,
    pub peak: Option<Vec<f32>>,
    pub y_label: String,
    pub y_center: f32,
    pub is_bands: bool,
}

impl RtaCache {
    pub fn is_stale(&self, key: u64, blocks: u64) -> bool {
        self.key != key || self.blocks != blocks || self.freqs.is_empty()
    }
}

pub struct AudioTuneApp {
    // ---- devices ----
    pub hosts: Vec<cpal::HostId>,
    pub host_idx: usize,
    pub device_list: DeviceList,
    pub in_dev: String,
    pub out_dev: String,
    pub sample_rate: u32,
    pub in_channels: u16,
    pub out_channels: u16,
    pub in_channel_idx: usize,
    pub buffer_frames: Option<u32>,
    pub engine: Option<Engine>,
    pub engine_error: Option<String>,

    // ---- live analyser ----
    pub analyzer: Analyzer,
    pub fft_size: usize,
    pub window: WindowKind,
    pub averaging: Averaging,
    pub avg_tau: f32,
    pub rta_view: RtaView,
    pub band_frac: u32,
    pub smoothing_frac: f32,
    pub pink_compensate: bool,
    pub show_peak_hold: bool,
    pub rta_show_target: bool,
    pub overlap: f32,
    pub weighting: Weighting,
    pub level: LevelMeter,
    pub level_hist: std::collections::VecDeque<f32>,
    /// The displayed curve is rebuilt only when new audio or new settings arrive,
    /// not on every repaint. Without this the analyser redraws the whole spectrum
    /// 60 times a second for data that only changes a dozen times a second.
    pub rta_cache: RtaCache,
    pub mag_cache: MagCache,

    // ---- signal generator ----
    pub signal: SignalKind,
    pub signal_level_db: f32,
    pub signal_freq: f32,
    pub band_center: f32,
    pub route: Route,
    pub test_playing: bool,

    // ---- measurement ----
    pub sweep_cfg: SweepConfig,
    pub ir_window: IrWindow,
    pub recorder: Recorder,
    pub job: Option<MeasureJob>,
    pub mstate: MeasureState,
    pub measurements: Vec<Measurement>,
    pub selected: Option<usize>,
    pub mode: TuneMode,
    pub system_latency_ms: f32,
    pub measure_view: crate::ui::measure::View,
    pub meas_smoothing: f32,
    pub normalize_traces: bool,

    // ---- correction ----
    pub target: TargetCurve,
    pub fit_cfg: FitConfig,
    pub filters: Vec<FilterSpec>,
    pub preamp_db: f32,
    pub eq_enabled: bool,
    pub fit_result: Option<FitResult>,
    pub eq_sr: f32,
    pub eq_show_error: bool,

    // ---- external EQ unit ----
    pub device_spec: DeviceSpec,
    pub device_result: Option<DeviceResult>,
    pub device_use_average: bool,

    // ---- calibration ----
    pub mic_cal: MicCalibration,
    pub spl_cal: SplCalibration,
    pub spl_reference_input: f32,

    // ---- listening ----
    pub music: Option<AudioFile>,
    pub music_gain_db: f32,
    pub music_playing: bool,

    // ---- shared ----
    pub tab: Tab,
    pub log: Vec<String>,
    pub fft: Fft,
    pub last_repaint: Instant,
}

impl AudioTuneApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::ui::style::install(&cc.egui_ctx);

        let hosts = devices::available_hosts();
        let host_idx = 0usize;
        let device_list = hosts
            .first()
            .map(|h| devices::enumerate(*h))
            .unwrap_or_default();

        let in_dev = device_list
            .inputs
            .first()
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let out_dev = device_list
            .outputs
            .first()
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let sample_rate = device_list
            .outputs
            .first()
            .map(|d| d.default_sample_rate)
            .unwrap_or(48_000);
        let in_channels = device_list
            .inputs
            .first()
            .map(|d| d.default_channels.max(1))
            .unwrap_or(1);
        let out_channels = device_list
            .outputs
            .first()
            .map(|d| d.default_channels.max(1))
            .unwrap_or(2);

        let fft_size = 8_192;
        let analyzer = Analyzer::new(sample_rate as f32, fft_size, WindowKind::Hann);

        Self {
            hosts,
            host_idx,
            device_list,
            in_dev,
            out_dev,
            sample_rate,
            in_channels,
            out_channels,
            in_channel_idx: 0,
            buffer_frames: None,
            engine: None,
            engine_error: None,

            analyzer,
            fft_size,
            window: WindowKind::Hann,
            averaging: Averaging::Exponential { tau: 1.0 },
            avg_tau: 1.0,
            rta_view: RtaView::Spectrum,
            band_frac: 3,
            smoothing_frac: 6.0,
            pink_compensate: true,
            show_peak_hold: false,
            rta_show_target: false,
            overlap: 0.75,
            weighting: Weighting::Z,
            level: LevelMeter::default(),
            level_hist: Default::default(),
            rta_cache: RtaCache::default(),
            mag_cache: MagCache::default(),

            signal: SignalKind::PinkNoise,
            signal_level_db: -20.0,
            signal_freq: 1000.0,
            band_center: 1000.0,
            route: Route::Both,
            test_playing: false,

            sweep_cfg: SweepConfig::default(),
            ir_window: IrWindow::default(),
            recorder: Recorder::new(sample_rate as f32),
            job: None,
            mstate: MeasureState::Idle,
            measurements: Vec::new(),
            selected: None,
            mode: TuneMode::OneSpeaker,
            system_latency_ms: 0.0,
            measure_view: crate::ui::measure::View::Magnitude,
            meas_smoothing: 6.0,
            normalize_traces: true,

            target: TargetCurve::default(),
            fit_cfg: FitConfig::preset_speaker(),
            filters: Vec::new(),
            preamp_db: 0.0,
            eq_enabled: false,
            fit_result: None,
            eq_sr: sample_rate as f32,
            eq_show_error: true,

            device_spec: DeviceSpec::default(),
            device_result: None,
            device_use_average: false,

            mic_cal: MicCalibration::default(),
            spl_cal: SplCalibration::default(),
            spl_reference_input: 75.0,

            music: None,
            music_gain_db: -6.0,
            music_playing: false,

            tab: Tab::Setup,
            log: vec!["Ready. Pick your devices on tab 1, then press Open audio streams.".into()],
            fft: Fft::new(),
            last_repaint: Instant::now(),
        }
        .maybe_autostart()
    }

    /// `--autostart` opens the default devices and goes straight to the analyser,
    /// which saves two clicks when the same rig is used every day.
    fn maybe_autostart(mut self) -> Self {
        if std::env::args().any(|a| a == "--autostart") {
            self.start_engine();
            if self.engine.is_some() {
                self.tab = Tab::Rta;
            }
        }
        self
    }

    pub fn logln(&mut self, s: impl Into<String>) {
        let s = s.into();
        log::info!("{s}");
        self.log.push(s);
        if self.log.len() > 300 {
            self.log.drain(..100);
        }
    }

    // ---------------------------------------------------------------- devices

    pub fn refresh_devices(&mut self) {
        if let Some(h) = self.hosts.get(self.host_idx) {
            self.device_list = devices::enumerate(*h);
            if !self.device_list.inputs.iter().any(|d| d.name == self.in_dev) {
                self.in_dev = self
                    .device_list
                    .inputs
                    .first()
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
            }
            if !self
                .device_list
                .outputs
                .iter()
                .any(|d| d.name == self.out_dev)
            {
                self.out_dev = self
                    .device_list
                    .outputs
                    .first()
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
            }
        }
    }

    pub fn start_engine(&mut self) {
        self.stop_engine();
        let Some(&host) = self.hosts.get(self.host_idx) else {
            self.engine_error = Some("No audio host available".into());
            return;
        };
        let cfg = EngineConfig {
            host,
            input_device: self.in_dev.clone(),
            output_device: self.out_dev.clone(),
            sample_rate_in: self.sample_rate,
            sample_rate_out: self.sample_rate,
            input_channels: self.in_channels.max(1),
            output_channels: self.out_channels.max(1),
            input_channel_index: self.in_channel_idx,
            buffer_frames: self.buffer_frames,
        };
        match Engine::start(&cfg) {
            Ok(e) => {
                let (sr_in, sr_out) = (e.sr_in, e.sr_out);
                self.analyzer.reconfigure(sr_in, self.fft_size, self.window);
                self.recorder = Recorder::new(sr_in);
                self.eq_sr = sr_out;
                self.engine = Some(e);
                self.engine_error = None;
                self.logln(format!(
                    "Streams open: in \"{}\" / out \"{}\" at {} Hz, {} in, {} out.",
                    self.in_dev, self.out_dev, self.sample_rate, self.in_channels, self.out_channels
                ));
                self.push_eq();
            }
            Err(e) => {
                let msg = format!("{e:#}");
                self.engine_error = Some(msg.clone());
                self.logln(format!("Failed to open streams: {msg}"));
            }
        }
    }

    pub fn stop_engine(&mut self) {
        if self.engine.is_some() {
            self.engine = None;
            self.test_playing = false;
            self.music_playing = false;
            self.job = None;
            self.mstate = MeasureState::Idle;
            self.recorder.cancel();
            self.logln("Audio streams closed.");
        }
    }

    // ---------------------------------------------------- test signal output

    pub fn make_source(&self) -> Source {
        let amp = 10f32.powf(self.signal_level_db / 20.0);
        match self.signal {
            SignalKind::PinkNoise => Source::Pink {
                gen: PinkNoise::default(),
                amp,
            },
            SignalKind::WhiteNoise => Source::White {
                rng: Rng::default(),
                amp,
            },
            SignalKind::Sine => Source::Sine {
                phase: 0.0,
                step: 2.0 * std::f32::consts::PI * self.signal_freq
                    / self.engine.as_ref().map(|e| e.sr_out).unwrap_or(48_000.0),
                amp,
            },
            SignalKind::PinkBand => {
                let sr = self.engine.as_ref().map(|e| e.sr_out).unwrap_or(48_000.0);
                let spec = crate::dsp::biquad::FilterSpec {
                    kind: crate::dsp::biquad::FilterKind::BandPass,
                    freq: self.band_center,
                    gain_db: 0.0,
                    q: 2.0,
                    enabled: true,
                };
                Source::PinkBand {
                    gen: PinkNoise::default(),
                    bp: crate::dsp::biquad::Biquad::new(crate::dsp::biquad::Coeffs::design(
                        &spec, sr,
                    )),
                    // Make up for the energy the band-pass throws away.
                    amp: amp * 3.0,
                }
            }
            SignalKind::LogSweep => {
                let sr = self.engine.as_ref().map(|e| e.sr_out).unwrap_or(48_000.0);
                let data = generator::log_sweep(&self.sweep_cfg, sr);
                Source::Buffer {
                    data: Arc::new(data),
                    pos: 0,
                    repeat: true,
                    amp: 1.0,
                }
            }
        }
    }

    pub fn start_test_signal(&mut self) {
        let src = self.make_source();
        let route = self.route;
        if let Some(e) = &self.engine {
            e.play_source(src, route);
            self.test_playing = true;
        }
    }

    pub fn stop_test_signal(&mut self) {
        if let Some(e) = &self.engine {
            e.stop_playback();
        }
        self.test_playing = false;
        self.music_playing = false;
    }

    // ------------------------------------------------------------ measuring

    pub fn routes_for_mode(&self) -> Vec<(Route, String)> {
        match self.mode {
            TuneMode::OneSpeaker => vec![(self.route, self.route.label())],
            TuneMode::Stereo => vec![
                (Route::Left, "Left".to_string()),
                (Route::Right, "Right".to_string()),
            ],
            TuneMode::Headphone => vec![
                (Route::Left, "Left ear".to_string()),
                (Route::Right, "Right ear".to_string()),
            ],
        }
    }

    pub fn start_measurement(&mut self, routes: Vec<(Route, String)>) {
        let Some(engine) = &self.engine else {
            self.logln("Audio streams are not open.");
            return;
        };
        if routes.is_empty() {
            return;
        }
        let cfg = self.sweep_cfg.clone();
        let sweep_out = Arc::new(generator::log_sweep(&cfg, engine.sr_out));
        // The inverse filter is built at the INPUT sample rate, so two devices running
        // at different rates still deconvolve correctly.
        let sweep_in = generator::log_sweep(&cfg, engine.sr_in);
        let inverse_in = generator::inverse_filter(&sweep_in, &cfg, engine.sr_in);

        self.job = Some(MeasureJob {
            routes,
            route_idx: 0,
            rep: 0,
            captures: Vec::new(),
            sweep_out,
            inverse_in,
            cfg,
            deadline: Instant::now() + Duration::from_secs(60),
        });
        self.mstate = MeasureState::Countdown {
            until: Instant::now() + Duration::from_millis(600),
        };
        self.logln("Measurement starting - keep the room quiet.");
    }

    pub fn cancel_measurement(&mut self) {
        self.job = None;
        self.mstate = MeasureState::Idle;
        self.recorder.cancel();
        if let Some(e) = &self.engine {
            e.stop_playback();
        }
        self.logln("Measurement cancelled.");
    }

    fn begin_capture(&mut self) {
        let sr_in = match &self.engine {
            Some(e) => e.sr_in,
            None => return,
        };
        let (route, sweep, total_s) = {
            let Some(job) = &self.job else { return };
            let route = job.routes[job.route_idx].0;
            let total = job.cfg.duration_s + job.cfg.tail_s + 0.6;
            (route, job.sweep_out.clone(), total)
        };
        // Throw away whatever is still queued so old audio cannot leak in.
        if let Some(e) = &self.engine {
            while e.in_rx.try_recv().is_ok() {}
        }
        let target = (total_s * sr_in) as usize;
        self.recorder.arm(target, 0);
        if let Some(e) = &self.engine {
            e.play_buffer(sweep, route, false, 1.0);
        }
        if let Some(job) = &mut self.job {
            job.deadline = Instant::now() + Duration::from_secs_f32(total_s + 10.0);
        }
        self.mstate = MeasureState::Capturing;
    }

    fn finish_route(&mut self) {
        let Some(job) = &mut self.job else { return };
        let captures = std::mem::take(&mut job.captures);
        let cfg = job.cfg.clone();
        let inverse = job.inverse_in.clone();
        let (route, name) = job.routes[job.route_idx].clone();
        let sr_in = self.engine.as_ref().map(|e| e.sr_in).unwrap_or(48_000.0);

        let recorded = measurement::average_captures(&captures);
        if recorded.is_empty() {
            self.logln("Capture was empty - skipped.");
        } else {
            let peak = recorded.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            let ir = measurement::deconvolve(&recorded, &inverse, sr_in, &mut self.fft);
            let mut fr = measurement::ir_to_fr(&ir, &self.ir_window, &mut self.fft);
            self.mic_cal.apply(&fr.freqs, &mut fr.mag_db);
            let decay = measurement::decay_times(&ir);
            let distortion = measurement::harmonic_distortion(&ir, &cfg, 5);
            let snr_db = measurement::estimate_snr_db(&ir);

            let idx = self.measurements.len();
            let m = Measurement {
                name: format!("{} #{}", name, idx + 1),
                timestamp: timestamp_now(),
                channel: route.label(),
                sr: sr_in,
                sweep: cfg,
                ir,
                fr,
                decay,
                distortion,
                snr_db,
                peak_dbfs: crate::dsp::lin_to_db(peak),
                clipped: peak >= 0.999,
                visible: true,
                color_idx: idx,
            };
            self.logln(format!(
                "\"{}\" done: input peak {:.1} dBFS, SNR ~{:.0} dB, THD ~{:.2} %, delay {:.1} ms.",
                m.name,
                m.peak_dbfs,
                m.snr_db,
                m.distortion.thd_percent,
                m.ir.delay_ms()
            ));
            if m.clipped {
                self.logln("WARNING: the input clipped. Lower the mic gain or the sweep level.");
            }
            self.measurements.push(m);
            self.selected = Some(self.measurements.len() - 1);
        }

        let Some(job) = &mut self.job else { return };
        job.route_idx += 1;
        job.rep = 0;
        if job.route_idx >= job.routes.len() {
            self.job = None;
            self.mstate = MeasureState::Idle;
            self.tab = Tab::Measure;
            self.logln("All measurements complete.");
        } else {
            self.mstate = MeasureState::Gap {
                until: Instant::now() + Duration::from_millis(900),
            };
        }
    }

    fn tick_measurement(&mut self) {
        if self.job.is_none() {
            return;
        }
        let now = Instant::now();
        match self.mstate {
            MeasureState::Idle => {}
            MeasureState::Countdown { until } | MeasureState::Gap { until } => {
                if now >= until {
                    self.begin_capture();
                }
            }
            MeasureState::Capturing => {
                let done = self.recorder.state == RecordState::Done;
                let expired = self.job.as_ref().map(|j| now >= j.deadline).unwrap_or(true);
                if done {
                    let data = self.recorder.take();
                    let (rep, repeats) = {
                        let job = self.job.as_mut().unwrap();
                        job.captures.push(data);
                        job.rep += 1;
                        (job.rep, job.cfg.repeats.max(1))
                    };
                    if rep < repeats {
                        self.mstate = MeasureState::Gap {
                            until: now + Duration::from_millis(500),
                        };
                        self.logln(format!("Pass {}/{} done.", rep, repeats));
                    } else {
                        self.finish_route();
                    }
                } else if expired {
                    self.logln("Timed out waiting for audio - check the input device.");
                    self.cancel_measurement();
                }
            }
        }
    }

    // ----------------------------------------------------------- correction

    /// The common frequency grid every EQ calculation runs on.
    pub fn eq_grid(&self) -> Vec<f32> {
        let f_max = (self.eq_sr * 0.48).min(24_000.0);
        octave::log_grid(10.0, f_max, self.fit_cfg.ppo.max(12))
    }

    /// The selected measurement, smoothed and levelled to 0 dB in the midband.
    pub fn measured_curve(&self, grid: &[f32]) -> Option<Vec<f32>> {
        let idx = self.selected?;
        let m = self.measurements.get(idx)?;
        let mut curve = m.fr.smoothed_on_grid(grid, self.fit_cfg.smoothing_frac);
        let mid = mean_band(grid, &curve, 200.0, 2000.0);
        for v in curve.iter_mut() {
            *v -= mid;
        }
        Some(curve)
    }

    /// Power average of every visible measurement, for one shared correction.
    pub fn averaged_curve(&self, grid: &[f32]) -> Option<Vec<f32>> {
        let visible: Vec<&Measurement> = self.measurements.iter().filter(|m| m.visible).collect();
        if visible.is_empty() {
            return None;
        }
        let mut acc = vec![0.0f64; grid.len()];
        for m in &visible {
            let c = m.fr.smoothed_on_grid(grid, self.fit_cfg.smoothing_frac);
            for (a, v) in acc.iter_mut().zip(c.iter()) {
                *a += 10f64.powf(*v as f64 / 10.0);
            }
        }
        let n = visible.len() as f64;
        let mut curve: Vec<f32> = acc
            .iter()
            .map(|a| (10.0 * (a / n).max(1e-30).log10()) as f32)
            .collect();
        let mid = mean_band(grid, &curve, 200.0, 2000.0);
        for v in curve.iter_mut() {
            *v -= mid;
        }
        Some(curve)
    }

    /// The correction the system needs: target minus what was measured.
    /// This is what gets dialled into an external EQ.
    pub fn desired_correction(&self, grid: &[f32], use_average: bool) -> Option<Vec<f32>> {
        let measured = if use_average {
            self.averaged_curve(grid)?
        } else {
            self.measured_curve(grid)?
        };
        let target = self.target.on_grid(grid);
        let diff: Vec<f32> = target
            .iter()
            .zip(measured.iter())
            .map(|(t, m)| t - m)
            .collect();
        // Remove the overall level difference; EQ should not change the volume.
        let offset = mean_band(grid, &diff, 200.0, 2000.0);
        Some(diff.iter().map(|d| d - offset).collect())
    }

    pub fn run_autofit(&mut self, use_average: bool) {
        let grid = self.eq_grid();
        let measured = if use_average {
            self.averaged_curve(&grid)
        } else {
            self.measured_curve(&grid)
        };
        let Some(measured) = measured else {
            self.logln("No measurement to fit an EQ to yet.");
            return;
        };
        let target = self.target.on_grid(&grid);
        let res = autoeq::fit(&grid, &measured, &target, self.eq_sr, &self.fit_cfg);
        self.logln(format!(
            "EQ fitted: {} filters, RMS error {:.2} dB -> {:.2} dB, preamp {:.1} dB.",
            res.filters.len(),
            res.rms_before,
            res.rms_after,
            res.preamp_db
        ));
        self.filters = res.filters.clone();
        self.preamp_db = res.preamp_db;
        self.fit_result = Some(res);
        self.push_eq();
    }

    pub fn suggest_limits(&mut self) {
        let grid = self.eq_grid();
        if let Some(measured) = self.measured_curve(&grid) {
            let lo = autoeq::suggest_low_limit(&grid, &measured, 10.0);
            self.fit_cfg.f_min = lo;
            self.logln(format!(
                "Suggested low limit: {lo:.0} Hz - below this the speaker has already run out."
            ));
        }
    }

    pub fn push_eq(&mut self) {
        if let Some(e) = &self.engine {
            e.apply_eq(&self.filters, self.preamp_db, self.eq_enabled);
        }
    }

    pub fn recompute_preamp(&mut self) {
        self.preamp_db = autoeq::required_preamp_db(&self.filters, self.eq_sr, 0.5);
    }

    // ------------------------------------------------- external EQ settings

    /// Work out the numbers to dial into the external EQ unit.
    pub fn compute_device_settings(&mut self) {
        let grid = self.eq_grid();
        let Some(desired) = self.desired_correction(&grid, self.device_use_average) else {
            self.logln("Measure something first - there is nothing to correct yet.");
            return;
        };
        let (f_min, f_max) = (self.fit_cfg.f_min, self.fit_cfg.f_max);
        let res = match self.device_spec.kind {
            DeviceKind::Graphic => {
                device_eq::fit_graphic(&grid, &desired, &self.device_spec, self.eq_sr, f_min, f_max)
            }
            DeviceKind::Parametric => {
                // A parametric box needs a filter set first, so fit one sized to it.
                let mut cfg = self.fit_cfg.clone();
                cfg.max_filters = self.device_spec.parametric_bands;
                cfg.max_boost_db = cfg.max_boost_db.min(self.device_spec.max_boost_db);
                cfg.max_cut_db = cfg.max_cut_db.min(self.device_spec.max_cut_db);
                let measured = if self.device_use_average {
                    self.averaged_curve(&grid)
                } else {
                    self.measured_curve(&grid)
                };
                let Some(measured) = measured else { return };
                let target = self.target.on_grid(&grid);
                let fit = autoeq::fit(&grid, &measured, &target, self.eq_sr, &cfg);
                self.filters = fit.filters.clone();
                self.fit_result = Some(fit.clone());
                device_eq::fit_parametric(
                    &grid,
                    &desired,
                    &fit.filters,
                    &self.device_spec,
                    self.eq_sr,
                    f_min,
                    f_max,
                )
            }
        };
        let used = res.active().count();
        self.logln(format!(
            "{} settings ready: {} band(s) in use, RMS error {:.2} dB -> {:.2} dB.",
            self.device_spec.name, used, res.rms_error_before, res.rms_error_after
        ));
        if res.any_limited() {
            self.logln(
                "Some bands hit the unit's limit. Move the speaker or treat the room instead of \
                 pushing harder.",
            );
        }
        self.device_result = Some(res);
    }

    /// Load the quantised device settings into the app's own EQ, so A/B listening
    /// reflects exactly what the hardware will do.
    pub fn preview_device_settings(&mut self) {
        let Some(res) = self.device_result.clone() else {
            self.logln("Compute the settings first.");
            return;
        };
        self.filters = res.to_filter_specs(&self.device_spec);
        self.recompute_preamp();
        self.eq_enabled = true;
        self.push_eq();
        self.logln(format!(
            "Previewing the {} settings through the app's own EQ.",
            self.device_spec.name
        ));
    }

    // ------------------------------------------------------------- project

    pub fn project(&self) -> Project {
        Project {
            version: Project::VERSION,
            mode: Some(self.mode),
            measurements: self.measurements.clone(),
            filters: self.filters.clone(),
            preamp_db: self.preamp_db,
            target: Some(self.target.clone()),
            mic_cal: Some(self.mic_cal.clone()),
            spl_cal: Some(self.spl_cal),
            device: Some(self.device_spec.clone()),
            note: String::new(),
        }
    }

    pub fn load_project(&mut self, p: Project) {
        self.measurements = p.measurements;
        self.filters = p.filters;
        self.preamp_db = p.preamp_db;
        if let Some(t) = p.target {
            self.target = t;
        }
        if let Some(c) = p.mic_cal {
            self.mic_cal = c;
        }
        if let Some(s) = p.spl_cal {
            self.spl_cal = s;
        }
        if let Some(m) = p.mode {
            self.mode = m;
        }
        if let Some(d) = p.device {
            self.device_spec = d;
        }
        self.selected = if self.measurements.is_empty() {
            None
        } else {
            Some(0)
        };
        self.fit_result = None;
        self.device_result = None;
        self.push_eq();
        self.logln("Project loaded.");
    }

    // --------------------------------------------------------- main loop

    fn drain_audio(&mut self) {
        let Some(engine) = &self.engine else { return };
        let sr = engine.sr_in;
        self.analyzer.reconfigure(sr, self.fft_size, self.window);
        self.analyzer.averaging = self.averaging;
        self.analyzer.peak_hold = self.show_peak_hold;
        self.analyzer.overlap = self.overlap;

        let mut blocks: Vec<Vec<f32>> = Vec::new();
        while let Ok(b) = engine.in_rx.try_recv() {
            blocks.push(b);
            if blocks.len() > 200 {
                break;
            }
        }
        for b in &blocks {
            if self.recorder.is_active() {
                self.recorder.push(b);
            }
            self.analyzer.push(b);
            let lm = crate::dsp::spectrum::measure_level(b);
            self.level = lm;
            self.level_hist.push_back(lm.rms_db);
            while self.level_hist.len() > 600 {
                self.level_hist.pop_front();
            }
        }

        if self.test_playing && !engine.is_playing() {
            let (_, total) = engine.progress();
            if total > 0 {
                self.test_playing = false;
            }
        }

        let errs = engine.take_errors();
        for e in errs {
            self.logln(format!("Stream error: {e}"));
        }
    }
}

/// Plain average of a curve across a frequency band.
pub fn mean_band(freqs: &[f32], vals: &[f32], lo: f32, hi: f32) -> f32 {
    let mut s = 0.0f64;
    let mut c = 0usize;
    for (f, v) in freqs.iter().zip(vals.iter()) {
        if *f >= lo && *f <= hi && v.is_finite() {
            s += *v as f64;
            c += 1;
        }
    }
    if c == 0 {
        0.0
    } else {
        (s / c as f64) as f32
    }
}

impl eframe::App for AudioTuneApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_audio();
        self.tick_measurement();

        crate::ui::draw(self, ctx);

        // Only the live analyser and a running measurement need a fast refresh.
        // The other tabs show data that changes when you press something, so
        // repainting them 30 times a second is pure heat.
        if self.engine.is_some() {
            let fast = self.job.is_some() || self.tab == Tab::Rta;
            ctx.request_repaint_after(Duration::from_millis(if fast { 33 } else { 150 }));
        }
        self.last_repaint = Instant::now();
    }
}
