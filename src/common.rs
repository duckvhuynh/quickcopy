//! What the platform-independent modules share: the rect and point types,
//! virtual-key codes and logging. On Windows these are the Win32 types
//! themselves; elsewhere, stand-ins with the same fields and values, so the
//! text, layout and settings logic builds and is tested on Linux too (see
//! docs/linux.md).

#[cfg(windows)]
pub use windows::Win32::Foundation::{POINT, RECT};

#[cfg(not(windows))]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RECT {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[cfg(not(windows))]
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct POINT {
    pub x: i32,
    pub y: i32,
}

#[cfg(windows)]
pub use windows::Win32::UI::Input::KeyboardAndMouse as vk;
#[cfg(not(windows))]
pub use virtual_keys as vk;

macro_rules! virtual_keys {
    ($($name:ident = $code:literal),* $(,)?) => {
        /// Windows virtual-key codes: settings.json stores shortcuts as these
        /// on every platform. Only the keys the settings logic names.
        #[cfg(any(not(windows), test))]
        #[allow(non_camel_case_types, dead_code)]
        pub mod virtual_keys {
            #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
            pub struct VIRTUAL_KEY(pub u16);
            $(pub const $name: VIRTUAL_KEY = VIRTUAL_KEY($code);)*
        }

        #[cfg(all(windows, test))]
        #[test]
        fn virtual_keys_match_windows() {
            $(assert_eq!(virtual_keys::$name.0, windows::Win32::UI::Input::KeyboardAndMouse::$name.0, stringify!($name));)*
        }
    };
}

virtual_keys! {
    VK_BACK = 0x08, VK_TAB = 0x09, VK_RETURN = 0x0D, VK_SHIFT = 0x10, VK_CONTROL = 0x11, VK_MENU = 0x12,
    VK_ESCAPE = 0x1B, VK_END = 0x23, VK_HOME = 0x24, VK_LEFT = 0x25, VK_UP = 0x26, VK_RIGHT = 0x27,
    VK_DOWN = 0x28, VK_INSERT = 0x2D, VK_DELETE = 0x2E, VK_A = 0x41, VK_C = 0x43, VK_T = 0x54,
    VK_LWIN = 0x5B, VK_RWIN = 0x5C, VK_F2 = 0x71, VK_LSHIFT = 0xA0, VK_RSHIFT = 0xA1,
    VK_LCONTROL = 0xA2, VK_RCONTROL = 0xA3, VK_LMENU = 0xA4, VK_RMENU = 0xA5,
}

/// Logs to stderr in debug builds, or in release builds when QC_LOG is set
/// (stderr must then be redirected, as release builds have no console).
pub fn log(msg: String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| cfg!(debug_assertions) || std::env::var_os("QC_LOG").is_some()) {
        eprintln!("[quickcopy] {msg}");
    }
}
