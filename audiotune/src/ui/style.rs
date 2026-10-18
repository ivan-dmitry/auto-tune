//! Load a system font with wide glyph coverage and set up the overall look.

/// System fonts tried in order, covering both Windows and Linux.
const FONT_CANDIDATES: &[&str] = &[
    // Windows
    "C:/Windows/Fonts/segoeui.ttf",
    "C:/Windows/Fonts/tahoma.ttf",
    "C:/Windows/Fonts/arial.ttf",
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-fonts/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
];

const MONO_CANDIDATES: &[&str] = &[
    "C:/Windows/Fonts/consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
];

pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui_sans".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "ui_sans".to_owned());
            // Also use it as a monospace fallback so tables never show missing glyphs.
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("ui_sans".to_owned());
            break;
        }
    }
    for path in MONO_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert("ui_mono".to_owned(), egui::FontData::from_owned(bytes));
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "ui_mono".to_owned());
            break;
        }
    }
    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    style.spacing.slider_width = 180.0;
    style.visuals = egui::Visuals::dark();
    style.visuals.window_rounding = 6.0.into();
    style.visuals.widgets.noninteractive.bg_stroke.color =
        egui::Color32::from_gray(60);
    style.visuals.panel_fill = egui::Color32::from_rgb(24, 26, 30);
    style.visuals.extreme_bg_color = egui::Color32::from_rgb(16, 18, 21);
    ctx.set_style(style);
}

pub const OK: egui::Color32 = egui::Color32::from_rgb(0x66, 0xBB, 0x6A);
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xB3, 0x00);
pub const BAD: egui::Color32 = egui::Color32::from_rgb(0xEF, 0x53, 0x50);
pub const MUTED: egui::Color32 = egui::Color32::from_gray(150);
pub const TARGET: egui::Color32 = egui::Color32::from_rgb(0xFF, 0xD5, 0x4F);
pub const EQ_CURVE: egui::Color32 = egui::Color32::from_rgb(0x9C, 0xCC, 0x65);
pub const PREDICT: egui::Color32 = egui::Color32::from_rgb(0x4F, 0xC3, 0xF7);
