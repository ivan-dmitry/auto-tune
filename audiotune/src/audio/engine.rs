//! The audio engine: opens cpal streams, plays test signals, collects captures.
//!
//! Data-flow rules:
//!   * The audio callback never locks a mutex. Commands arrive over a crossbeam
//!     channel, captured samples leave over another, status numbers are atomics.
//!   * The UI drains the capture channel each frame and analyses afterwards.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::dsp::biquad::{Bank, FilterSpec};
use crate::dsp::generator::Source;

/// Which output channels to play to.
#[derive(Copy, Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Route {
    Both,
    Left,
    Right,
    /// Address a channel index directly, for interfaces with many outputs.
    Channel(usize),
}

impl Route {
    pub fn label(&self) -> String {
        match self {
            Route::Both => "Both (L+R)".into(),
            Route::Left => "Left".into(),
            Route::Right => "Right".into(),
            Route::Channel(i) => format!("Channel {}", i + 1),
        }
    }

    fn includes(&self, ch: usize) -> bool {
        match self {
            Route::Both => true,
            Route::Left => ch == 0,
            Route::Right => ch == 1,
            Route::Channel(i) => ch == *i,
        }
    }
}

/// What is currently playing.
pub enum Playback {
    /// A mono source (a test signal), routed to channels according to Route.
    Mono(Source),
    /// Stereo music, for A/B listening.
    Stereo {
        data: Arc<Vec<[f32; 2]>>,
        pos: usize,
        repeat: bool,
        amp: f32,
    },
}

pub enum OutCmd {
    Play(Box<Playback>),
    Stop,
    SetRoute(Route),
    /// Load a new EQ; coefficients are prebuilt so the callback does no maths.
    SetEq(Vec<Bank>),
    SetEqEnabled(bool),
}

/// Status numbers shared between the callback and the UI.
#[derive(Default)]
pub struct Shared {
    pub out_frames: AtomicU64,
    pub out_total: AtomicU64,
    pub playing: AtomicBool,
    pub out_peak_milli: AtomicU64,
    pub in_frames: AtomicU64,
    pub input_channel: AtomicUsize,
    pub xruns: AtomicU64,
}

pub struct EngineConfig {
    pub host: cpal::HostId,
    pub input_device: String,
    pub output_device: String,
    pub sample_rate_in: u32,
    pub sample_rate_out: u32,
    pub input_channels: u16,
    pub output_channels: u16,
    pub input_channel_index: usize,
    pub buffer_frames: Option<u32>,
}

pub struct Engine {
    _in_stream: Option<cpal::Stream>,
    _out_stream: Option<cpal::Stream>,
    pub in_rx: Receiver<Vec<f32>>,
    pub out_tx: Sender<OutCmd>,
    pub shared: Arc<Shared>,
    pub sr_in: f32,
    pub sr_out: f32,
    pub ch_in: u16,
    pub ch_out: u16,
    pub errors: Arc<std::sync::Mutex<Vec<String>>>,
}

struct OutState {
    playback: Option<Playback>,
    route: Route,
    gain: f32,
    banks: Vec<Bank>,
    eq_on: bool,
}

fn err_sink(errors: Arc<std::sync::Mutex<Vec<String>>>, tag: &'static str) -> impl FnMut(cpal::StreamError) {
    move |e| {
        if let Ok(mut v) = errors.lock() {
            if v.len() < 50 {
                v.push(format!("[{tag}] {e}"));
            }
        }
    }
}

impl Engine {
    pub fn start(cfg: &EngineConfig) -> Result<Self> {
        let in_dev = super::devices::find_device(cfg.host, &cfg.input_device, true)
            .ok_or_else(|| anyhow!("Input device not found: {}", cfg.input_device))?;
        let out_dev = super::devices::find_device(cfg.host, &cfg.output_device, false)
            .ok_or_else(|| anyhow!("Output device not found: {}", cfg.output_device))?;

        let shared = Arc::new(Shared::default());
        shared
            .input_channel
            .store(cfg.input_channel_index, Ordering::Relaxed);
        let errors: Arc<std::sync::Mutex<Vec<String>>> = Default::default();

        let buffer_size = match cfg.buffer_frames {
            Some(n) => cpal::BufferSize::Fixed(n),
            None => cpal::BufferSize::Default,
        };

        // ---------- input stream ----------
        let in_conf = StreamConfig {
            channels: cfg.input_channels,
            sample_rate: cpal::SampleRate(cfg.sample_rate_in),
            buffer_size,
        };
        let in_fmt = super::devices::pick_format(
            &in_dev,
            true,
            cfg.input_channels,
            cfg.sample_rate_in,
        )
        .unwrap_or(SampleFormat::F32);

        // Bounded so memory cannot balloon if the UI freezes.
        let (in_tx, in_rx) = bounded::<Vec<f32>>(512);
        let sh = shared.clone();
        let ch_in = cfg.input_channels as usize;
        let in_stream = build_input_stream(
            &in_dev,
            &in_conf,
            in_fmt,
            ch_in,
            in_tx,
            sh.clone(),
            errors.clone(),
        )
        .context("Could not open the input stream")?;

        // ---------- output stream ----------
        let out_conf = StreamConfig {
            channels: cfg.output_channels,
            sample_rate: cpal::SampleRate(cfg.sample_rate_out),
            buffer_size,
        };
        let out_fmt = super::devices::pick_format(
            &out_dev,
            false,
            cfg.output_channels,
            cfg.sample_rate_out,
        )
        .unwrap_or(SampleFormat::F32);

        let (out_tx, out_rx) = bounded::<OutCmd>(64);
        let out_stream = build_output_stream(
            &out_dev,
            &out_conf,
            out_fmt,
            cfg.output_channels as usize,
            out_rx,
            shared.clone(),
            errors.clone(),
        )
        .context("Could not open the output stream")?;

        in_stream.play().context("Could not start the input stream")?;
        out_stream.play().context("Could not start the output stream")?;

        Ok(Self {
            _in_stream: Some(in_stream),
            _out_stream: Some(out_stream),
            in_rx,
            out_tx,
            shared,
            sr_in: cfg.sample_rate_in as f32,
            sr_out: cfg.sample_rate_out as f32,
            ch_in: cfg.input_channels,
            ch_out: cfg.output_channels,
            errors,
        })
    }

    pub fn set_input_channel(&self, idx: usize) {
        self.shared.input_channel.store(idx, Ordering::Relaxed);
    }

    pub fn send(&self, cmd: OutCmd) {
        let _ = self.out_tx.try_send(cmd);
    }

    pub fn stop_playback(&self) {
        self.send(OutCmd::Stop);
    }

    pub fn play_buffer(&self, data: Arc<Vec<f32>>, route: Route, repeat: bool, amp: f32) {
        self.shared
            .out_total
            .store(data.len() as u64, Ordering::Relaxed);
        self.shared.out_frames.store(0, Ordering::Relaxed);
        self.send(OutCmd::SetRoute(route));
        self.send(OutCmd::Play(Box::new(Playback::Mono(Source::Buffer {
            data,
            pos: 0,
            repeat,
            amp,
        }))));
    }

    pub fn play_source(&self, src: Source, route: Route) {
        self.shared.out_total.store(0, Ordering::Relaxed);
        self.shared.out_frames.store(0, Ordering::Relaxed);
        self.send(OutCmd::SetRoute(route));
        self.send(OutCmd::Play(Box::new(Playback::Mono(src))));
    }

    pub fn play_stereo(&self, data: Arc<Vec<[f32; 2]>>, repeat: bool, amp: f32) {
        self.shared
            .out_total
            .store(data.len() as u64, Ordering::Relaxed);
        self.shared.out_frames.store(0, Ordering::Relaxed);
        self.send(OutCmd::Play(Box::new(Playback::Stereo {
            data,
            pos: 0,
            repeat,
            amp,
        })));
    }

    /// Load the EQ into every output channel.
    pub fn apply_eq(&self, specs: &[FilterSpec], preamp_db: f32, enabled: bool) {
        let banks: Vec<Bank> = (0..self.ch_out.max(1) as usize)
            .map(|_| Bank::from_specs(specs, preamp_db, self.sr_out))
            .collect();
        self.send(OutCmd::SetEq(banks));
        self.send(OutCmd::SetEqEnabled(enabled));
    }

    pub fn is_playing(&self) -> bool {
        self.shared.playing.load(Ordering::Relaxed)
    }

    pub fn progress(&self) -> (u64, u64) {
        (
            self.shared.out_frames.load(Ordering::Relaxed),
            self.shared.out_total.load(Ordering::Relaxed),
        )
    }

    pub fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

fn build_input_stream(
    dev: &cpal::Device,
    conf: &StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    tx: Sender<Vec<f32>>,
    shared: Arc<Shared>,
    errors: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<cpal::Stream> {
    macro_rules! make {
        ($t:ty, $conv:expr) => {{
            let sh = shared.clone();
            let cb = move |data: &[$t], _: &cpal::InputCallbackInfo| {
                let ch = channels.max(1);
                let sel = sh.input_channel.load(Ordering::Relaxed).min(ch - 1);
                let frames = data.len() / ch;
                let mut mono = Vec::with_capacity(frames);
                for f in 0..frames {
                    let v: $t = data[f * ch + sel];
                    mono.push($conv(v));
                }
                sh.in_frames.fetch_add(frames as u64, Ordering::Relaxed);
                if tx.try_send(mono).is_err() {
                    sh.xruns.fetch_add(1, Ordering::Relaxed);
                }
            };
            dev.build_input_stream(conf, cb, err_sink(errors.clone(), "in"), None)?
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => make!(f32, |v: f32| v),
        SampleFormat::I16 => make!(i16, |v: i16| v as f32 / 32768.0),
        SampleFormat::U16 => make!(u16, |v: u16| (v as f32 - 32768.0) / 32768.0),
        SampleFormat::I32 => make!(i32, |v: i32| v as f32 / 2147483648.0),
        other => return Err(anyhow!("Unsupported sample format: {other:?}")),
    };
    Ok(stream)
}

fn build_output_stream(
    dev: &cpal::Device,
    conf: &StreamConfig,
    fmt: SampleFormat,
    channels: usize,
    rx: Receiver<OutCmd>,
    shared: Arc<Shared>,
    errors: Arc<std::sync::Mutex<Vec<String>>>,
) -> Result<cpal::Stream> {
    macro_rules! make {
        ($t:ty, $conv:expr) => {{
            let sh = shared.clone();
            let mut st = OutState {
                playback: None,
                route: Route::Both,
                gain: 1.0,
                banks: Vec::new(),
                eq_on: false,
            };
            let cb = move |data: &mut [$t], _: &cpal::OutputCallbackInfo| {
                while let Ok(cmd) = rx.try_recv() {
                    match cmd {
                        OutCmd::Play(p) => {
                            for b in st.banks.iter_mut() {
                                b.reset();
                            }
                            st.playback = Some(*p);
                            sh.playing.store(true, Ordering::Relaxed);
                        }
                        OutCmd::Stop => {
                            st.playback = None;
                            sh.playing.store(false, Ordering::Relaxed);
                        }
                        OutCmd::SetRoute(r) => st.route = r,
                        OutCmd::SetEq(b) => st.banks = b,
                        OutCmd::SetEqEnabled(e) => st.eq_on = e,
                    }
                }

                let ch = channels.max(1);
                let frames = data.len() / ch;
                let mut peak = 0.0f32;
                let mut finished = false;

                for f in 0..frames {
                    // Fetch one frame, up to 2 source channels.
                    let mut sample = [0.0f32; 2];
                    let mut stereo_mode = false;
                    let mut ended = false;
                    match st.playback.as_mut() {
                        None => {}
                        Some(Playback::Mono(src)) => match src.next() {
                            Some(v) => sample = [v, v],
                            None => ended = true,
                        },
                        Some(Playback::Stereo {
                            data: buf,
                            pos,
                            repeat,
                            amp,
                        }) => {
                            if *pos >= buf.len() && *repeat && !buf.is_empty() {
                                *pos = 0;
                            }
                            if *pos < buf.len() {
                                stereo_mode = true;
                                let s = buf[*pos];
                                sample = [s[0] * *amp, s[1] * *amp];
                                *pos += 1;
                            } else {
                                ended = true;
                            }
                        }
                    }
                    if ended {
                        st.playback = None;
                        finished = true;
                        sh.playing.store(false, Ordering::Relaxed);
                    }

                    for c in 0..ch {
                        let raw = if stereo_mode {
                            sample[c.min(1)]
                        } else if st.route.includes(c) {
                            sample[0]
                        } else {
                            0.0
                        };
                        // Keep running silence through the EQ so its tail decays smoothly.
                        let mut v = raw;
                        if st.eq_on {
                            if let Some(b) = st.banks.get_mut(c) {
                                v = b.process(v);
                            }
                        }
                        v *= st.gain;
                        let a = v.abs();
                        if a > peak {
                            peak = a;
                        }
                        data[f * ch + c] = $conv(v.clamp(-1.0, 1.0));
                    }
                }
                let _ = finished;
                sh.out_frames.fetch_add(frames as u64, Ordering::Relaxed);
                sh.out_peak_milli
                    .store((peak * 1000.0) as u64, Ordering::Relaxed);
            };
            dev.build_output_stream(conf, cb, err_sink(errors.clone(), "out"), None)?
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => make!(f32, |v: f32| v),
        SampleFormat::I16 => make!(i16, |v: f32| (v * 32767.0) as i16),
        SampleFormat::U16 => make!(u16, |v: f32| ((v * 32767.0) + 32768.0) as u16),
        SampleFormat::I32 => make!(i32, |v: f32| (v * 2147483647.0) as i32),
        other => return Err(anyhow!("Unsupported sample format: {other:?}")),
    };
    Ok(stream)
}
