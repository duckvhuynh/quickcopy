//! User settings, kept in %APPDATA%\QuickCopy\settings.json (elsewhere in
//! $XDG_CONFIG_HOME/quickcopy). Starting with Windows is not stored there:
//! the Run registry value is the one source of truth for it.

use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};

use serde::{Deserialize, Serialize};
#[cfg(windows)]
use windows::Win32::System::Registry::*;
#[cfg(windows)]
use windows::core::*;

use crate::common::vk::*;
use crate::i18n::UiLang;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Virtual-key code of the main key.
    pub key: u16,
}

impl Default for Shortcut {
    fn default() -> Self {
        Self { ctrl: true, shift: false, alt: false, key: VK_T.0 }
    }
}

impl Shortcut {
    pub fn matches(&self, key: u16, ctrl: bool, shift: bool, alt: bool) -> bool {
        self.key == key && self.ctrl == ctrl && self.shift == shift && self.alt == alt
    }

    /// Keys that selecting text already uses, so they cannot translate.
    pub fn conflicts(&self) -> bool {
        let k = VIRTUAL_KEY(self.key);
        let navigation = [VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_HOME, VK_END, VK_ESCAPE, VK_TAB, VK_RETURN, VK_BACK, VK_DELETE];
        let modifier = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LSHIFT, VK_RSHIFT, VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LWIN, VK_RWIN];
        navigation.contains(&k) && !self.alt
            || modifier.contains(&k)
            || (self.ctrl && !self.shift && !self.alt && [VK_C, VK_A, VK_INSERT].contains(&k))
    }

    /// "Ctrl+Shift+T" style.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        parts.push(key_name(self.key));
        parts.join("+")
    }
}

fn key_name(key: u16) -> String {
    match key {
        0x30..=0x39 | 0x41..=0x5A => (key as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", key - 0x6F),
        0x20 => "Space".into(),
        0xBA => ";".into(),
        0xBB => "=".into(),
        0xBC => ",".into(),
        0xBD => "-".into(),
        0xBE => ".".into(),
        0xBF => "/".into(),
        0xC0 => "`".into(),
        0xDB => "[".into(),
        0xDC => "\\".into(),
        0xDD => "]".into(),
        0xDE => "'".into(),
        _ => format!("0x{key:02X}"),
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Google Translate code of the language translations go into.
    pub target_language: String,
    pub ui_language: UiLang,
    pub translate_shortcut: Shortcut,
    /// Re-read lines with VieSVTR (when the model is installed).
    pub vietnamese_ocr: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { target_language: "vi".into(), ui_language: UiLang::Vi, translate_shortcut: Shortcut::default(), vietnamese_ocr: true }
    }
}

static CURRENT: LazyLock<RwLock<Settings>> = LazyLock::new(|| RwLock::new(load()));

pub fn get() -> Settings {
    CURRENT.read().unwrap().clone()
}

/// Changes the settings and saves them.
pub fn update(change: impl FnOnce(&mut Settings)) {
    let mut s = CURRENT.write().unwrap();
    change(&mut s);
    if let Err(e) = save(&s) {
        crate::common::log(format!("saving settings failed: {e}"));
    }
}

#[cfg(windows)]
fn path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("QuickCopy").join("settings.json"))
}

#[cfg(not(windows))]
fn path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".config")))?;
    Some(config.join("quickcopy").join("settings.json"))
}

fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(s: &Settings) -> std::io::Result<()> {
    let p = path().ok_or_else(|| std::io::Error::other("APPDATA not set"))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, serde_json::to_string_pretty(s).map_err(std::io::Error::other)?)
}

#[cfg(windows)]
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
#[cfg(windows)]
const RUN_VALUE: PCWSTR = w!("QuickCopy");

/// Whether Windows starts this exe at sign-in (the Run value points here).
#[cfg(windows)]
pub fn start_with_windows() -> bool {
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if ok.is_err() {
        return false;
    }
    let value = String::from_utf16_lossy(&buf[..(size as usize / 2).saturating_sub(1)]);
    std::env::current_exe().is_ok_and(|exe| value.trim_matches('"').eq_ignore_ascii_case(&exe.to_string_lossy()))
}

#[cfg(windows)]
pub fn set_start_with_windows(on: bool) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).ok()?;
        let result = if on {
            let exe = std::env::current_exe().map_err(|e| Error::new(windows::Win32::Foundation::E_FAIL, e.to_string()))?;
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(std::iter::once(0)).collect();
            let bytes = std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2);
            RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes)).ok()
        } else {
            match RegDeleteValueW(key, RUN_VALUE) {
                e if e == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND => Ok(()),
                e => e.ok(),
            }
        };
        let _ = RegCloseKey(key);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_labels_and_matching() {
        let s = Shortcut::default();
        assert_eq!(s.label(), "Ctrl+T");
        assert!(s.matches(VK_T.0, true, false, false));
        assert!(!s.matches(VK_T.0, true, true, false));
        let f = Shortcut { ctrl: false, shift: true, alt: true, key: 0x71 };
        assert_eq!(f.label(), "Shift+Alt+F2");
    }

    #[test]
    fn selection_keys_cannot_be_the_shortcut() {
        let with = |ctrl, key: VIRTUAL_KEY| Shortcut { ctrl, shift: false, alt: false, key: key.0 };
        assert!(with(true, VK_C).conflicts());
        assert!(with(true, VK_A).conflicts());
        assert!(with(false, VK_LEFT).conflicts());
        assert!(with(true, VK_SHIFT).conflicts());
        assert!(!with(true, VK_T).conflicts());
        assert!(!with(false, VK_F2).conflicts());
    }

    #[test]
    fn settings_round_trip_and_fill_missing_fields() {
        let s = Settings { target_language: "ja".into(), ui_language: UiLang::En, ..Default::default() };
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // A file from an older version without some fields still loads.
        let old: Settings = serde_json::from_str(r#"{"target_language":"fr"}"#).unwrap();
        assert_eq!(old.target_language, "fr");
        assert_eq!(old.translate_shortcut, Shortcut::default());
    }

    #[test]
    fn language_lists_match() {
        use crate::languages::{EN, VI};
        assert_eq!(EN.len(), VI.len());
        for code in ["vi", "en", "ja", "zh-CN", "fr"] {
            assert!(EN.iter().any(|(c, _)| *c == code) && VI.iter().any(|(c, _)| *c == code), "{code}");
        }
        // Every Vietnamese name has exactly one "Tiếng", at the start.
        for (_, name) in VI {
            assert!(name.starts_with("Tiếng ") && name.to_lowercase().matches("tiếng").count() == 1, "{name}");
        }
    }
}
