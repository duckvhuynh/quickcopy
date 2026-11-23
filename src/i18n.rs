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
    pub check_updates: &'static str,
    pub updates: &'static str,
    pub check_now: &'static str,
    /// The tray entry and the settings button; "{}" is the version.
    pub update_to: &'static str,
    pub update_available: &'static str,
    /// Under the "available" notification.
    pub update_hint: &'static str,
    pub updated: &'static str,
    pub update_not_checked: &'static str,
    pub update_checking: &'static str,
    pub up_to_date: &'static str,
    pub update_installing: &'static str,
    pub update_restarting: &'static str,
    /// "{}" is one of the `fail_` reasons.
    pub check_failed: &'static str,
    pub update_failed: &'static str,
    pub fail_offline: &'static str,
    pub fail_no_release: &'static str,
    pub fail_server: &'static str,
    pub fail_hash: &'static str,
    pub fail_files: &'static str,
    pub fail_launch: &'static str,
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
    check_updates: "Check for updates automatically",
    updates: "Updates",
    check_now: "Check now",
    update_to: "Update to {}",
    update_available: "QuickCopy {} is available",
    update_hint: "Click here or use the tray menu to update.",
    updated: "Updated to QuickCopy {}",
    update_not_checked: "Not checked yet",
    update_checking: "Checking…",
    up_to_date: "You have the latest version",
    update_installing: "Installing {}…",
    update_restarting: "Restarting…",
    check_failed: "Couldn't check: {}",
    update_failed: "Update failed: {}",
    fail_offline: "no connection",
    fail_no_release: "no release published",
    fail_server: "server error",
    fail_hash: "the download did not match",
    fail_files: "can't replace the app file",
    fail_launch: "the new version didn't start",
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
    check_updates: "Tự động kiểm tra cập nhật",
    updates: "Cập nhật",
    check_now: "Kiểm tra ngay",
    update_to: "Cập nhật lên {}",
    update_available: "Đã có QuickCopy {}",
    update_hint: "Bấm vào đây hoặc dùng menu khay để cập nhật.",
    updated: "Đã cập nhật lên QuickCopy {}",
    update_not_checked: "Chưa kiểm tra",
    update_checking: "Đang kiểm tra…",
    up_to_date: "Bạn đang dùng phiên bản mới nhất",
    update_installing: "Đang cài {}…",
    update_restarting: "Đang khởi động lại…",
    check_failed: "Không kiểm tra được: {}",
    update_failed: "Cập nhật thất bại: {}",
    fail_offline: "không có mạng",
    fail_no_release: "chưa có bản phát hành",
    fail_server: "lỗi máy chủ",
    fail_hash: "tệp tải về không khớp",
    fail_files: "không thay được tệp ứng dụng",
    fail_launch: "bản mới không chạy được",
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
