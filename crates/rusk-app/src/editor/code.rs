//! Syntax highlighting for fenced code blocks.
//!
//! Only foreground colors are taken from syntect; Rusk's own surface and type
//! scale stay in control of the visual language.

use eframe::egui::epaint::text::{LayoutJob, TextWrapping};
use eframe::egui::{Color32, FontId, TextFormat};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::theme::Theme;

pub struct CodeHighlighter {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

impl CodeHighlighter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            syntaxes: SyntaxSet::load_defaults_newlines(),
            themes: ThemeSet::load_defaults(),
        }
    }

    #[must_use]
    pub fn job(
        &self,
        code: &str,
        language: Option<&str>,
        theme: &Theme,
        wrap_width: f32,
    ) -> LayoutJob {
        let base_format = TextFormat {
            font_id: FontId::new(15.0, eframe::egui::FontFamily::Name("rusk-mono".into())),
            color: theme.text,
            line_height: Some(23.0),
            ..Default::default()
        };
        let mut job = LayoutJob {
            wrap: TextWrapping::wrap_at_width(wrap_width),
            break_on_newline: true,
            keep_trailing_whitespace: true,
            ..Default::default()
        };

        let Some(language) = language else {
            job.append(code, 0.0, base_format);
            return job;
        };
        let Some(syntax) = self.syntaxes.find_syntax_by_token(language) else {
            job.append(code, 0.0, base_format);
            return job;
        };
        let theme_name = if theme.kind.is_dark() {
            "base16-ocean.dark"
        } else {
            "InspiredGitHub"
        };
        let Some(syn_theme) = self
            .themes
            .themes
            .get(theme_name)
            .or_else(|| self.themes.themes.values().next())
        else {
            job.append(code, 0.0, base_format);
            return job;
        };

        let mut highlighter = HighlightLines::new(syntax, syn_theme);
        for line in LinesWithEndings::from(code) {
            let ranges = match highlighter.highlight_line(line, &self.syntaxes) {
                Ok(ranges) => ranges,
                Err(_) => {
                    job.append(line, 0.0, base_format.clone());
                    continue;
                }
            };
            for (style, text) in ranges {
                if text.is_empty() {
                    continue;
                }
                job.append(
                    text,
                    0.0,
                    TextFormat {
                        font_id: base_format.font_id.clone(),
                        color: syntect_color(style.foreground),
                        line_height: base_format.line_height,
                        ..Default::default()
                    },
                );
            }
        }
        if job.text.is_empty() {
            job.append(" ", 0.0, base_format);
        }
        job
    }
}

fn syntect_color(color: syntect::highlighting::Color) -> Color32 {
    Color32::from_rgb(color.r, color.g, color.b)
}
