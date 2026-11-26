//! The editing surface. One document, two projections, and a custom text
//! interaction layer that keeps the canonical source authoritative.

mod code;
mod images;
mod input;
mod layout;
mod paint;

use eframe::egui::{self, Id, Rect, Sense, Vec2};
use rusk_core::Document;

use crate::theme::Theme;
use layout::{DocumentLayout, LayoutEngine, LayoutKey};

pub struct Editor {
    engine: LayoutEngine,
    layout: Option<DocumentLayout>,
    focus_id: Id,
    focus_requested: bool,
    pub scroll_to_caret: bool,
    drag_anchor: Option<usize>,
    last_caret_activity: f64,
    ime_preedit: String,
    pub last_ime_rect: Option<Rect>,
    vertical_goal_x: Option<f32>,
    pending_language_edit: Option<std::ops::Range<usize>>,
    hide_completed_inline: bool,
    draft_fence: Option<usize>,
}

#[derive(Default)]
pub struct EditorOutcome {
    pub changed: bool,
    pub open_url: Option<String>,
    pub language_edit: Option<std::ops::Range<usize>>,
}

impl Editor {
    #[must_use]
    pub fn new(ctx: &egui::Context) -> Self {
        let focus_id = Id::unique("rusk-editor-surface");
        ctx.memory_mut(|memory| memory.request_focus(focus_id));
        Self {
            engine: LayoutEngine::default(),
            layout: None,
            focus_id,
            focus_requested: true,
            scroll_to_caret: false,
            drag_anchor: None,
            last_caret_activity: 0.0,
            ime_preedit: String::new(),
            last_ime_rect: None,
            vertical_goal_x: None,
            pending_language_edit: None,
            hide_completed_inline: false,
            draft_fence: None,
        }
    }

    pub fn reset(&mut self, scroll_to_caret: bool) {
        self.layout = None;
        self.scroll_to_caret = scroll_to_caret;
        self.drag_anchor = None;
        self.ime_preedit.clear();
        self.focus_requested = true;
        self.vertical_goal_x = None;
        self.pending_language_edit = None;
        self.hide_completed_inline = false;
        self.draft_fence = None;
    }

    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    pub fn reset_vertical_goal(&mut self) {
        self.vertical_goal_x = None;
    }

    pub fn invalidate_layout(&mut self) {
        self.layout = None;
        self.vertical_goal_x = None;
    }

    pub fn focus_id(&self) -> Id {
        self.focus_id
    }

    fn key(&self, doc: &Document, width: f32, theme: &Theme) -> LayoutKey {
        LayoutKey {
            revision: doc.revision(),
            selection: doc.selection(),
            width_px: width.round() as i32,
            mode: doc.mode(),
            dark: theme.kind.is_dark(),
            hide_completed: self.hide_completed_inline,
            draft_fence: self.draft_fence,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        doc: &mut Document,
        theme: &Theme,
        interactive: bool,
    ) -> EditorOutcome {
        let ctx = ui.ctx().clone();
        let width = (ui.available_width() - 6.0).max(360.0);
        let min_height = ui.available_height().max(320.0);
        let mut outcome = EditorOutcome::default();

        if self.focus_requested {
            ui.memory_mut(|memory| memory.request_focus(self.focus_id));
            self.focus_requested = false;
        }

        let key = self.key(doc, width, theme);
        if self.layout.as_ref().is_none_or(|layout| layout.key != key) {
            let layout = {
                let mut previous = self.layout.take();
                self.engine.hide_completed = self.hide_completed_inline;
                self.engine.draft_fence = self.draft_fence;
                self.engine
                    .build(&ctx, doc, theme, width, min_height, previous.as_mut())
            };
            self.layout = Some(layout);
        }

        let old_revision = doc.revision();
        let old_selection = doc.selection();

        egui::ScrollArea::vertical()
            .id_salt("rusk-document-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let height = self.layout.as_ref().map_or(min_height, |l| l.height);
                let (allocated, painter) =
                    ui.allocate_painter(Vec2::new(width, height), Sense::hover());
                // Register the focus id as a real widget id. egui's focus
                // dead-man switch drops focus ids that no widget used this
                // frame, so an ad-hoc id would silently lose keyboard input.
                let response = ui.interact(allocated.rect, self.focus_id, Sense::click_and_drag());
                let origin = response.rect.min;
                painter.rect_filled(response.rect, 0.0, theme.bg);

                let focused = ui.memory(|memory| memory.has_focus(self.focus_id));
                if interactive && focused {
                    ui.memory_mut(|memory| {
                        memory.set_focus_lock_filter(
                            self.focus_id,
                            egui::EventFilter {
                                tab: true,
                                horizontal_arrows: true,
                                vertical_arrows: true,
                                escape: true,
                            },
                        );
                    });
                }

                if interactive {
                    let layout = self.layout.take().expect("layout exists");
                    if let Some(open_url) =
                        self.handle_pointer(ui, &response, &layout, doc, origin, focused)
                    {
                        outcome.open_url = Some(open_url);
                    }
                    if let Some(range) = self.pending_language_edit.take() {
                        outcome.language_edit = Some(range);
                    }
                    if focused {
                        let changed = self.handle_keyboard(ui, &layout, doc, origin);
                        if changed {
                            outcome.changed = true;
                        }
                    }
                    self.layout = Some(layout);
                    if !focused {
                        self.ime_preedit.clear();
                    }
                }

                let new_key = self.key(doc, width, theme);
                if self
                    .layout
                    .as_ref()
                    .is_none_or(|layout| layout.key != new_key)
                {
                    let layout = {
                        let mut previous = self.layout.take();
                        self.engine.hide_completed = self.hide_completed_inline;
                        self.engine.draft_fence = self.draft_fence;
                        self.engine
                            .build(&ctx, doc, theme, width, min_height, previous.as_mut())
                    };
                    self.layout = Some(layout);
                }
                let layout = self.layout.as_ref().expect("layout exists");

                if doc.revision() != old_revision || doc.selection() != old_selection {
                    self.last_caret_activity = ui.input(|i| i.time);
                }

                let caret_visible = self.caret_visible(&ctx, doc);
                paint::paint(
                    ui,
                    &painter,
                    layout,
                    doc,
                    theme,
                    origin,
                    focused && interactive,
                    caret_visible,
                    &self.ime_preedit,
                );

                if focused && interactive {
                    let caret = paint::caret_rect_local(layout, doc);
                    if let Some(caret) = caret {
                        let caret_screen = caret.translate(origin.to_vec2());
                        self.last_ime_rect = Some(caret_screen);
                        let to_global = ctx
                            .layer_transform_to_global(ui.layer_id())
                            .unwrap_or_default();
                        ui.output_mut(|output| {
                            output.ime = Some(egui::output::IMEOutput {
                                purpose: egui::IMEPurpose::Normal,
                                rect: to_global * response.rect,
                                cursor_rect: to_global * caret_screen,
                                should_interrupt_composition: false,
                            });
                        });
                    }
                } else {
                    self.last_ime_rect = None;
                }

                if self.scroll_to_caret
                    && let Some(caret) = paint::caret_rect_local(layout, doc)
                {
                    let screen = caret.translate(origin.to_vec2());
                    // Only ask the ScrollArea to move when the caret actually
                    // left the comfortable reading area. Calling
                    // `scroll_to_rect` for every keystroke makes the page
                    // twitch by a few pixels when typing near an edge.
                    if !ui.clip_rect().shrink(18.0).contains_rect(screen) {
                        ui.scroll_to_rect(screen.expand(36.0), None);
                    }
                    self.scroll_to_caret = false;
                }
            });

        if doc.selection() != old_selection {
            self.last_caret_activity = ctx.input(|i| i.time);
        }
        outcome.changed |= doc.revision() != old_revision;
        outcome
    }

    fn caret_visible(&self, ctx: &egui::Context, _doc: &Document) -> bool {
        let time = ctx.input(|i| i.time);
        if time - self.last_caret_activity < 0.55 {
            return true;
        }
        ((time * 1.8) as i64) % 2 == 0
    }
}
