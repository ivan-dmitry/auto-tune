//! Helpers for plotting on a log frequency axis.
//!
//! Every frequency plot uses x = log10(Hz), so one octave always takes the same
//! width on screen, which matches how hearing works.

use egui_plot::{GridInput, GridMark, Line, Plot, PlotPoints, PlotUi};

pub fn hz_label(f: f64) -> String {
    if f >= 10_000.0 {
        format!("{:.0}k", f / 1000.0)
    } else if f >= 1000.0 {
        let k = f / 1000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{k:.0}k")
        } else {
            format!("{k:.1}k")
        }
    } else if f >= 10.0 {
        format!("{f:.0}")
    } else {
        format!("{f:.1}")
    }
}

/// Grid marks for a log10(Hz) axis: a strong line per decade, fainter ones at 2/3/5.
pub fn log_freq_grid(input: GridInput) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    let mut marks = Vec::new();
    let d0 = lo.floor() as i32 - 1;
    let d1 = hi.ceil() as i32 + 1;
    for d in d0..=d1 {
        let decade = 10f64.powi(d);
        for m in 1..10 {
            let f = decade * m as f64;
            let v = f.log10();
            if v < lo - 0.05 || v > hi + 0.05 {
                continue;
            }
            let step = if m == 1 {
                1.0
            } else if m == 2 || m == 5 {
                0.15
            } else {
                0.05
            };
            marks.push(GridMark {
                value: v,
                step_size: step,
            });
        }
    }
    marks
}

/// A preconfigured frequency plot.
pub struct FreqPlot {
    id: String,
    height: f32,
    y_range: (f64, f64),
    x_range: (f64, f64),
    y_label: String,
    legend: bool,
    link: Option<String>,
}

impl FreqPlot {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            height: 320.0,
            y_range: (-40.0, 20.0),
            x_range: (20.0, 20_000.0),
            y_label: "dB".into(),
            legend: true,
            link: None,
        }
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn y_range(mut self, lo: f64, hi: f64) -> Self {
        self.y_range = (lo, hi);
        self
    }


    pub fn y_label(mut self, s: impl Into<String>) -> Self {
        self.y_label = s.into();
        self
    }

    pub fn legend(mut self, on: bool) -> Self {
        self.legend = on;
        self
    }

    /// Link the x axis with other plots in the same group so zoom and pan stay in sync.
    pub fn link_x(mut self, group: impl Into<String>) -> Self {
        self.link = Some(group.into());
        self
    }

    pub fn show<R>(self, ui: &mut egui::Ui, content: impl FnOnce(&mut PlotUi) -> R) -> R {
        let mut plot = Plot::new(self.id)
            .height(self.height)
            .y_axis_label(self.y_label)
            .allow_drag(true)
            .allow_scroll(false)
            .allow_boxed_zoom(true)
            .set_margin_fraction(egui::vec2(0.0, 0.05))
            .x_grid_spacer(log_freq_grid)
            .x_axis_formatter(|m: GridMark, _r: &std::ops::RangeInclusive<f64>| {
                hz_label(10f64.powf(m.value))
            })
            .label_formatter(|name, p| {
                let f = 10f64.powf(p.x);
                if name.is_empty() {
                    format!("{} Hz\n{:.2} dB", hz_label(f), p.y)
                } else {
                    format!("{name}\n{} Hz\n{:.2} dB", hz_label(f), p.y)
                }
            })
            .include_x(self.x_range.0.log10())
            .include_x(self.x_range.1.log10())
            .include_y(self.y_range.0)
            .include_y(self.y_range.1);
        if self.legend {
            plot = plot.legend(egui_plot::Legend::default().position(egui_plot::Corner::LeftBottom));
        }
        if let Some(g) = self.link {
            plot = plot.link_axis(egui::Id::new(g), true, false);
        }
        plot.show(ui, content).inner
    }
}

/// Turn (frequency, dB) pairs into plot points on the log axis, dropping bad values.
pub fn curve_points(freqs: &[f32], vals: &[f32], f_lo: f32, f_hi: f32) -> PlotPoints {
    let pts: Vec<[f64; 2]> = freqs
        .iter()
        .zip(vals.iter())
        .filter(|(f, v)| **f >= f_lo && **f <= f_hi && v.is_finite() && **v > -200.0)
        .map(|(f, v)| [(*f as f64).log10(), *v as f64])
        .collect();
    PlotPoints::from(pts)
}

/// Thin out an over-long array so the plot stays responsive.
pub fn decimate(freqs: &[f32], vals: &[f32], max_points: usize) -> (Vec<f32>, Vec<f32>) {
    let n = freqs.len();
    if n <= max_points {
        return (freqs.to_vec(), vals.to_vec());
    }
    let step = (n as f32 / max_points as f32).ceil() as usize;
    let mut f = Vec::with_capacity(max_points);
    let mut v = Vec::with_capacity(max_points);
    let mut i = 0;
    while i < n {
        // Keep the maximum of each group so resonance peaks are not lost.
        let hi = (i + step).min(n);
        let mut best = vals[i];
        let mut bf = freqs[i];
        for k in i..hi {
            if vals[k] > best {
                best = vals[k];
                bf = freqs[k];
            }
        }
        f.push(bf);
        v.push(best);
        i = hi;
    }
    (f, v)
}

/// Thin several curves that share one frequency axis, keeping them aligned.
/// Each group keeps its maximum, so resonance peaks survive the thinning.
pub fn decimate_multi(
    freqs: &[f32],
    curves: &[&[f32]],
    max_points: usize,
) -> (Vec<f32>, Vec<Vec<f32>>) {
    let n = freqs.len();
    if n <= max_points || max_points == 0 {
        return (freqs.to_vec(), curves.iter().map(|c| c.to_vec()).collect());
    }
    let step = (n as f32 / max_points as f32).ceil() as usize;
    let mut out_f = Vec::with_capacity(max_points + 1);
    let mut out_c: Vec<Vec<f32>> = curves
        .iter()
        .map(|_| Vec::with_capacity(max_points + 1))
        .collect();
    let mut i = 0usize;
    while i < n {
        let hi = (i + step).min(n);
        out_f.push(freqs[(i + hi - 1) / 2]);
        for (ci, c) in curves.iter().enumerate() {
            let mut best = f32::NEG_INFINITY;
            for k in i..hi.min(c.len()) {
                if c[k] > best {
                    best = c[k];
                }
            }
            out_c[ci].push(if best.is_finite() { best } else { 0.0 });
        }
        i = hi;
    }
    (out_f, out_c)
}

pub fn line(
    pui: &mut PlotUi,
    freqs: &[f32],
    vals: &[f32],
    name: &str,
    color: egui::Color32,
    width: f32,
) {
    let (f, v) = decimate(freqs, vals, 3000);
    pui.line(
        Line::new(curve_points(&f, &v, 5.0, 30_000.0))
            .name(name)
            .color(color)
            .width(width),
    );
}

pub fn dashed_line(
    pui: &mut PlotUi,
    freqs: &[f32],
    vals: &[f32],
    name: &str,
    color: egui::Color32,
) {
    let (f, v) = decimate(freqs, vals, 3000);
    pui.line(
        Line::new(curve_points(&f, &v, 5.0, 30_000.0))
            .name(name)
            .color(color)
            .width(1.6_f32)
            .style(egui_plot::LineStyle::dashed_dense()),
    );
}
