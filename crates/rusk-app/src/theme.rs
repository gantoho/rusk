//! Visual language for Rusk: warm paper, quiet chrome, and a typographic
//! hierarchy tuned for long-form reading.

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Visuals,
};

pub const SANS_BOLD: &str = "rusk-sans-bold";
pub const SANS_ITALIC: &str = "rusk-sans-italic";
pub const SANS_BOLD_ITALIC: &str = "rusk-sans-bold-italic";
pub const MONO: &str = "rusk-mono";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Paper,
    Ink,
}

impl ThemeKind {
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Paper => Self::Ink,
            Self::Ink => Self::Paper,
        }
    }

    #[must_use]
    pub fn is_dark(self) -> bool {
        matches!(self, Self::Ink)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub kind: ThemeKind,
    pub bg: Color32,
    pub panel: Color32,
    pub page: Color32,
    pub page_edge: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub border: Color32,
    pub code_bg: Color32,
    pub code_border: Color32,
    pub inline_code: Color32,
    pub quote_bar: Color32,
    pub selection: Color32,
    pub caret: Color32,
    pub link: Color32,
    pub syntax: Color32,
    pub table_head: Color32,
    pub table_stripe: Color32,
    pub image_placeholder: Color32,
    pub error: Color32,
}

impl Theme {
    #[must_use]
    pub fn new(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Paper => Self {
                kind,
                bg: Color32::from_rgb(0xF2, 0xF0, 0xEA),
                panel: Color32::from_rgb(0xEE, 0xEB, 0xE4),
                page: Color32::from_rgb(0xFD, 0xFC, 0xF9),
                page_edge: Color32::from_rgb(0xE7, 0xE3, 0xD9),
                text: Color32::from_rgb(0x2D, 0x2B, 0x26),
                muted: Color32::from_rgb(0x8A, 0x85, 0x7C),
                faint: Color32::from_rgb(0xB9, 0xB3, 0xA8),
                accent: Color32::from_rgb(0xB1, 0x58, 0x32),
                accent_soft: Color32::from_rgb(0xF6, 0xE7, 0xDC),
                border: Color32::from_rgb(0xE6, 0xE1, 0xD6),
                code_bg: Color32::from_rgb(0xF5, 0xF2, 0xEB),
                code_border: Color32::from_rgb(0xE8, 0xE2, 0xD6),
                inline_code: Color32::from_rgb(0x9B, 0x4B, 0x2C),
                quote_bar: Color32::from_rgb(0xDD, 0xD4, 0xC5),
                selection: Color32::from_rgba_premultiplied(0xEB, 0xCD, 0xB8, 0x98),
                caret: Color32::from_rgb(0xB8, 0x5D, 0x33),
                link: Color32::from_rgb(0x2E, 0x6B, 0x6E),
                syntax: Color32::from_rgb(0xB0, 0xA7, 0x99),
                table_head: Color32::from_rgb(0xF3, 0xEF, 0xE7),
                table_stripe: Color32::from_rgb(0xFB, 0xF9, 0xF4),
                image_placeholder: Color32::from_rgb(0xF7, 0xF3, 0xEB),
                error: Color32::from_rgb(0xB4, 0x3B, 0x2F),
            },
            ThemeKind::Ink => Self {
                kind,
                bg: Color32::from_rgb(0x19, 0x18, 0x16),
                panel: Color32::from_rgb(0x15, 0x14, 0x12),
                page: Color32::from_rgb(0x21, 0x1F, 0x1C),
                page_edge: Color32::from_rgb(0x2C, 0x29, 0x25),
                text: Color32::from_rgb(0xE8, 0xE2, 0xD7),
                muted: Color32::from_rgb(0x9D, 0x97, 0x8C),
                faint: Color32::from_rgb(0x60, 0x5A, 0x51),
                accent: Color32::from_rgb(0xE1, 0x8A, 0x5B),
                accent_soft: Color32::from_rgb(0x39, 0x2A, 0x21),
                border: Color32::from_rgb(0x33, 0x30, 0x2B),
                code_bg: Color32::from_rgb(0x1A, 0x18, 0x16),
                code_border: Color32::from_rgb(0x32, 0x2E, 0x28),
                inline_code: Color32::from_rgb(0xE4, 0x9A, 0x71),
                quote_bar: Color32::from_rgb(0x4B, 0x44, 0x3B),
                selection: Color32::from_rgba_premultiplied(0x8B, 0x5B, 0x3D, 0x90),
                caret: Color32::from_rgb(0xE9, 0x9E, 0x6E),
                link: Color32::from_rgb(0x83, 0xBC, 0xB7),
                syntax: Color32::from_rgb(0x75, 0x6D, 0x62),
                table_head: Color32::from_rgb(0x28, 0x25, 0x21),
                table_stripe: Color32::from_rgb(0x1E, 0x1C, 0x19),
                image_placeholder: Color32::from_rgb(0x1D, 0x1B, 0x18),
                error: Color32::from_rgb(0xE0, 0x78, 0x69),
            },
        }
    }

    #[must_use]
    pub fn body_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Proportional)
    }

    #[must_use]
    pub fn heading_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(Arc::from(SANS_BOLD)))
    }

    #[must_use]
    pub fn mono_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(Arc::from(MONO)))
    }

    #[must_use]
    pub fn bold_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(Arc::from(SANS_BOLD)))
    }

    #[must_use]
    pub fn italic_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(Arc::from(SANS_ITALIC)))
    }

    #[must_use]
    pub fn bold_italic_font(&self, size: f32) -> FontId {
        FontId::new(size, FontFamily::Name(Arc::from(SANS_BOLD_ITALIC)))
    }

    pub fn install(&self, ctx: &egui::Context) {
        ctx.set_visuals(self.visuals());
        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 7.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.spacing.menu_margin = egui::Margin::same(8);
        style.visuals.widgets.inactive.corner_radius = 8.into();
        style.visuals.widgets.hovered.corner_radius = 8.into();
        style.visuals.widgets.active.corner_radius = 8.into();
        style.visuals.widgets.noninteractive.corner_radius = 8.into();
        style.visuals.widgets.inactive.weak_bg_fill = self.panel;
        style.visuals.widgets.inactive.bg_fill = self.panel;
        style.visuals.widgets.hovered.weak_bg_fill = self.accent_soft;
        style.visuals.widgets.hovered.bg_fill = self.accent_soft;
        style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, self.border);
        style.visuals.widgets.active.weak_bg_fill = self.accent_soft;
        style.visuals.widgets.active.bg_fill = self.accent_soft;
        style.visuals.widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, self.muted);
        style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        style.visuals.widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        style.visuals.override_text_color = None;
        style.visuals.hyperlink_color = self.link;
        style.visuals.window_fill = self.page;
        style.visuals.window_stroke = Stroke::new(1.0, self.border);
        style.visuals.window_corner_radius = 14.into();
        style.visuals.menu_corner_radius = 12.into();
        style.visuals.panel_fill = self.bg;
        style.visuals.faint_bg_color = self.table_stripe;
        style.visuals.extreme_bg_color = self.code_bg;
        style.visuals.code_bg_color = self.code_bg;
        style.visuals.selection.bg_fill = self.selection;
        style.visuals.selection.stroke = Stroke::new(1.0, self.accent);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.5, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(24.0, FontFamily::Name(Arc::from(SANS_BOLD))),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(14.0, FontFamily::Name(Arc::from(MONO))),
        );
        ctx.set_global_style(style);
    }

    #[must_use]
    pub fn visuals(&self) -> Visuals {
        let mut visuals = if self.kind.is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };
        visuals.panel_fill = self.bg;
        visuals.window_fill = self.page;
        visuals.window_stroke = Stroke::new(1.0, self.border);
        visuals.override_text_color = Some(self.text);
        visuals.selection.bg_fill = self.selection;
        visuals.selection.stroke = Stroke::new(1.0, self.accent);
        visuals.hyperlink_color = self.link;
        visuals.extreme_bg_color = self.code_bg;
        visuals.code_bg_color = self.code_bg;
        visuals.faint_bg_color = self.table_stripe;
        visuals.widgets.noninteractive.bg_fill = self.panel;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        visuals.widgets.inactive.bg_fill = self.panel;
        visuals.widgets.inactive.weak_bg_fill = self.panel;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, self.muted);
        visuals.widgets.hovered.bg_fill = self.accent_soft;
        visuals.widgets.hovered.weak_bg_fill = self.accent_soft;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, self.border);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, self.text);
        visuals.widgets.active.bg_fill = self.accent_soft;
        visuals.widgets.active.weak_bg_fill = self.accent_soft;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, self.accent);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, self.text);
        visuals
    }
}

/// Install system fonts with separate families for each weight/italic
/// combination. egui's `TextFormat` has no weight field, so named families are
/// the clean way to get real bold and italic faces instead of synthetic ones.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let default_proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let default_mono = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let sans = load_first(&[
        "C:/Windows/Fonts/segoeui.ttf",
        "C:/Windows/Fonts/calibri.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ]);
    let sans_bold = load_first(&[
        "C:/Windows/Fonts/segoeuib.ttf",
        "C:/Windows/Fonts/calibrib.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    ]);
    let sans_italic = load_first(&[
        "C:/Windows/Fonts/segoeuii.ttf",
        "C:/Windows/Fonts/calibrii.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
    ]);
    let sans_bold_italic = load_first(&[
        "C:/Windows/Fonts/segoeuiz.ttf",
        "C:/Windows/Fonts/calibriz.ttf",
        "/System/Library/Fonts/SFNS.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
    ]);
    let cjk = load_with_index(&[
        ("C:/Windows/Fonts/msyh.ttc", 0),
        ("C:/Windows/Fonts/Deng.ttf", 0),
    ]);
    let cjk_bold = load_with_index(&[
        ("C:/Windows/Fonts/msyhbd.ttc", 0),
        ("C:/Windows/Fonts/Dengb.ttf", 0),
        ("C:/Windows/Fonts/simhei.ttf", 0),
    ]);
    let mono = load_first(&[
        "C:/Windows/Fonts/CascadiaMono.ttf",
        "C:/Windows/Fonts/consola.ttf",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    ]);
    let mono_bold = load_first(&[
        "C:/Windows/Fonts/consolab.ttf",
        "/System/Library/Fonts/SFNSMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    ]);
    let emoji = load_first(&[
        "C:/Windows/Fonts/seguiemj.ttf",
        "/System/Library/Fonts/Apple Color Emoji.ttc",
        "/usr/share/fonts/truetype/noto/NotoColorEmoji.ttf",
    ]);

    let mut proportional = Vec::new();
    let add_face = |fonts: &mut FontDefinitions,
                    key: &str,
                    data: Option<(Vec<u8>, u32)>,
                    list: &mut Vec<String>| {
        if let Some((bytes, index)) = data {
            fonts.font_data.insert(
                key.to_owned(),
                Arc::new(FontData::from_owned(bytes).with_index(index)),
            );
            list.push(key.to_owned());
        }
    };

    add_face(&mut fonts, "rusk-sans", sans, &mut proportional);
    add_face(&mut fonts, "rusk-cjk", cjk, &mut proportional);
    add_face(&mut fonts, "rusk-emoji", emoji, &mut proportional);
    proportional.extend(default_proportional.iter().cloned());
    fonts
        .families
        .insert(FontFamily::Proportional, proportional);

    add_face(&mut fonts, "rusk-cjk-bold", cjk_bold, &mut Vec::new());

    let mut build = |name: &str, key: &str, data: Option<(Vec<u8>, u32)>, cjk_fallback: &str| {
        let mut list = Vec::new();
        if let Some((bytes, index)) = data {
            fonts.font_data.insert(
                key.to_owned(),
                Arc::new(FontData::from_owned(bytes).with_index(index)),
            );
            list.push(key.to_owned());
        }
        if fonts.font_data.contains_key(cjk_fallback) {
            list.push(cjk_fallback.to_owned());
        }
        if fonts.font_data.contains_key("rusk-emoji") {
            list.push("rusk-emoji".to_owned());
        }
        list.extend(default_proportional.iter().cloned());
        fonts
            .families
            .insert(FontFamily::Name(Arc::from(name)), list);
    };

    build(SANS_BOLD, "rusk-sans-bold", sans_bold, "rusk-cjk-bold");
    build(SANS_ITALIC, "rusk-sans-italic", sans_italic, "rusk-cjk");
    build(
        SANS_BOLD_ITALIC,
        "rusk-sans-bold-italic",
        sans_bold_italic,
        "rusk-cjk-bold",
    );

    let mut mono_list = Vec::new();
    add_face(&mut fonts, "rusk-mono", mono, &mut mono_list);
    if fonts.font_data.contains_key("rusk-cjk") {
        mono_list.push("rusk-cjk".to_owned());
    }
    mono_list.extend(default_mono);
    fonts
        .families
        .insert(FontFamily::Name(Arc::from(MONO)), mono_list.clone());

    add_face(&mut fonts, "rusk-mono-bold", mono_bold, &mut Vec::new());
    if fonts.font_data.contains_key("rusk-mono-bold") {
        let mut bold = vec!["rusk-mono-bold".to_owned()];
        bold.extend(mono_list);
        fonts
            .families
            .insert(FontFamily::Name(Arc::from("rusk-mono-bold")), bold);
    }

    ctx.set_fonts(fonts);
}

trait FontDataIndex {
    fn with_index(self, index: u32) -> Self;
}

impl FontDataIndex for FontData {
    fn with_index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }
}

fn load_first(paths: &[&str]) -> Option<(Vec<u8>, u32)> {
    paths
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| (bytes, 0)))
}

fn load_with_index(paths: &[(&str, u32)]) -> Option<(Vec<u8>, u32)> {
    paths.iter().find_map(|(path, index)| {
        std::fs::read(Path::new(path))
            .ok()
            .map(|bytes| (bytes, *index))
    })
}
