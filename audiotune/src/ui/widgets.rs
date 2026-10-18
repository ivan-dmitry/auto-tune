//! Small widgets reused across tabs.

use super::style;

/// Mixer-style level bar from -60 dBFS to 0 dBFS, with a peak tick.
pub fn level_meter(ui: &mut egui::Ui, rms_db: f32, peak_db: f32, width: f32) {
    let height = 14.0;
    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, egui::Color32::from_gray(30));

    let norm = |db: f32| ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let w = rect.width() * norm(rms_db);
    let bar = egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height()));
    let color = if rms_db > -3.0 {
        style::BAD
    } else if rms_db > -12.0 {
        style::WARN
    } else {
        style::OK
    };
    painter.rect_filled(bar, 3.0, color);

    // Peak tick.
    let px = rect.min.x + rect.width() * norm(peak_db);
    painter.line_segment(
        [egui::pos2(px, rect.min.y), egui::pos2(px, rect.max.y)],
        egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
    );
    // Reference marks at -18 and -6 dBFS.
    for m in [-18.0f32, -6.0] {
        let x = rect.min.x + rect.width() * norm(m);
        painter.line_segment(
            [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
            egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
        );
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(70)),
    );
}

/// Small grey caption explaining a control.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(style::MUTED));
}

/// Group heading.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(title).strong());
    ui.separator();
}

/// Frequency entry with octave up / down buttons.
pub fn freq_drag(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.small_button("/2").clicked() {
            *value = (*value / 2.0).clamp(*range.start(), *range.end());
            changed = true;
        }
        let speed = (*value * 0.01).max(0.1);
        let r = ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range.clone())
                .suffix(" Hz"),
        );
        changed |= r.changed();
        if ui.small_button("x2").clicked() {
            *value = (*value * 2.0).clamp(*range.start(), *range.end());
            changed = true;
        }
    });
    changed
}


/// A slider bar drawn to show a dB value between two limits, used in settings tables.
pub fn gain_bar(ui: &mut egui::Ui, gain_db: f32, limit_db: f32, width: f32) {
    let height = 12.0;
    let (rect, _r) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, egui::Color32::from_gray(28));
    let mid = rect.center().x;
    p.line_segment(
        [egui::pos2(mid, rect.min.y), egui::pos2(mid, rect.max.y)],
        egui::Stroke::new(1.0_f32, egui::Color32::from_gray(80)),
    );
    let frac = (gain_db / limit_db.max(0.1)).clamp(-1.0, 1.0);
    let w = (rect.width() * 0.5) * frac;
    let bar = if w >= 0.0 {
        egui::Rect::from_min_size(egui::pos2(mid, rect.min.y), egui::vec2(w, height))
    } else {
        egui::Rect::from_min_size(egui::pos2(mid + w, rect.min.y), egui::vec2(-w, height))
    };
    let color = if gain_db > 0.0 { style::WARN } else { style::PREDICT };
    p.rect_filled(bar, 2.0, color);
}
