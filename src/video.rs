//! Videos, in web pages and in desktop players. A paused video's frame is
//! read like an image; a playing one is left alone.
//!
//! Chromium (Edge, Chrome) shows a `<video>` to UI Automation as a plain
//! group, like any `<div>`; the HTML tag is only in the IAccessible2
//! attributes screen readers read (`tag:video`). Whether it plays is told by the
//! frames changing (`worker`) and, for a video showing a still picture
//! (music over cover art), by the system's media sessions: a web video's by
//! its tab's title, a desktop player's by the app (`Media`).

#![cfg(windows)]

#![allow(non_snake_case)]

use std::cell::OnceCell;

use windows::Media::Control::{GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::{IDispatch, IServiceProvider};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
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
/// video it controls), or a `<video>` under the point in another branch of
/// one of those ancestors. X, Facebook and Threads lay a transparent `<div>`
/// over the video beside it, not inside it; the object at the point is
/// that layer, and the video is three or four levels up and down again.
pub fn video_at(pt: POINT) -> Option<RECT> {
    let mut object = object_at(pt)?;
    let mut budget = SEARCH_BUDGET;
    for _ in 0..5 {
        if let Some(rect) = tagged_rect(&object, "video") {
            return Some(rect);
        }
        let parent: IAccessible = unsafe { object.accParent().ok()?.cast().ok()? };
        if let Some(rect) = tagged_below(&parent, pt, "video", 4, &mut budget) {
            return Some(rect);
        }
        object = parent;
    }
    None
}

/// Objects looked at below the ancestors, at most: a feed's post is a few
/// dozen, and the search runs each time the cursor rests on a web page.
const SEARCH_BUDGET: u32 = 120;

/// The screen rect of an `<img>` at a screen point inside a link or button
/// that UI Automation reports instead of the image (Threads wraps each
/// photo in a link named "Photo by ...").
pub fn image_at(pt: POINT) -> Option<RECT> {
    let object = object_at(pt)?;
    tagged_rect(&object, "img").or_else(|| tagged_below(&object, pt, "img", 3, &mut SEARCH_BUDGET.clone()))
}

/// The object's rect if its HTML tag is `tag`.
fn tagged_rect(object: &IAccessible, tag: &str) -> Option<RECT> {
    if attributes(object).and_then(|a| attribute(&a, "tag")).as_deref() != Some(tag) {
        return None;
    }
    let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
    unsafe { object.accLocation(&mut x, &mut y, &mut w, &mut h, &VARIANT::from(SELF)).ok()? };
    Some(RECT { left: x, top: y, right: x + w, bottom: y + h })
}

/// An element with HTML tag `tag` holding `pt` among the descendants of
/// `object`, `depth` levels down at most, going only into objects whose
/// rect holds `pt`.
fn tagged_below(object: &IAccessible, pt: POINT, tag: &str, depth: u32, budget: &mut u32) -> Option<RECT> {
    if depth == 0 || *budget == 0 {
        return None;
    }
    let count = unsafe { object.accChildCount() }.ok()?.clamp(0, 64);
    let mut children = vec![VARIANT::default(); count as usize];
    let mut got = 0;
    unsafe { AccessibleChildren(object, 0, &mut children, &mut got).ok()? };
    for child in children.iter().take(got as usize) {
        *budget = budget.saturating_sub(1);
        if *budget == 0 {
            return None;
        }
        let Ok(dispatch) = IDispatch::try_from(child) else {
            continue;
        };
        let Ok(child) = dispatch.cast::<IAccessible>() else {
            continue;
        };
        let (mut x, mut y, mut w, mut h) = (0, 0, 0, 0);
        if unsafe { child.accLocation(&mut x, &mut y, &mut w, &mut h, &VARIANT::from(SELF)) }.is_err() {
            continue;
        }
        if !(x..x + w).contains(&pt.x) || !(y..y + h).contains(&pt.y) {
            continue;
        }
        if let Some(rect) = tagged_rect(&child, tag) {
            return Some(rect);
        }
        if let Some(rect) = tagged_below(&child, pt, tag, depth - 1, budget) {
            return Some(rect);
        }
    }
    None
}

/// The title of a top-level window (a browser shows the tab's title).
pub fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Where a video's media session is found.
#[derive(Clone, Copy, Debug)]
pub enum Media {
    /// A web page's video: the session whose title is in this window's
    /// title (the browser shows the tab's title), not music in another tab.
    Tab(HWND),
    /// A desktop player's video: the session of the app with this process.
    App(u32),
}

/// The system's media sessions (what the volume flyout's media controls
/// show): one per app with media, with its title and playback state.
#[derive(Default)]
pub struct Sessions {
    manager: OnceCell<Option<Manager>>,
}

impl Sessions {
    fn all(&self) -> Vec<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
        let Some(manager) = self.manager.get_or_init(|| Manager::RequestAsync().and_then(|op| op.join()).ok()) else {
            return Vec::new();
        };
        manager.GetSessions().map(|s| s.into_iter().collect()).unwrap_or_default()
    }

    /// True if the media `media` points at is playing.
    pub fn playing(&self, media: Media) -> bool {
        let app = match media {
            Media::App(pid) => app_ids(pid),
            Media::Tab(_) => Vec::new(),
        };
        self.all().into_iter().any(|session| {
            let playing = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).is_ok_and(|s| s == Status::Playing);
            playing
                && match media {
                    Media::Tab(window) => {
                        let title = session.TryGetMediaPropertiesAsync().and_then(|op| op.join()).and_then(|p| p.Title()).map(|t| t.to_string()).unwrap_or_default();
                        matches_window(&title, &window_title(window))
                    }
                    Media::App(_) => session.SourceAppUserModelId().is_ok_and(|id| matches_app(&id.to_string(), &app)),
                }
        })
    }

    /// True if the app with this process has a media session (playing or
    /// not): it is a media player.
    pub fn has_app(&self, pid: u32) -> bool {
        let app = app_ids(pid);
        !app.is_empty() && self.all().into_iter().any(|session| session.SourceAppUserModelId().is_ok_and(|id| matches_app(&id.to_string(), &app)))
    }
}

/// What a media session may name an app by: its package app ID (Microsoft
/// Media Player is "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
/// or, for an app without a package, its executable ("vlc.exe").
fn app_ids(pid: u32) -> Vec<String> {
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else { return Vec::new() };
    let mut ids = Vec::new();
    let mut buf = [0u16; 512];
    let mut len = buf.len() as u32;
    if unsafe { GetApplicationUserModelId(process, &mut len, Some(PWSTR(buf.as_mut_ptr()))) }.is_ok() {
        ids.push(String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]));
    }
    let mut len = buf.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok() {
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        if let Some(exe) = path.rsplit('\\').next() {
            ids.push(exe.to_string());
        }
    }
    unsafe {
        let _ = CloseHandle(process);
    }
    ids
}

/// A session's app ID names one of `app`'s IDs (an executable's name may
/// appear with or without its folder or ".exe").
fn matches_app(session_app: &str, app: &[String]) -> bool {
    let session_app = session_app.to_lowercase();
    app.iter().any(|id| {
        let id = id.to_lowercase();
        let stem = id.trim_end_matches(".exe");
        session_app == id || (stem.len() >= 3 && (session_app == stem || session_app.ends_with(&format!("\\{id}")) || session_app.ends_with(&format!("\\{stem}"))))
    })
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
    fn a_session_belongs_to_the_app_it_names() {
        let player = vec!["Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic".to_string(), "Microsoft.Media.Player.exe".to_string()];
        assert!(matches_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", &player));
        let vlc = vec!["vlc.exe".to_string()];
        assert!(matches_app("vlc.exe", &vlc));
        assert!(matches_app("VLC", &vlc));
        assert!(matches_app("C:\\Program Files\\VideoLAN\\VLC\\vlc.exe", &vlc));
        assert!(!matches_app("Spotify.exe", &vlc));
        assert!(!matches_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic", &vlc));
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
