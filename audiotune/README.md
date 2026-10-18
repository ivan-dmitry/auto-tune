# AudioTune

A desktop application for measuring a loudspeaker or headphone with a microphone and
working out the EQ settings that bring it onto a target curve. Written in Rust, runs on
Windows (WASAPI) and Linux (ALSA / JACK) from the same source.

The correction does not have to be applied inside the app. Its main output is a table
of numbers to dial into a separate EQ unit — a 31-band graphic, a rack parametric, a
DSP1462 or similar — quantised to what that unit can actually be set to.

## What it does

**Measurement**
- Farina exponential (log) sweep, deconvolved to an impulse response
- Frequency response, phase, impulse, Schroeder decay (EDT / T20 / T30 / RT60)
- Harmonic distortion per order (H2…H6) and total THD, extracted from the sweep
- Signal-to-noise estimate, clip detection, arrival delay and distance
- Multi-pass averaging (+3 dB SNR per doubling of passes)
- Selectable IR gating: long window for the listening seat, short window for the
  speaker on its own
- Input and output may run at different sample rates; the inverse filter is
  synthesised at the input rate so deconvolution stays correct

**Live analysis**
- Welch-averaged RTA with selectable FFT size, window and averaging mode
- Full spectrum or 1/1 … 1/24 octave bands, 1/N-octave smoothing, peak hold
- Pink-noise compensation so pink noise reads as a flat line
- A / C / Z weighting, dB SPL once calibrated
- Test signals: pink, white, sine, single-band pink, looping sweep
- Target curve overlaid, for tuning band by band by eye

**Correction**
- Target curves: flat, straight tilt, Harman in-room, Harman OE (approx.), cinema
  X-curve, or a custom curve from file
- Automatic parametric fit: greedy placement then coordinate refinement, with limits
  on gain, Q, frequency range and how hard to correct
- Preamp calculated so the combined response never exceeds 0 dB
- A/B audition through the app's own filters with a WAV file

**EQ unit settings (the part that matters if your EQ is a separate box)**
- Describe your unit: graphic (31-band 1/3 oct, 15-band, 10-band octave, the 10-band
  "<31 … >16K" grid media players use, or a custom grid of up to 32 bands) or
  parametric with any band count;
  gain limit, gain step, Q or bandwidth labelling
- The app solves for the band values whose *combined* response matches the correction.
  Graphic EQ bands overlap heavily, so copying the error curve band by band overshoots;
  solving does not.
- Values are rounded to the unit's own step, and the prediction is computed from the
  rounded values as a real filter cascade — so what the plot shows is what you will get
- Output trim suggested so the boosts cannot clip the unit
- Audition the exact quantised settings through the app before touching the hardware
- Export as a printable sheet or CSV

**Calibration**
- Microphone .cal / .frd / .txt files (REW, miniDSP UMIK, Dayton format)
- SPL calibration against a meter
- System loopback latency, so the reported distance is the acoustic one

**Modes**
- One speaker, stereo (measures L then R and reports level / timing mismatch), and
  headphones

## Building

The project needs nothing but a Rust toolchain.

```bash
cargo build --release
```

On this machine the toolchain lives under `B:\Bussiness\dsp\.toolchain`, so load it
first:

```bash
cd B:/Bussiness/dsp && . ./env.sh && cd audiotune && cargo build --release
```

Or in PowerShell:

```powershell
cd B:\Bussiness\dsp; . .\env.ps1; cd audiotune; cargo build --release
```

### Linux notes

ALSA development headers are needed for cpal, and a windowing stack for eframe:

```bash
sudo apt install libasound2-dev libgl1-mesa-dev libxkbcommon-dev pkg-config
```

For file dialogs, rfd uses the XDG desktop portal, which is present on any normal
desktop. JACK is picked up automatically if it is running.

## Command line

```bash
audiotune                  # start the GUI
audiotune --list-devices   # print every host and device cpal can see
audiotune --autostart      # open the default devices and jump to the analyser
audiotune --selftest       # open the real streams, record 1.5 s of ambient noise
                           # and report the level and noise floor (plays nothing)
```

`--selftest` is the quickest way to prove the capture path works before touching the
GUI.

## Tests

```bash
cargo test
```

53 tests cover the DSP core, and they are not smoke tests — they check the maths
against known answers:

- Filter responses against the RBJ formulas, and the running filter against its own
  computed response
- A full-scale sine reading exactly 0 dBFS through the analyser
- The sweep deconvolved against its own inverse filter producing a clean impulse
- An injected delay being measured back to within 2 samples
- A round trip: sweep through a known filter, measured response recovers it to under
  2 dB across 40 Hz – 16 kHz
- RT60 recovered from a synthetic exponential tail to within 60 ms
- Distortion near zero on a linear path, and high when the path is clipped
- Auto EQ flattening a deformed response and respecting its limits
- The graphic EQ solver beating naive per-band copying, staying inside the unit's
  limits, landing on its gain step, and its prediction matching the filter cascade

One of these caught a real bug during development: the inverse sweep filter's envelope
was inverted, which tilted every measurement by -12 dB/octave.

## Layout

```
src/
  main.rs            entry point, --list-devices and --selftest
  app.rs             application state, measurement state machine
  dsp/               pure signal processing, no GUI or OS dependency
    biquad.rs        RBJ filter design and runtime
    generator.rs     sweep, inverse filter, pink and white noise
    measurement.rs   deconvolution, IR gating, decay, distortion
    spectrum.rs      Welch RTA
    octave.rs        banding, 1/N-octave smoothing, log interpolation
    autoeq.rs        automatic parametric fit
    device_eq.rs     solving for an external EQ unit's controls
    target.rs        target curves
    calibration.rs   mic and SPL calibration, A/C weighting
    fft.rs, window.rs
    tests.rs
  audio/             cpal streams, capture, WAV I/O, resampling
  ui/                egui tabs
  model/             measurement and project types
```

## Known limits

- EQ changes magnitude. It does not shorten the room's decay time and it does not
  reduce the speaker's distortion.
- A measurement at one point is only valid at that point. For a whole sofa, measure
  several positions and use *Fit from average*.
- The Harman headphone target only means something measured through a proper coupler.
  A measurement mic in front of a headphone cannot be corrected to it.
- Music playback for A/B is WAV only. MP3 and FLAC would need a decoder crate added.
- Gated measurements below roughly 200 Hz are not trustworthy, by nature of the short
  window rather than any limitation of the app.

## Performance notes

Two things made the first build sluggish, both fixed:

- **1/N-octave smoothing was quadratic.** On a linear FFT axis a 1/6-octave window
  near 20 kHz covers hundreds of bins, and the weighted implementation measured
  **400 ms per full spectrum** — it was running on every repaint, so the analyser
  managed about two frames a second. It now uses two prefix-sum box passes, which
  give the same triangular 1/N-octave window in **1.3 ms**, about 300x faster, and
  agree with the old weighted curve to within a dB on real spectra.
- **A scroll area pinned to the bottom** kept an animation alive, which repainted
  the whole window forever: 24 % of a core with nothing happening. Idle is now 0 %.

On top of that the analyser rebuilds its display curve only when a new FFT block
arrives rather than on every frame, and tabs other than the analyser refresh at
about 7 fps instead of 30. A live 30 fps analyser costs roughly 19 % of one core.

## Licence

GPL-3.0-or-later. See `LICENSE` in the repository root.
