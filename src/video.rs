//! Videos in web pages. A paused video's frame is read like an image; a
//! playing one is left alone.
//!
//! Chromium (Edge, Chrome) shows a `<video>` to UI Automation as a plain
//! group, like any `<div>`; the HTML tag is only in the IAccessible2
//! attributes screen readers read (`tag:video`). Whether it plays is told by the
//! frames changing (`worker`) and, for a video showing a still picture
//! (music over cover art), by the system's media sessions.

#![allow(non_snake_case)]

use std::cell::OnceCell;

use windows::Media::Control::{GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::IServiceProvider;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::GetWindowTextW;
use windows::core::*;

/// IAccessible2, as far as `get_attributes` (the 18th method after
/// IAccessible's). Only that method is called.
#[interface("E89F726E-C4F4-4C19-BB19-B647D7FA8478")]
unsafe trait IAccessible2: IAccessible {
    fn get_nRelations(&self, n: *mut i32) -> HRESULT;
    fn get_relation(&self, index: i32, relation: *mut *mut core::ffi::c_void) -> HRESULT;
    fn get_relations(&self, max: i32, relations: *mut *mut core::ffi::c_void, n: *mut i32) -> HRESULT;
    fn role(&self, role: *mut i32) -> HRESULT;
    fn scrollTo(&self, kind: i32) -> HRESULT;
    fn scrollToPoint(&self, kind: i32, x: i32, y: i32) -> HRESULT;
    fn get_groupPosition(&self, level: *mut i32, similar: *mut i32, position: *mut i32) -> HRESULT;
    fn get_states(&self, states: *mut i32) -> HRESULT;
    fn get_extendedRole(&self, role: *mut BSTR) -> HRESULT;
    fn get_localizedExtendedRole(&self, role: *mut BSTR) -> HRESULT;
    fn get_nExtendedStates(&self, n: *mut i32) -> HRESULT;
    fn get_extendedStates(&self, max: i32, states: *mut *mut BSTR, n: *mut i32) -> HRESULT;
    fn get_localizedExtendedStates(&self, max: i32, states: *mut *mut BSTR, n: *mut i32) -> HRESULT;
    fn get_uniqueID(&self, id: *mut i32) -> HRESULT;
    fn get_windowHandle(&self, hwnd: *mut HWND) -> HRESULT;
    fn get_indexInParent(&self, index: *mut i32) -> HRESULT;
    fn get_locale(&self, locale: *mut core::ffi::c_void) -> HRESULT;
    fn get_attributes(&self, attributes: *mut BSTR) -> HRESULT;
}

/// The innermost accessible object at a screen point. On a video with its
/// player's controls showing, that is the controls' panel inside it.
fn object_at(pt: POINT) -> Option<IAccessible> {
    let mut accessible: Option<IAccessible> = None;
    let mut child = VARIANT::default();
    unsafe { AccessibleObjectFromPoint(pt, &mut accessible, &mut child).ok()? };
    accessible
}

/// The IAccessible2 attributes of an object ("display:block;tag:video;..."),
/// if the app has them (Chromium does).
fn attributes(object: &IAccessible) -> Option<String> {
    unsafe {
        let ia2: IAccessible2 = object.cast::<IServiceProvider>().ok()?.QueryService(&IAccessible::IID).ok()?;
        let mut attributes = BSTR::default();
        ia2.get_attributes(&mut attributes).ok().ok()?;
        Some(attributes.to_string())
    }
}

/// The value of one attribute (`tag` in "tag:video;...").
fn attribute(attributes: &str, key: &str) -> Option<String> {
    attributes.split(';').find_map(|pair| pair.strip_prefix(key)?.strip_prefix(':').map(str::to_string))
}

const SELF: i32 = 0;

/// The IAccessible2 attributes at a screen point, for printing (`--uia`).
pub fn attributes_at(pt: POINT) -> Option<String> {
    attributes(&object_at(pt)?)
}

/// The screen rect of the HTML `<video>` at a screen point: the object
/// there or one of its ancestors (a player's control panel lies inside the
/// video it controls).
pub fn video_at(pt: POINT) -> Option<RECT> {
    let mut object = object_at(pt)?;
    for _ in 0..4 {
        if attributes(&object).and_then(|a| attribute(&a, "tag")).as_deref() == Some("video") {
            let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
            unsafe { object.accLocation(&mut x, &mut y, &mut w, &mut h, &VARIANT::from(SELF)).ok()? };
            return Some(RECT { left: x, top: y, right: x + w, bottom: y + h });
        }
        object = unsafe { object.accParent().ok()?.cast().ok()? };
    }
    None
}

/// The title of a top-level window (a browser shows the tab's title).
pub fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The system's media sessions (what the volume flyout's media controls
/// show): one per app playing audible media, with its title.
#[derive(Default)]
pub struct Sessions {
    manager: OnceCell<Option<Manager>>,
}

impl Sessions {
    /// True if media is playing whose title is in `window_title`: the video
    /// in that browser tab, not music playing in another app or tab.
    pub fn playing_in(&self, window_title: &str) -> bool {
        let Some(manager) = self.manager.get_or_init(|| Manager::RequestAsync().and_then(|op| op.join()).ok()) else {
            return false;
        };
        let Ok(sessions) = manager.GetSessions() else { return false };
        sessions.into_iter().any(|session| {
            let playing = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).is_ok_and(|s| s == Status::Playing);
            let title = || session.TryGetMediaPropertiesAsync().and_then(|op| op.join()).and_then(|p| p.Title()).map(|t| t.to_string()).unwrap_or_default();
            playing && matches_window(&title(), window_title)
        })
    }
}

/// A media title belongs to a window when the window's title holds it (a
/// tab is titled "<video title> - YouTube - Microsoft Edge"). Very short
/// titles are ignored: they would match too much.
fn matches_window(media_title: &str, window_title: &str) -> bool {
    let media_title = media_title.trim();
    media_title.chars().count() >= 4 && window_title.contains(media_title)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_one_attribute() {
        let a = "display:block;tag:video;class:video-stream html5-main-video;";
        assert_eq!(attribute(a, "tag").as_deref(), Some("video"));
        assert_eq!(attribute(a, "class").as_deref(), Some("video-stream html5-main-video"));
        assert_eq!(attribute(a, "ta"), None);
    }

    #[test]
    fn a_session_belongs_to_the_tab_showing_its_title() {
        let tab = "Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film - YouTube - Personal - Microsoft Edge";
        assert!(matches_window("Big Buck Bunny 60fps 4K - Official Blender Foundation Short Film", tab));
        assert!(!matches_window("Lo-fi beats to study to", tab));
        assert!(!matches_window("You", tab));
        assert!(!matches_window("", tab));
    }
}
