//! The app's look, following Windows: light or dark from the "app mode"
//! setting and the accent color from the personalization palette. Shared by
//! the popover, the tray menu, the settings window and the selection.
//! `refresh` re-reads the system; call it when a surface is shown or when
//! Windows broadcasts a color change.

#![cfg(windows)]

use std::sync::RwLock;

use windows::Win32::System::Registry::*;
use windows::core::*;

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub panel: Rgb,
    pub text: Rgb,
    pub subtle: Rgb,
    pub border: (Rgb, f32),
    pub control_border: (Rgb, f32),
    pub hover: (Rgb, f32),
    /// Window outline (the panel color, a little stronger).
    pub window_border: Rgb,
    /// Filled controls: an on toggle, the chosen segment, focus outlines.
    pub accent: Rgb,
    /// Text and knobs drawn on `accent`.
    pub on_accent: Rgb,
    /// The text selection, like the system's own.
    pub selection: Rgb,
    pub toggle_off: (Rgb, Rgb),
    pub error: Rgb,
}

/// Panel corner radius (the popover, the menu).
pub const RADIUS: f32 = 6.0;
/// Inputs and segmented controls.
pub const CONTROL_RADIUS: f32 = 5.0;
/// Hover highlights inside a panel.
pub const HOVER_RADIUS: f32 = 4.0;

/// Windows' default blue, as its palette shades (Light2, Base, Dark1).
const DEFAULT_ACCENT: [Rgb; 3] = [[0x99, 0xEB, 0xFF], [0x00, 0x78, 0xD4], [0x00, 0x5A, 0x9E]];

impl Palette {
    fn new(dark: bool, accent: [Rgb; 3]) -> Palette {
        let [light2, base, dark1] = accent;
        let fill = if dark { light2 } else { dark1 };
        if dark {
            Palette {
                dark,
                panel: [44, 44, 44],
                text: [240, 240, 240],
                subtle: [160, 160, 160],
                border: ([255, 255, 255], 0.10),
                control_border: ([255, 255, 255], 0.16),
                hover: ([255, 255, 255], 0.08),
                window_border: [66, 66, 66],
                accent: fill,
                on_accent: on(fill),
                selection: base,
                toggle_off: ([70, 70, 70], [170, 170, 170]),
                error: [235, 120, 110],
            }
        } else {
            Palette {
                dark,
                panel: [249, 249, 249],
                text: [26, 26, 26],
                subtle: [96, 96, 96],
                border: ([0, 0, 0], 0.10),
                control_border: ([0, 0, 0], 0.16),
                hover: ([0, 0, 0], 0.06),
                window_border: [214, 214, 214],
                accent: fill,
                on_accent: on(fill),
                selection: base,
                toggle_off: ([214, 214, 214], [96, 96, 96]),
                error: [196, 43, 28],
            }
        }
    }
}

/// Black or white, whichever reads better on `c`.
fn on(c: Rgb) -> Rgb {
    let luma = 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
    if luma > 150.0 { [0, 0, 0] } else { [255, 255, 255] }
}

static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);

pub fn get() -> Palette {
    if let Some(p) = *CURRENT.read().unwrap_or_else(|e| e.into_inner()) {
        return p;
    }
    refresh();
    get()
}

/// Re-reads the system colors; true when they changed.
pub fn refresh() -> bool {
    // QC_THEME=light|dark overrides the system, for checking both looks.
    let dark = match std::env::var("QC_THEME").as_deref() {
        Ok("light") => false,
        Ok("dark") => true,
        _ => !system_light("AppsUseLightTheme"),
    };
    let p = Palette::new(dark, accent());
    let mut current = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    let changed = *current != Some(p);
    *current = Some(p);
    changed
}

/// Whether the taskbar (and the tray) is light, for the tray icon.
pub fn taskbar_light() -> bool {
    system_light("SystemUsesLightTheme")
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
const ACCENT: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent");

fn system_light(value: &str) -> bool {
    let mut v = 0u32;
    let mut size = size_of::<u32>() as u32;
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PERSONALIZE, &HSTRING::from(value), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut size))
    };
    // Missing means the default, which is light.
    ok.is_err() || v != 0
}

/// Light2, Base and Dark1 from the accent palette (eight RGBA shades,
/// lightest first, Base at index 3).
fn accent() -> [Rgb; 3] {
    let mut buf = [0u8; 32];
    let mut size = buf.len() as u32;
    let ok = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, ACCENT, w!("AccentPalette"), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr().cast()), Some(&mut size))
    };
    if ok.is_err() || size < 20 {
        return DEFAULT_ACCENT;
    }
    let shade = |i: usize| [buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]];
    [shade(1), shade(3), shade(4)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_on_accent_contrasts() {
        assert_eq!(on([0x99, 0xEB, 0xFF]), [0, 0, 0]);
        assert_eq!(on([0x00, 0x5A, 0x9E]), [255, 255, 255]);
    }

    #[test]
    fn modes_pick_their_accent_shade() {
        assert_eq!(Palette::new(true, DEFAULT_ACCENT).accent, DEFAULT_ACCENT[0]);
        assert_eq!(Palette::new(false, DEFAULT_ACCENT).accent, DEFAULT_ACCENT[2]);
        assert!(Palette::new(false, DEFAULT_ACCENT).text[0] < 60);
    }
}
