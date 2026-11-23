//! The app's own words in English and Vietnamese.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Default)]
pub enum UiLang {
    En,
    #[default]
    Vi,
}

impl UiLang {
    /// Language names for the translate-to list, in this language's order.
    pub fn languages(self) -> &'static [(&'static str, &'static str)] {
        match self {
            UiLang::En => crate::languages::EN,
            UiLang::Vi => crate::languages::VI,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(1);

pub fn set(lang: UiLang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn lang() -> UiLang {
    if CURRENT.load(Ordering::Relaxed) == 0 { UiLang::En } else { UiLang::Vi }
}

pub struct Text {
    pub copy: &'static str,
    pub translate: &'static str,
    /// "Translate to {}" with the target language's name.
    pub translate_to: &'static str,
    pub copy_translation: &'static str,
    pub original: &'static str,
    pub show_original: &'static str,
    pub select_all: &'static str,
    pub settings: &'static str,
    pub pause: &'static str,
    pub quit: &'static str,
    pub tooltip: &'static str,
    pub start_with_windows: &'static str,
    pub translate_into: &'static str,
    pub app_language: &'static str,
    pub translate_shortcut: &'static str,
    pub vietnamese_ocr: &'static str,
    pub vietnamese_ocr_missing: &'static str,
    pub press_shortcut: &'static str,
    pub shortcut_taken: &'static str,
    pub search_languages: &'static str,
    pub no_match: &'static str,
}

const EN: Text = Text {
    copy: "Copy",
    translate: "Translate",
    translate_to: "Translate to {}",
    copy_translation: "Copy translation",
    original: "Original",
    show_original: "Show original",
    select_all: "Select all",
    settings: "Settings…",
    pause: "Pause",
    quit: "Quit QuickCopy",
    tooltip: "QuickCopy - copy text from images",
    start_with_windows: "Start with Windows",
    translate_into: "Translate into",
    app_language: "App language",
    translate_shortcut: "Translate shortcut",
    vietnamese_ocr: "Vietnamese text recognition",
    vietnamese_ocr_missing: "Vietnamese model not installed",
    press_shortcut: "Press a shortcut…",
    shortcut_taken: "Used for selecting text",
    search_languages: "Search languages",
    no_match: "No matching language",
};

const VI: Text = Text {
    copy: "Sao chép",
    translate: "Dịch",
    translate_to: "Dịch sang {}",
    copy_translation: "Sao chép bản dịch",
    original: "Bản gốc",
    show_original: "Hiện bản gốc",
    select_all: "Chọn tất cả",
    settings: "Cài đặt…",
    pause: "Tạm dừng",
    quit: "Thoát QuickCopy",
    tooltip: "QuickCopy - sao chép chữ trong ảnh",
    start_with_windows: "Khởi động cùng Windows",
    translate_into: "Dịch sang",
    app_language: "Ngôn ngữ ứng dụng",
    translate_shortcut: "Phím tắt dịch",
    vietnamese_ocr: "Nhận dạng tiếng Việt",
    vietnamese_ocr_missing: "Chưa cài model tiếng Việt",
    press_shortcut: "Nhấn phím tắt…",
    shortcut_taken: "Phím này dùng để chọn chữ",
    search_languages: "Tìm ngôn ngữ",
    no_match: "Không có ngôn ngữ phù hợp",
};

pub fn t() -> &'static Text {
    match lang() {
        UiLang::En => &EN,
        UiLang::Vi => &VI,
    }
}

/// Name of a language code in the current app language.
pub fn language_name(code: &str) -> &'static str {
    lang().languages().iter().find(|(c, _)| *c == code).map_or("?", |(_, n)| n)
}
