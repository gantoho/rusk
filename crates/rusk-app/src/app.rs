//! The native shell: window chrome, file workflow, outline, and editor host.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers,
    RichText, Sense, Stroke, Vec2,
};
use rusk_core::{Document, EditKind, Selection, ViewMode};

use crate::editor::Editor;
use crate::theme::{Theme, ThemeKind, install_fonts};

#[derive(Clone, Debug)]
enum Pending {
    New,
    Open,
    OpenPath(PathBuf),
    Close,
}

#[derive(Clone, Debug)]
enum Modal {
    None,
    Unsaved(Pending),
    About,
    Shortcuts,
    Error(String),
}

struct Toast {
    text: String,
    until: Instant,
    error: bool,
}

#[derive(Default)]
struct FindState {
    open: bool,
    replace_visible: bool,
    query: String,
    replacement: String,
    focus_query: bool,
}

impl FindState {
    fn height(&self) -> f32 {
        if self.replace_visible { 78.0 } else { 44.0 }
    }
}

pub struct RuskApp {
    doc: Document,
    editor: Editor,
    theme: Theme,
    kind: ThemeKind,
    outline_open: bool,
    modal: Modal,
    toast: Option<Toast>,
    force_close: bool,
    close_now: bool,
    relayout_frames: u8,
    last_title: String,
    find: FindState,
    #[cfg(target_os = "windows")]
    native_menu: Option<crate::native_menu::NativeMenu>,
}

impl RuskApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_fonts(&cc.egui_ctx);
        let kind = ThemeKind::Paper;
        let theme = Theme::new(kind);
        theme.install(&cc.egui_ctx);

        let mut startup_error = None;
        let path = std::env::args().nth(1).map(PathBuf::from);
        let doc = match path {
            Some(path) => match Document::open(&path) {
                Ok(doc) => doc,
                Err(err) => {
                    startup_error = Some(format!("Could not open {}: {err}", path.display()));
                    Document::new()
                }
            },
            None => Document::new(),
        };
        let mut editor = Editor::new(&cc.egui_ctx);
        editor.reset(true);
        let last_title = window_title(&doc);

        #[cfg(target_os = "windows")]
        let native_menu = {
            let menu = crate::native_menu::NativeMenu::install(cc);
            if let Some(menu) = &menu {
                menu.set_dark(kind.is_dark());
                menu.sync(false, kind.is_dark(), false);
            }
            menu
        };

        Self {
            doc,
            editor,
            theme,
            kind,
            outline_open: false,
            modal: Modal::None,
            toast: startup_error.as_ref().map(|error| Toast {
                text: error.clone(),
                until: Instant::now() + Duration::from_secs(6),
                error: true,
            }),
            force_close: false,
            close_now: false,
            relayout_frames: 0,
            last_title,
            find: FindState::default(),
            #[cfg(target_os = "windows")]
            native_menu,
        }
    }

    /// True when Windows draws the File menu, so egui must not draw its own.
    #[cfg(target_os = "windows")]
    fn has_native_menu(&self) -> bool {
        self.native_menu.is_some()
    }

    #[cfg(not(target_os = "windows"))]
    fn has_native_menu(&self) -> bool {
        false
    }

    #[cfg(target_os = "windows")]
    fn poll_native_menu(&mut self, ctx: &egui::Context) {
        use crate::native_menu::MenuCmd;
        let events = self
            .native_menu
            .as_ref()
            .map(crate::native_menu::NativeMenu::take_events)
            .unwrap_or_default();
        for cmd in events {
            if !matches!(self.modal, Modal::None) {
                continue;
            }
            match cmd {
                MenuCmd::New => self.confirm(Pending::New),
                MenuCmd::Open => self.confirm(Pending::Open),
                MenuCmd::Save => self.save(),
                MenuCmd::SaveAs => self.save_as(),
                MenuCmd::ExportHtml => self.export_html(),
                MenuCmd::CopyHtml => self.copy_html(ctx),
                MenuCmd::Shortcuts => self.modal = Modal::Shortcuts,
                MenuCmd::About => self.modal = Modal::About,
                MenuCmd::Quit => self.confirm(Pending::Close),
                MenuCmd::Undo => {
                    if self.doc.undo() {
                        self.editor.invalidate_layout();
                    }
                    self.editor.request_focus();
                }
                MenuCmd::Redo => {
                    if self.doc.redo() {
                        self.editor.invalidate_layout();
                    }
                    self.editor.request_focus();
                }
                MenuCmd::Find => self.open_find(false),
                MenuCmd::FindNext => self.find_step(false),
                MenuCmd::Replace => self.open_find(true),
                MenuCmd::WriteMode | MenuCmd::SourceMode => {
                    let mode = if cmd == MenuCmd::WriteMode {
                        ViewMode::Wysiwyg
                    } else {
                        ViewMode::Source
                    };
                    if self.doc.mode() != mode {
                        self.doc.set_mode(mode);
                        self.editor.invalidate_layout();
                        self.editor.request_focus();
                    }
                }
                MenuCmd::ToggleOutline => {
                    self.outline_open = !self.outline_open;
                    self.editor.request_focus();
                }
                MenuCmd::ToggleTheme => self.toggle_theme(ctx),
            }
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, kind: ThemeKind) {
        self.kind = kind;
        self.theme = Theme::new(kind);
        self.theme.install(ctx);
        #[cfg(target_os = "windows")]
        if let Some(menu) = &self.native_menu {
            menu.set_dark(kind.is_dark());
        }
        self.editor.invalidate_layout();
        // A palette change alters every text color; make sure the layout is
        // rebuilt with the new theme even if the widget pass is mid-frame.
        self.relayout_frames = 3;
    }

    fn toggle_theme(&mut self, ctx: &egui::Context) {
        let kind = self.kind.toggled();
        self.set_theme(ctx, kind);
        self.set_toast(
            if kind.is_dark() {
                "Ink theme"
            } else {
                "Paper theme"
            },
            false,
        );
    }

    fn set_toast(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            until: Instant::now() + Duration::from_secs(if error { 6 } else { 3 }),
            error,
        });
    }

    fn open_find(&mut self, replace: bool) {
        if !self.find.open {
            let selected = self.doc.selection_text();
            if !selected.is_empty() && !selected.contains('\n') && selected.chars().count() <= 120 {
                self.find.query = selected;
            }
        }
        self.find.open = true;
        self.find.replace_visible = replace;
        self.find.focus_query = true;
        self.find_select_first();
    }

    fn close_find(&mut self) {
        self.find.open = false;
        self.find.focus_query = false;
        self.editor.request_focus();
    }

    fn find_matches(&self) -> Vec<std::ops::Range<usize>> {
        self.doc.find_all(&self.find.query)
    }

    fn find_select_first(&mut self) {
        let matches = self.find_matches();
        if matches.is_empty() {
            return;
        }
        let selection = self.doc.selection();
        if matches.iter().any(|range| range == &selection.range()) {
            return;
        }
        let target = matches
            .iter()
            .find(|range| range.start >= selection.end())
            .or_else(|| matches.first())
            .cloned();
        if let Some(range) = target {
            self.doc
                .set_selection(Selection::new(range.start, range.end));
            self.editor.reset_vertical_goal();
            self.editor.scroll_to_caret = true;
        }
    }

    fn find_step(&mut self, backward: bool) {
        let matches = self.find_matches();
        if matches.is_empty() {
            return;
        }
        let selection = self.doc.selection().range();
        let current = matches.iter().position(|range| range == &selection);
        let index = match current {
            Some(index) => {
                if backward {
                    (index + matches.len() - 1) % matches.len()
                } else {
                    (index + 1) % matches.len()
                }
            }
            None => {
                if backward {
                    matches
                        .iter()
                        .rposition(|range| range.end <= selection.start)
                        .unwrap_or(matches.len() - 1)
                } else {
                    matches
                        .iter()
                        .position(|range| range.start >= selection.end)
                        .unwrap_or(0)
                }
            }
        };
        let range = matches[index].clone();
        self.doc
            .set_selection(Selection::new(range.start, range.end));
        self.editor.reset_vertical_goal();
        self.editor.scroll_to_caret = true;
    }

    fn replace_current(&mut self) {
        if self.find.query.is_empty() {
            return;
        }
        let selection = self.doc.selection().range();
        let is_match = self.find_matches().iter().any(|range| range == &selection);
        if !is_match {
            self.find_step(false);
            return;
        }
        let replacement = self.find.replacement.clone();
        if self
            .doc
            .replace_range(selection, &replacement, EditKind::Replace)
        {
            self.find_step(false);
        }
    }

    fn replace_all_matches(&mut self) {
        if self.find.query.is_empty() {
            return;
        }
        let count = self
            .doc
            .replace_all(&self.find.query, &self.find.replacement);
        if count == 0 {
            self.set_toast("No matches", false);
        } else {
            self.set_toast(format!("Replaced {count} matches"), false);
            self.editor.scroll_to_caret = true;
        }
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        let query_id = ui.make_persistent_id("rusk-find-query");
        let query_focused = ui.memory(|memory| memory.has_focus(query_id));
        let mut close = false;
        let mut next = false;
        let mut previous = false;
        let mut replace_one = false;
        let mut replace_all = false;
        let mut toggle_replace = false;

        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            close = true;
        } else if query_focused {
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                next = true;
            } else if ui.input_mut(|i| i.consume_key(Modifiers::SHIFT, Key::Enter)) {
                previous = true;
            }
        }

        let mut query = self.find.query.clone();
        let mut replacement = self.find.replacement.clone();
        let matches = self.doc.find_all(&query);
        let selection = self.doc.selection().range();
        let current = matches
            .iter()
            .position(|range| range == &selection)
            .map_or(0, |index| index + 1);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new("Find").size(12.0).color(theme.muted));
            let response = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .id(query_id)
                    .hint_text("Search in document")
                    .desired_width(260.0),
            );
            if self.find.focus_query {
                response.request_focus();
                self.find.focus_query = false;
            }
            if response.changed() {
                self.find.query = query.clone();
                self.find_select_first();
            }
            let count = if matches.is_empty() {
                "0/0".to_owned()
            } else {
                format!("{current}/{}", matches.len())
            };
            ui.label(
                RichText::new(count)
                    .font(theme.mono_font(10.5))
                    .color(theme.faint),
            );
            if ui
                .add(
                    egui::Button::new(RichText::new("↑").size(14.0).color(theme.muted))
                        .frame(false),
                )
                .on_hover_text("Previous match (Shift+Enter)")
                .clicked()
            {
                previous = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("↓").size(14.0).color(theme.muted))
                        .frame(false),
                )
                .on_hover_text("Next match (Enter)")
                .clicked()
            {
                next = true;
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("Replace").size(11.5).color(
                        if self.find.replace_visible {
                            theme.accent
                        } else {
                            theme.muted
                        },
                    ))
                    .frame(false),
                )
                .clicked()
            {
                toggle_replace = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("×").size(16.0).color(theme.muted))
                            .frame(false),
                    )
                    .on_hover_text("Close (Esc)")
                    .clicked()
                {
                    close = true;
                }
            });
        });

        if self.find.replace_visible {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(30.0);
                ui.add(
                    egui::TextEdit::singleline(&mut replacement)
                        .hint_text("Replace with")
                        .desired_width(260.0),
                );
                if ui.button("Replace").clicked() {
                    replace_one = true;
                }
                if ui.button("All").clicked() {
                    replace_all = true;
                }
            });
        }

        if query != self.find.query {
            self.find.query = query;
        }
        if replacement != self.find.replacement {
            self.find.replacement = replacement;
        }
        if toggle_replace {
            self.find.replace_visible = !self.find.replace_visible;
            self.find.focus_query = true;
        }
        if close {
            self.close_find();
            return;
        }
        if previous {
            self.find_step(true);
        } else if next {
            self.find_step(false);
        }
        if replace_one {
            self.replace_current();
        }
        if replace_all {
            self.replace_all_matches();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        if !matches!(self.modal, Modal::None) {
            return;
        }
        if self.find.open && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close_find();
            return;
        }
        if consume(ctx, Modifiers::COMMAND, Key::F) {
            self.open_find(false);
            return;
        }
        if consume(ctx, Modifiers::COMMAND, Key::H) {
            self.open_find(true);
            return;
        }
        if consume(ctx, Modifiers::NONE, Key::F3) {
            self.find_step(false);
            return;
        }
        if consume(ctx, Modifiers::SHIFT, Key::F3) {
            self.find_step(true);
            return;
        }
        if consume(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::S) {
            self.save_as();
        } else if consume(ctx, Modifiers::COMMAND, Key::S) {
            self.save();
        } else if consume(ctx, Modifiers::COMMAND, Key::O) {
            self.confirm(Pending::Open);
        } else if consume(ctx, Modifiers::COMMAND, Key::N) {
            self.confirm(Pending::New);
        } else if consume(ctx, Modifiers::COMMAND, Key::Slash) {
            self.doc.toggle_mode();
            self.editor.invalidate_layout();
            self.editor.request_focus();
            self.set_toast(format!("{} mode", self.doc.mode().label()), false);
        } else if consume(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::E) {
            self.export_html();
        } else if consume(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::O) {
            self.outline_open = !self.outline_open;
        } else if consume(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::D) {
            self.toggle_theme(ctx);
        } else if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            self.modal = Modal::Shortcuts;
        }
    }

    fn handle_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && self.doc.dirty() && !self.force_close {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            if matches!(self.modal, Modal::None) {
                self.modal = Modal::Unsaved(Pending::Close);
            }
        }
    }

    fn confirm(&mut self, pending: Pending) {
        if self.doc.dirty() {
            self.modal = Modal::Unsaved(pending);
        } else {
            self.execute(pending);
        }
    }

    fn execute(&mut self, pending: Pending) {
        match pending {
            Pending::New => {
                self.doc = Document::empty();
                self.close_find();
                self.editor.reset(true);
                self.editor.request_focus();
                self.set_toast("New document", false);
            }
            Pending::Open => self.open_dialog(),
            Pending::OpenPath(path) => self.open_path(&path),
            Pending::Close => {
                self.force_close = true;
                self.close_now = true;
            }
        }
    }

    fn open_dialog(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "mdown", "mkd", "txt"])
            .add_filter("All files", &["*"])
            .set_title("Open Markdown")
            .pick_file()
        else {
            return;
        };
        self.open_path(&path);
    }

    fn open_path(&mut self, path: &Path) {
        match Document::open(path) {
            Ok(doc) => {
                self.doc = doc;
                self.close_find();
                self.editor.reset(true);
                self.editor.request_focus();
                self.set_toast(format!("Opened {}", path.display()), false);
            }
            Err(err) => self.set_toast(format!("Could not open file: {err}"), true),
        }
    }

    fn save(&mut self) {
        if self.doc.path().is_some() {
            match self.doc.save() {
                Ok(_) => {
                    self.set_toast("Saved", false);
                    self.editor.request_focus();
                }
                Err(err) => self.set_toast(format!("Save failed: {err}"), true),
            }
        } else {
            self.save_as();
        }
    }

    fn save_as(&mut self) {
        let suggested = self
            .doc
            .file_name()
            .unwrap_or_else(|| "untitled.md".to_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown"])
            .set_file_name(suggested)
            .set_title("Save Markdown")
            .save_file()
        else {
            return;
        };
        match self.doc.save_to(&path) {
            Ok(()) => {
                self.set_toast(format!("Saved {}", path.display()), false);
                self.editor.request_focus();
            }
            Err(err) => self.set_toast(format!("Save failed: {err}"), true),
        }
    }

    fn export_html(&mut self) {
        let html = html_document(&self.doc, &self.theme);
        let suggested = self
            .doc
            .path()
            .and_then(|p| p.file_stem())
            .map(|s| format!("{}.html", s.to_string_lossy()))
            .unwrap_or_else(|| "untitled.html".to_owned());
        let Some(path) = rfd::FileDialog::new()
            .add_filter("HTML", &["html", "htm"])
            .set_file_name(suggested)
            .set_title("Export HTML")
            .save_file()
        else {
            return;
        };
        match std::fs::write(&path, html) {
            Ok(()) => {
                self.set_toast(format!("Exported {}", path.display()), false);
                self.editor.request_focus();
            }
            Err(err) => self.set_toast(format!("Export failed: {err}"), true),
        }
    }

    fn copy_html(&mut self, ctx: &egui::Context) {
        ctx.copy_text(html_document(&self.doc, &self.theme));
        self.editor.request_focus();
        self.set_toast("HTML copied to clipboard", false);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let theme = self.theme;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (logo_rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::hover());
            ui.painter().rect_filled(logo_rect, 8.0, theme.accent);
            ui.painter().text(
                logo_rect.center(),
                Align2::CENTER_CENTER,
                "R",
                theme.bold_font(16.0),
                Color32::WHITE,
            );
            ui.label(
                RichText::new("Rusk")
                    .font(theme.bold_font(16.0))
                    .color(theme.text),
            );
            ui.add_space(2.0);
            if !self.has_native_menu() {
                ui.menu_button(RichText::new("File").size(13.0).color(theme.muted), |ui| {
                    ui.set_min_width(210.0);
                    if menu_item(ui, "New", "Ctrl+N").clicked() {
                        self.confirm(Pending::New);
                        ui.close();
                    }
                    if menu_item(ui, "Open…", "Ctrl+O").clicked() {
                        self.confirm(Pending::Open);
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Save", "Ctrl+S").clicked() {
                        self.save();
                        ui.close();
                    }
                    if menu_item(ui, "Save As…", "Ctrl+Shift+S").clicked() {
                        self.save_as();
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Export HTML…", "Ctrl+Shift+E").clicked() {
                        self.export_html();
                        ui.close();
                    }
                    if menu_item(ui, "Copy as HTML", "").clicked() {
                        self.copy_html(ctx);
                        ui.close();
                    }
                    ui.separator();
                    if menu_item(ui, "Keyboard shortcuts", "F1").clicked() {
                        self.modal = Modal::Shortcuts;
                        ui.close();
                    }
                    if menu_item(ui, "About Rusk", "").clicked() {
                        self.modal = Modal::About;
                        ui.close();
                    }
                });
            }

            ui.add_space(4.0);
            ui.label(
                RichText::new(self.doc.title())
                    .size(12.5)
                    .color(theme.muted),
            );
            if self.doc.dirty() {
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.0), Sense::hover());
                ui.painter().circle_filled(dot.center(), 2.5, theme.accent);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.theme_toggle(ui);
                ui.add_space(4.0);
                self.mode_switch(ui);
                ui.add_space(4.0);
                let outline = ui.add(
                    egui::Button::new(RichText::new("Outline").size(12.5).color(
                        if self.outline_open {
                            theme.accent
                        } else {
                            theme.muted
                        },
                    ))
                    .frame(false)
                    .fill(if self.outline_open {
                        theme.accent_soft
                    } else {
                        Color32::TRANSPARENT
                    })
                    .min_size(Vec2::new(62.0, 28.0)),
                );
                if outline.clicked() {
                    self.outline_open = !self.outline_open;
                    self.editor.request_focus();
                }
                if !self.has_native_menu() {
                    ui.add_space(4.0);
                    ui.menu_button(
                        RichText::new("Export").size(12.5).color(theme.muted),
                        |ui| {
                            if menu_item(ui, "HTML file…", "Ctrl+Shift+E").clicked() {
                                self.export_html();
                                ui.close();
                            }
                            if menu_item(ui, "Copy HTML", "").clicked() {
                                self.copy_html(ctx);
                                ui.close();
                            }
                        },
                    );
                }
            });
            let rect = ui.max_rect();
            ui.painter().line_segment(
                [
                    egui::Pos2::new(rect.left(), rect.bottom()),
                    egui::Pos2::new(rect.right(), rect.bottom()),
                ],
                Stroke::new(1.0, theme.border),
            );
        });
    }

    fn theme_toggle(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        let (rect, response) = ui.allocate_exact_size(Vec2::new(30.0, 30.0), Sense::click());
        let response = response.on_hover_text("Toggle Paper / Ink");
        let center = rect.center();
        ui.painter().circle_filled(center, 10.0, theme.page);
        ui.painter()
            .circle_stroke(center, 10.0, Stroke::new(1.0, theme.border));
        if self.kind.is_dark() {
            ui.painter()
                .circle_filled(center + Vec2::new(2.0, -1.0), 6.5, theme.panel);
        } else {
            ui.painter()
                .circle_filled(center + Vec2::new(2.5, -1.5), 6.5, theme.accent);
        }
        if response.clicked() {
            self.toggle_theme(ui.ctx());
            self.editor.request_focus();
        }
    }

    fn mode_switch(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        Frame::new()
            .fill(theme.bg)
            .stroke(Stroke::new(1.0, theme.border))
            .corner_radius(9.0)
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (mode, label) in [(ViewMode::Wysiwyg, "Write"), (ViewMode::Source, "Source")] {
                    let active = self.doc.mode() == mode;
                    let button =
                        egui::Button::new(RichText::new(label).size(12.0).color(if active {
                            theme.accent
                        } else {
                            theme.muted
                        }))
                        .fill(if active {
                            theme.accent_soft
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(6.0)
                        .min_size(Vec2::new(58.0, 24.0));
                    if ui.add(button).clicked() {
                        self.doc.set_mode(mode);
                        self.editor.invalidate_layout();
                        self.editor.request_focus();
                    }
                }
            });
    }
    fn outline_panel(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("OUTLINE")
                    .font(theme.mono_font(10.0))
                    .color(theme.faint),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("×").size(16.0).color(theme.muted))
                            .frame(false),
                    )
                    .clicked()
                {
                    self.outline_open = false;
                    self.editor.request_focus();
                }
            });
        });
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        let outline = self.doc.outline();
        if outline.is_empty() {
            ui.label(
                RichText::new("Add a heading to build an outline.")
                    .size(12.5)
                    .color(theme.faint),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("rusk-outline")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in outline {
                    let indent = (item.level.saturating_sub(1) as f32) * 10.0;
                    let active = self.doc.selection().head == item.byte;
                    let width = ui.available_width();
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width, 27.0), Sense::click());
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(rect, 6.0, theme.accent_soft.gamma_multiply(0.5));
                    }
                    let color = if active {
                        theme.accent
                    } else if item.level <= 2 {
                        theme.text
                    } else {
                        theme.muted
                    };
                    ui.painter().text(
                        egui::Pos2::new(rect.left() + 8.0 + indent, rect.center().y),
                        Align2::LEFT_CENTER,
                        &item.text,
                        theme.body_font(if item.level == 1 { 13.5 } else { 12.5 }),
                        color,
                    );
                    if response.clicked() {
                        let caret = Selection::caret(item.byte);
                        self.doc.set_selection(caret);
                        self.editor.reset_vertical_goal();
                        self.editor.scroll_to_caret = true;
                        self.editor.request_focus();
                    }
                }
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let theme = self.theme;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let path = self
                .doc
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Untitled — not saved".to_owned());
            let response = ui.add(
                egui::Label::new(
                    RichText::new(shorten_path(&path, 70))
                        .size(11.0)
                        .color(theme.faint),
                )
                .truncate(),
            );
            response.on_hover_text(path);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let stats = self.doc.stats();
                let (line, col) = self.doc.line_col();
                let sep = RichText::new("·").size(11.0).color(theme.faint);
                ui.label(
                    RichText::new(format!("Ln {line}, Col {col}"))
                        .size(11.0)
                        .color(theme.faint),
                );
                ui.label(sep.clone());
                ui.label(
                    RichText::new(format!("{} words", stats.words))
                        .size(11.0)
                        .color(theme.faint),
                );
                ui.label(sep.clone());
                ui.label(
                    RichText::new(format!("{} min", stats.reading_minutes))
                        .size(11.0)
                        .color(theme.faint),
                );
                ui.label(sep);
                ui.label(
                    RichText::new(if self.doc.mode() == ViewMode::Wysiwyg {
                        "WRITE"
                    } else {
                        "SOURCE"
                    })
                    .font(theme.mono_font(9.5))
                    .color(theme.accent),
                );
            });
            let rect = ui.max_rect();
            ui.painter().line_segment(
                [
                    egui::Pos2::new(rect.left(), rect.top()),
                    egui::Pos2::new(rect.right(), rect.top()),
                ],
                Stroke::new(1.0, theme.border),
            );
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let modal = self.modal.clone();
        match modal {
            Modal::None => {}
            Modal::Unsaved(pending) => {
                let theme = self.theme;
                let mut close = false;
                let mut save = false;
                let mut discard = false;
                egui::Window::new("Unsaved changes")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .default_width(390.0)
                    .show(ctx, |ui| {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("This document has changes that are not saved.")
                                .size(14.0)
                                .color(theme.text),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("Save before continuing?")
                                .size(13.0)
                                .color(theme.muted),
                        );
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            if ui.button("Save").clicked() {
                                save = true;
                            }
                            if ui.button("Discard").clicked() {
                                discard = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Cancel").clicked() {
                                    close = true;
                                }
                            });
                        });
                    });
                if save {
                    let had_path = self.doc.path().is_some();
                    if had_path {
                        match self.doc.save() {
                            Ok(_) => {
                                self.modal = Modal::None;
                                self.apply_pending(pending, ctx);
                            }
                            Err(err) => {
                                self.modal = Modal::Error(format!("Save failed: {err}"));
                            }
                        }
                    } else {
                        self.save_as();
                        if !self.doc.dirty() {
                            self.modal = Modal::None;
                            self.apply_pending(pending, ctx);
                        }
                    }
                } else if discard {
                    self.modal = Modal::None;
                    self.apply_pending(pending, ctx);
                } else if close {
                    self.modal = Modal::None;
                }
            }
            Modal::About => {
                let theme = self.theme;
                egui::Window::new("About Rusk")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .default_width(360.0)
                    .show(ctx, |ui| {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Rusk")
                                .font(theme.heading_font(28.0))
                                .color(theme.text),
                        );
                        ui.label(
                            RichText::new("A source-first Markdown editor.")
                                .size(13.5)
                                .color(theme.muted),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(
                                "The Markdown source is the document. Styling is a projection, \
                                 so nothing is ever lost in a round trip.",
                            )
                            .size(13.0)
                            .color(theme.muted),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new("v0.1.0 · native Rust · wgpu")
                                .font(theme.mono_font(10.5))
                                .color(theme.faint),
                        );
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            self.modal = Modal::None;
                        }
                    });
            }
            Modal::Shortcuts => {
                let theme = self.theme;
                egui::Window::new("Keyboard shortcuts")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .default_width(440.0)
                    .show(ctx, |ui| {
                        let rows = [
                            ("Write / Source", "Ctrl + /"),
                            ("Bold / Italic", "Ctrl + B / I"),
                            ("Inline code", "Ctrl + `"),
                            ("Link", "Ctrl + K"),
                            ("Save / Save as", "Ctrl + S / Ctrl + Shift + S"),
                            ("Open / New", "Ctrl + O / Ctrl + N"),
                            ("Export HTML", "Ctrl + Shift + E"),
                            ("Toggle outline", "Ctrl + Shift + O"),
                            ("Paper / Ink", "Ctrl + Shift + D"),
                            ("Open link", "Ctrl + Click"),
                            ("Move by word", "Ctrl + Arrow"),
                            ("Exit list", "Enter on empty item"),
                        ];
                        for (label, shortcut) in rows {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(label).size(12.5).color(theme.muted));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(
                                        RichText::new(shortcut)
                                            .font(theme.mono_font(10.5))
                                            .color(theme.faint),
                                    );
                                });
                            });
                            ui.add_space(3.0);
                        }
                        ui.add_space(6.0);
                        if ui.button("Close").clicked() {
                            self.modal = Modal::None;
                        }
                    });
            }
            Modal::Error(message) => {
                egui::Window::new("Something went wrong")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .default_width(400.0)
                    .show(ctx, |ui| {
                        ui.label(RichText::new(message).size(13.0));
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            self.modal = Modal::None;
                        }
                    });
            }
        }
    }

    fn apply_pending(&mut self, pending: Pending, ctx: &egui::Context) {
        match pending {
            Pending::Close => {
                self.force_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            other => self.execute(other),
        }
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some(toast) = &self.toast else {
            return;
        };
        if Instant::now() >= toast.until {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(120));
        let theme = self.theme;
        let fill = if toast.error { theme.error } else { theme.text };
        let text = toast.text.clone();
        egui::Area::new(egui::Id::unique("rusk-toast"))
            .anchor(Align2::RIGHT_BOTTOM, Vec2::new(-18.0, -44.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(fill)
                    .corner_radius(9.0)
                    .inner_margin(Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).size(12.5).color(if toast.error {
                            Color32::WHITE
                        } else {
                            theme.page
                        }));
                    });
            });
    }

    fn window_chrome(&mut self, ctx: &egui::Context) {
        let title = window_title(&self.doc);
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

impl eframe::App for RuskApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let theme = self.theme;
        ui.painter().rect_filled(ui.max_rect(), 0.0, theme.bg);

        if self.relayout_frames > 0 {
            self.relayout_frames -= 1;
            self.editor.invalidate_layout();
        }

        #[cfg(target_os = "windows")]
        self.poll_native_menu(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_close(&ctx);
        if self.close_now {
            self.close_now = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if matches!(self.modal, Modal::None)
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().map(|f| f.path().to_path_buf()))
        {
            self.confirm(Pending::OpenPath(path));
        }

        if !self.has_native_menu() {
            egui::Panel::top("rusk-top")
                .exact_size(54.0)
                .frame(
                    Frame::new()
                        .fill(theme.panel)
                        .inner_margin(Margin::symmetric(18, 0)),
                )
                .show(ui, |ui| self.top_bar(ui, &ctx));
        }

        if self.find.open {
            egui::Panel::top("rusk-find")
                .exact_size(self.find.height())
                .frame(
                    Frame::new()
                        .fill(theme.panel)
                        .stroke(Stroke::new(1.0, theme.border))
                        .inner_margin(Margin::symmetric(18, 5)),
                )
                .show(ui, |ui| self.find_bar(ui));
        }

        let body = ui.available_rect_before_wrap();
        let footer_height = 30.0;
        let main_rect = egui::Rect::from_min_max(
            body.min,
            egui::Pos2::new(body.max.x, (body.max.y - footer_height).max(body.min.y)),
        );
        let footer_rect =
            egui::Rect::from_min_max(egui::Pos2::new(body.min.x, main_rect.max.y), body.max);

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(main_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                if self.outline_open {
                    egui::Panel::right("rusk-outline-panel")
                        .exact_size(238.0)
                        .resizable(false)
                        .show_separator_line(false)
                        .frame(
                            Frame::new()
                                .fill(theme.bg)
                                .inner_margin(Margin::symmetric(16, 0)),
                        )
                        .show(ui, |ui| self.outline_panel(ui));
                }

                egui::CentralPanel::default()
                    .frame(Frame::new().fill(theme.bg))
                    .show(ui, |ui| {
                        let interactive = matches!(self.modal, Modal::None);
                        let outcome = self
                            .editor
                            .show(ui, &mut self.doc, &self.theme, interactive);
                        if let Some(url) = outcome.open_url
                            && let Err(err) = open::that_detached(&url)
                        {
                            self.set_toast(format!("Could not open link: {err}"), true);
                        }
                    });
            },
        );

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(footer_rect)
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                Frame::new()
                    .fill(theme.panel)
                    .inner_margin(Margin::symmetric(18, 0))
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.set_min_height(footer_rect.height());
                        self.bottom_bar(ui);
                    });
            },
        );

        self.dialogs(&ctx);
        self.toast(&ctx);
        self.window_chrome(&ctx);
        #[cfg(target_os = "windows")]
        if let Some(menu) = &self.native_menu {
            menu.sync(
                self.doc.mode() == ViewMode::Source,
                self.kind.is_dark(),
                self.outline_open,
            );
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.bg.to_normalized_gamma_f32()
    }
}

fn consume(ctx: &egui::Context, modifiers: Modifiers, key: Key) -> bool {
    let shortcut = KeyboardShortcut::new(modifiers, key);
    ctx.input_mut(|i| i.consume_shortcut(&shortcut))
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> egui::Response {
    let button = egui::Button::new(RichText::new(label).size(13.0))
        .shortcut_text(RichText::new(shortcut).size(11.0));
    ui.add(button)
}

fn window_title(doc: &Document) -> String {
    let name = doc.file_name().unwrap_or_else(|| "Untitled".to_owned());
    if doc.dirty() {
        format!("• {name} — Rusk")
    } else {
        format!("{name} — Rusk")
    }
}

fn shorten_path(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_owned();
    }
    let tail: String = path
        .chars()
        .rev()
        .take(max.saturating_sub(2))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("…{tail}")
}

fn html_document(doc: &Document, theme: &Theme) -> String {
    let body = rusk_core::to_html(&doc.text());
    let title = doc
        .file_name()
        .unwrap_or_else(|| "Untitled".to_owned())
        .trim_end_matches(".md")
        .to_owned();
    let bg = hex(theme.bg);
    let page = hex(theme.page);
    let text = hex(theme.text);
    let muted = hex(theme.muted);
    let border = hex(theme.border);
    let code_bg = hex(theme.code_bg);
    let accent = hex(theme.accent);
    let link = hex(theme.link);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title>
<style>
  :root {{ color-scheme: {}; }}
  html, body {{ margin: 0; background: {bg}; }}
  body {{ font-family: "Segoe UI", -apple-system, BlinkMacSystemFont, "Microsoft YaHei", sans-serif; color: {text}; }}
  article {{ max-width: 760px; margin: 48px auto; background: {page}; padding: 64px 72px; box-sizing: border-box;
             border: 1px solid {border}; border-radius: 3px; line-height: 1.7; font-size: 17px; }}
  h1, h2, h3, h4, h5, h6 {{ line-height: 1.25; margin: 1.8em 0 0.6em; }}
  h1 {{ font-size: 2.1em; }} h2 {{ font-size: 1.65em; }} h3 {{ font-size: 1.32em; }}
  p {{ margin: 0 0 1em; }}
  a {{ color: {link}; }}
  code {{ font-family: "Cascadia Mono", Consolas, monospace; font-size: 0.88em; background: {code_bg};
          border-radius: 4px; padding: 0.12em 0.32em; color: {accent}; }}
  pre {{ background: {code_bg}; border: 1px solid {border}; border-radius: 8px; padding: 18px 20px; overflow: auto; }}
  pre code {{ background: none; padding: 0; color: {text}; font-size: 0.86em; }}
  blockquote {{ margin: 1.4em 0; padding: 0.2em 0 0.2em 1.1em; border-left: 3px solid {border}; color: {muted}; }}
  table {{ border-collapse: collapse; width: 100%; margin: 1.4em 0; }}
  th, td {{ border: 1px solid {border}; padding: 8px 12px; text-align: left; }}
  th {{ background: {code_bg}; }}
  img {{ max-width: 100%; border-radius: 6px; }}
  hr {{ border: 0; border-top: 1px solid {border}; margin: 2em 0; }}
  @media (max-width: 800px) {{ article {{ margin: 0; border: 0; padding: 32px 24px; }} }}
</style>
</head>
<body>
<article>
{body}
</article>
</body>
</html>
"#,
        if theme.kind.is_dark() {
            "dark"
        } else {
            "light"
        },
        bg = bg,
        page = page,
        text = text,
        muted = muted,
        border = border,
        code_bg = code_bg,
        accent = accent,
        link = link,
        title = html_escape(&title),
        body = body,
    )
}

fn hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
