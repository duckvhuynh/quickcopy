//! Small HTTP(S) GET over WinHTTP (built into Windows: no TLS library to
//! ship). Translation keeps its own shared connection (translate.rs); the
//! update checker opens a session per request.

use std::ffi::c_void;

use windows::Win32::Networking::WinHttp::*;
use windows::core::*;

pub const USER_AGENT: &str = concat!("QuickCopy/", env!("CARGO_PKG_VERSION"));

pub struct Handle(pub *mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

// WinHTTP handles may be used from any thread.
unsafe impl Send for Handle {}

pub fn check(h: *mut c_void) -> Result<Handle> {
    if h.is_null() { Err(Error::from_thread()) } else { Ok(Handle(h)) }
}

pub fn open_session() -> Result<Handle> {
    unsafe { check(WinHttpOpen(&HSTRING::from(USER_AGENT), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0)) }
}

/// Reads the whole response body of a request already received.
pub fn read_body(request: &Handle) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    unsafe {
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available)?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut c_void, available, &mut read)?;
            body.truncate(start + read as usize);
        }
    }
    Ok(body)
}

pub struct Response {
    pub status: u32,
    pub body: Vec<u8>,
}

/// An absolute http(s) URL, split for WinHTTP.
#[derive(Debug, PartialEq)]
struct Url<'a> {
    secure: bool,
    host: &'a str,
    port: u16,
    /// Path and query, starting with '/'.
    path: &'a str,
}

fn parse_url(url: &str) -> Option<Url<'_>> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else {
        (false, url.strip_prefix("http://")?)
    };
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    (!host.is_empty()).then_some(Url { secure, host, port, path })
}

/// GET `url` with `Accept: accept`. Redirects are followed (WinHTTP does it
/// by default, never from https to http), so a GitHub asset URL arrives at
/// its storage host. Any status comes back; only transport errors fail.
pub fn get(url: &str, accept: &str) -> Result<Response> {
    let u = parse_url(url).ok_or_else(|| Error::new(windows::Win32::Foundation::E_INVALIDARG, format!("bad URL: {url}")))?;
    unsafe {
        let session = open_session()?;
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 15_000, 30_000)?;
        let connect = check(WinHttpConnect(session.0, &HSTRING::from(u.host), u.port, 0))?;
        let flags = if u.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = check(WinHttpOpenRequest(connect.0, w!("GET"), &HSTRING::from(u.path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags))?;
        let headers: Vec<u16> = format!("Accept: {accept}\r\n").encode_utf16().collect();
        WinHttpAddRequestHeaders(request.0, &headers, WINHTTP_ADDREQ_FLAG_ADD)?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut status = 0u32;
        let mut size = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            std::ptr::null_mut(),
        )?;
        Ok(Response { status, body: read_body(&request)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls() {
        let u = parse_url("https://api.github.com/repos/a/b/releases/latest").unwrap();
        assert_eq!(u, Url { secure: true, host: "api.github.com", port: 443, path: "/repos/a/b/releases/latest" });
        let u = parse_url("http://127.0.0.1:8765/latest.json?x=1").unwrap();
        assert_eq!(u, Url { secure: false, host: "127.0.0.1", port: 8765, path: "/latest.json?x=1" });
        assert_eq!(parse_url("http://example.com").unwrap().path, "/");
        assert!(parse_url("ftp://example.com/x").is_none());
        assert!(parse_url("https://:443/x").is_none());
    }
}
