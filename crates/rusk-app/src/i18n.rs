//! Small, dependency-free UI localization table. Keys are plain strings so a
//! missing translation falls back to English instead of failing to compile.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    Chinese,
}

impl Language {
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "简体中文",
        }
    }

    pub fn is_chinese(self) -> bool {
        matches!(self, Self::Chinese)
    }
}

pub fn t(language: Language, key: &'static str) -> &'static str {
    if language == Language::Chinese {
        match key {
            "file" => "文件",
            "edit" => "编辑",
            "view" => "视图",
            "new" => "新建",
            "open" => "打开…",
            "save" => "保存",
            "save_as" => "另存为…",
            "export" => "导出",
            "html_file" => "HTML 文件…",
            "export_html" => "导出 HTML…",
            "copy_html" => "复制为 HTML",
            "keyboard_shortcuts" => "键盘快捷键",
            "about" => "关于 Rusk",
            "exit" => "退出",
            "settings" => "设置…",
            "undo" => "撤销",
            "redo" => "重做",
            "find" => "查找…",
            "find_next" => "查找下一个",
            "replace" => "替换…",
            "write_mode" => "写作模式",
            "source_mode" => "源码模式",
            "outline" => "大纲",
            "ink_theme" => "深色主题",
            "paper_theme" => "浅色主题",
            "close" => "关闭",
            "cancel" => "取消",
            "discard" => "放弃",
            "save_button" => "保存",
            "settings_title" => "设置",
            "appearance" => "外观",
            "language" => "语言",
            "language_hint" => "界面语言会立即切换。",
            "theme" => "主题",
            "paper" => "纸张",
            "ink" => "墨水",
            "find_label" => "查找",
            "search_document" => "在文档中查找",
            "replace_with" => "替换为",
            "all" => "全部",
            "previous_match" => "上一个匹配 (Shift+Enter)",
            "next_match" => "下一个匹配 (Enter)",
            "no_matches" => "没有匹配",
            "replaced_matches" => "已替换",
            "new_document" => "新文档",
            "opened" => "已打开",
            "saved" => "已保存",
            "save_failed" => "保存失败",
            "export_failed" => "导出失败",
            "exported" => "已导出",
            "html_copied" => "HTML 已复制到剪贴板",
            "could_not_open" => "无法打开",
            "could_not_open_file" => "无法打开文件",
            "could_not_open_link" => "无法打开链接",
            "add_heading" => "添加标题以生成大纲。",
            "words" => "字",
            "minutes" => "分钟",
            "write" => "写作",
            "source" => "源码",
            "untitled" => "未命名",
            "untitled_not_saved" => "未命名 — 未保存",
            "unsaved_title" => "未保存的更改",
            "unsaved_body" => "当前文档有未保存的修改。",
            "unsaved_confirm" => "继续前要保存吗？",
            "about_blurb" => "以源码为唯一真相的 Markdown 编辑器。",
            "about_body" => "Markdown 源码就是文档。样式只是投影，任何往返都不会丢内容。",
            "shortcut_write_source" => "写作 / 源码",
            "shortcut_bold_italic" => "粗体 / 斜体",
            "shortcut_inline_code" => "行内代码",
            "shortcut_link" => "链接",
            "shortcut_save" => "保存 / 另存为",
            "shortcut_open_new" => "打开 / 新建",
            "shortcut_export" => "导出 HTML",
            "shortcut_outline" => "切换大纲",
            "shortcut_theme" => "纸张 / 墨水",
            "shortcut_open_link" => "打开链接",
            "shortcut_move_word" => "按词移动",
            "shortcut_exit_list" => "空列表项退出",
            "language_name" => "界面语言",
            "settings_note" => "所有设置立即生效。",
            _ => key,
        }
    } else {
        match key {
            "file" => "File",
            "edit" => "Edit",
            "view" => "View",
            "new" => "New",
            "open" => "Open…",
            "save" => "Save",
            "save_as" => "Save As…",
            "export" => "Export",
            "html_file" => "HTML file…",
            "export_html" => "Export HTML…",
            "copy_html" => "Copy as HTML",
            "keyboard_shortcuts" => "Keyboard shortcuts",
            "about" => "About Rusk",
            "exit" => "Exit",
            "settings" => "Settings…",
            "undo" => "Undo",
            "redo" => "Redo",
            "find" => "Find…",
            "find_next" => "Find Next",
            "replace" => "Replace…",
            "write_mode" => "Write mode",
            "source_mode" => "Source mode",
            "outline" => "Outline",
            "ink_theme" => "Ink theme",
            "paper_theme" => "Paper theme",
            "close" => "Close",
            "cancel" => "Cancel",
            "discard" => "Discard",
            "save_button" => "Save",
            "settings_title" => "Settings",
            "appearance" => "Appearance",
            "language" => "Language",
            "language_hint" => "The interface language changes immediately.",
            "theme" => "Theme",
            "paper" => "Paper",
            "ink" => "Ink",
            "find_label" => "Find",
            "search_document" => "Search in document",
            "replace_with" => "Replace with",
            "all" => "All",
            "previous_match" => "Previous match (Shift+Enter)",
            "next_match" => "Next match (Enter)",
            "no_matches" => "No matches",
            "replaced_matches" => "Replaced",
            "new_document" => "New document",
            "opened" => "Opened",
            "saved" => "Saved",
            "save_failed" => "Save failed",
            "export_failed" => "Export failed",
            "exported" => "Exported",
            "html_copied" => "HTML copied to clipboard",
            "could_not_open" => "Could not open",
            "could_not_open_file" => "Could not open file",
            "could_not_open_link" => "Could not open link",
            "add_heading" => "Add a heading to build an outline.",
            "words" => "words",
            "minutes" => "min",
            "write" => "Write",
            "source" => "Source",
            "untitled" => "Untitled",
            "untitled_not_saved" => "Untitled — not saved",
            "unsaved_title" => "Unsaved changes",
            "unsaved_body" => "This document has changes that are not saved.",
            "unsaved_confirm" => "Save before continuing?",
            "about_blurb" => "A source-first Markdown editor.",
            "about_body" => {
                "The Markdown source is the document. Styling is a projection, so nothing is ever lost in a round trip."
            }
            "shortcut_write_source" => "Write / Source",
            "shortcut_bold_italic" => "Bold / Italic",
            "shortcut_inline_code" => "Inline code",
            "shortcut_link" => "Link",
            "shortcut_save" => "Save / Save as",
            "shortcut_open_new" => "Open / New",
            "shortcut_export" => "Export HTML",
            "shortcut_outline" => "Toggle outline",
            "shortcut_theme" => "Paper / Ink",
            "shortcut_open_link" => "Open link",
            "shortcut_move_word" => "Move by word",
            "shortcut_exit_list" => "Exit list",
            "language_name" => "Language",
            "settings_note" => "All settings apply immediately.",
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_language_switches_menu_and_ui_strings() {
        assert_eq!(t(Language::English, "file"), "File");
        assert_eq!(t(Language::Chinese, "file"), "文件");
        assert_eq!(t(Language::Chinese, "settings_title"), "设置");
        assert_ne!(
            t(Language::English, "language_hint"),
            t(Language::Chinese, "language_hint")
        );
    }
}
