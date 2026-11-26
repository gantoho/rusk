//! Native Win32 menu bar (via `muda`) so the File menu is drawn by Windows
//! instead of egui. Menu clicks arrive through a global handler that wakes the
//! app with `request_repaint`.

use std::cell::Cell;
use std::sync::{Arc, Mutex};

use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{
    CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, MenuTheme, PredefinedMenuItem, Submenu,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::i18n::{Language, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuCmd {
    New,
    Open,
    Save,
    SaveAs,
    ExportHtml,
    CopyHtml,
    Shortcuts,
    About,
    Settings,
    Quit,
    Undo,
    Redo,
    Find,
    FindNext,
    Replace,
    WriteMode,
    SourceMode,
    ToggleOutline,
    ToggleTheme,
}

impl MenuCmd {
    fn from_id(id: &MenuId) -> Option<Self> {
        Some(match id.0.as_str() {
            "new" => Self::New,
            "open" => Self::Open,
            "save" => Self::Save,
            "save-as" => Self::SaveAs,
            "export-html" => Self::ExportHtml,
            "copy-html" => Self::CopyHtml,
            "shortcuts" => Self::Shortcuts,
            "about" => Self::About,
            "settings" => Self::Settings,
            "quit" => Self::Quit,
            "undo" => Self::Undo,
            "redo" => Self::Redo,
            "find" => Self::Find,
            "find-next" => Self::FindNext,
            "replace" => Self::Replace,
            "write-mode" => Self::WriteMode,
            "source-mode" => Self::SourceMode,
            "outline" => Self::ToggleOutline,
            "ink-theme" => Self::ToggleTheme,
            _ => return None,
        })
    }
}

struct ViewItems {
    write: CheckMenuItem,
    source: CheckMenuItem,
    outline: CheckMenuItem,
    ink: CheckMenuItem,
}

pub struct NativeMenu {
    menu: Menu,
    hwnd: isize,
    events: Arc<Mutex<Vec<MenuCmd>>>,
    view: ViewItems,
    language: Language,
    synced: Cell<Option<(bool, bool, bool)>>,
}

impl NativeMenu {
    pub fn install(cc: &eframe::CreationContext<'_>) -> Option<Self> {
        let handle = cc.window_handle().ok()?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return None;
        };
        let hwnd = win32.hwnd.get();

        let (menu, view) = build_menu(Language::English)?;
        // SAFETY: `hwnd` is the window eframe created for this app.
        unsafe { menu.init_for_hwnd(hwnd) }.ok()?;

        let events: Arc<Mutex<Vec<MenuCmd>>> = Arc::default();
        let queue = Arc::clone(&events);
        let ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(Arc::new(move |event: MenuEvent| {
            if let Some(cmd) = MenuCmd::from_id(&event.id) {
                if let Ok(mut queue) = queue.lock() {
                    queue.push(cmd);
                }
                ctx.request_repaint();
            }
        })));

        Some(Self {
            menu,
            hwnd,
            events,
            view,
            language: Language::English,
            synced: Cell::new(None),
        })
    }

    pub fn take_events(&self) -> Vec<MenuCmd> {
        self.events
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }

    pub fn sync(&self, source_mode: bool, ink: bool, outline: bool) {
        let state = (source_mode, ink, outline);
        if self.synced.get() == Some(state) {
            return;
        }
        self.synced.set(Some(state));
        self.view.write.set_checked(!source_mode);
        self.view.source.set_checked(source_mode);
        self.view.ink.set_checked(ink);
        self.view.outline.set_checked(outline);
    }

    pub fn set_language(&mut self, language: Language) {
        if self.language == language {
            return;
        }
        let Some((menu, view)) = build_menu(language) else {
            return;
        };
        // SAFETY: the HWND stays valid while this menu is attached to it.
        let _ = unsafe { self.menu.remove_for_hwnd(self.hwnd) };
        if unsafe { menu.init_for_hwnd(self.hwnd) }.is_err() {
            return;
        }
        self.menu = menu;
        self.view = view;
        self.language = language;
        self.synced.set(None);
    }

    pub fn set_dark(&self, dark: bool) {
        let theme = if dark {
            MenuTheme::Dark
        } else {
            MenuTheme::Light
        };
        // SAFETY: the HWND stays valid while this menu is attached to it.
        let _ = unsafe { self.menu.set_theme_for_hwnd(self.hwnd, theme) };
    }
}

fn build_menu(language: Language) -> Option<(Menu, ViewItems)> {
    let s = |key| t(language, key);
    let ctrl = Modifiers::CONTROL;
    let ctrl_shift = Modifiers::CONTROL | Modifiers::SHIFT;
    let accel = |mods: Modifiers, code: Code| Some(Accelerator::new(mods, code));

    let write = CheckMenuItem::with_id("write-mode", s("write_mode"), true, true, None);
    let source = CheckMenuItem::with_id("source-mode", s("source_mode"), true, false, None);
    let outline = CheckMenuItem::with_id(
        "outline",
        s("outline"),
        true,
        false,
        accel(ctrl_shift, Code::KeyO),
    );
    let ink = CheckMenuItem::with_id(
        "ink-theme",
        s("ink_theme"),
        true,
        false,
        accel(ctrl_shift, Code::KeyD),
    );

    let edit = Submenu::with_items(
        s("edit"),
        true,
        &[
            &MenuItem::with_id("undo", s("undo"), true, accel(ctrl, Code::KeyZ)),
            &MenuItem::with_id("redo", s("redo"), true, accel(ctrl, Code::KeyY)),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("find", s("find"), true, accel(ctrl, Code::KeyF)),
            &MenuItem::with_id(
                "find-next",
                s("find_next"),
                true,
                accel(Modifiers::empty(), Code::F3),
            ),
            &MenuItem::with_id("replace", s("replace"), true, accel(ctrl, Code::KeyH)),
        ],
    )
    .ok()?;

    let view = Submenu::with_items(
        s("view"),
        true,
        &[
            &write,
            &source,
            &PredefinedMenuItem::separator(),
            &outline,
            &PredefinedMenuItem::separator(),
            &ink,
        ],
    )
    .ok()?;

    let file = Submenu::with_items(
        s("file"),
        true,
        &[
            &MenuItem::with_id("new", s("new"), true, accel(ctrl, Code::KeyN)),
            &MenuItem::with_id("open", s("open"), true, accel(ctrl, Code::KeyO)),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("save", s("save"), true, accel(ctrl, Code::KeyS)),
            &MenuItem::with_id("save-as", s("save_as"), true, accel(ctrl_shift, Code::KeyS)),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(
                "export-html",
                s("export_html"),
                true,
                accel(ctrl_shift, Code::KeyE),
            ),
            &MenuItem::with_id("copy-html", s("copy_html"), true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id(
                "shortcuts",
                s("keyboard_shortcuts"),
                true,
                accel(Modifiers::empty(), Code::F1),
            ),
            &MenuItem::with_id("about", s("about"), true, None),
            &PredefinedMenuItem::separator(),
            &MenuItem::with_id("settings", s("settings"), true, None),
            &MenuItem::with_id("quit", s("exit"), true, None),
        ],
    )
    .ok()?;

    let menu = Menu::new();
    menu.append(&file).ok()?;
    menu.append(&edit).ok()?;
    menu.append(&view).ok()?;
    Some((
        menu,
        ViewItems {
            write,
            source,
            outline,
            ink,
        },
    ))
}
