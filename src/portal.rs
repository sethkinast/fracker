//! Open the generated page (BRIEF §4.2, seventh pass §3, eighth pass §4).
//!
//! **Why the old path silently did nothing.** v0.2.0 called
//! `org.freedesktop.portal.OpenURI.OpenURI(parent, "file:///…", {})`.
//! Two things were wrong inside the Flatpak sandbox:
//!
//! 1. **The URI was a sandbox path.** `OpenURI` hands the string to the
//!    host's opener, which has never seen `/home/seth/.var/app/…` — the
//!    portal cannot resolve the path the sandbox sent. The sandbox-correct
//!    route for an *app-owned* file is `OpenURI.OpenFile`, which takes a
//!    **file descriptor**: the portal exports the FD through the document
//!    portal and opens the resulting host path. This is what we call now.
//! 2. **Success was assumed.** The method call merely *delivers* a request;
//!    the real answer arrives later as a `org.freedesktop.portal.Request`
//!    `Response` signal. The old code returned `Ok(())` the moment the call
//!    was delivered, so a portal that refused (no handler, denied, bad path)
//!    still looked like success and the `xdg-open` fallback never ran.
//!    We now predict the request handle, subscribe to `Response` *before*
//!    the call (race-free), and report the actual outcome.
//!
//! Any failure is returned as an error so the caller can surface it (tray
//! status); nothing is swallowed. The `xdg-open` subprocess fallback fires
//! only when the portal itself is unreachable (an unsandboxed dev build
//! with no portal) — never to paper over a portal that answered "no".
//!
//! All portal bindings are plain `zbus` calls (pure-Rust D-Bus); the one
//! extra crate used here, `futures-util`, is already in the vendored
//! `cargo-sources.json` as a `ksni` dependency, so the offline Flatpak
//! build gains nothing new to fetch.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;

use zbus::zvariant::{Fd, OwnedValue, Value};

/// The portal's well-known name and object/interface paths.
const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const OPENURI_IFACE: &str = "org.freedesktop.portal.OpenURI";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// `org.freedesktop.portal.Request` response code for success; anything
/// else is a refusal (1 = user cancelled, 2 = other error).
pub const RESPONSE_SUCCESS: u32 = 0;

/// How long to wait for the portal's `Response` signal before giving up
/// and reporting a visible failure. `OpenFile` normally answers at once
/// (or with an error when no handler exists); the bound only stops a
/// wedged portal from hanging the tray's open verb forever.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(90);

/// Percent-encode a filesystem path into a `file://` URI (RFC 3986:
/// unreserved + `/` stay literal, everything else becomes `%XX`).
/// Non-UTF-8 bytes are encoded byte-wise, so the URI is always valid.
pub fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for b in path.as_os_str().as_bytes() {
        match b {
            b'/' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                s.push(*b as char)
            }
            _ => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// Why a portal open attempt failed, which decides fallback behaviour.
#[derive(Debug)]
enum PortalError {
    /// No session bus / no portal service: an unsandboxed dev build. Safe
    /// to fall back to `xdg-open`.
    Unavailable(String),
    /// The portal answered and refused (or timed out). Falling back would
    /// hide a real failure, so we surface it instead.
    Failed(String),
}

/// Open the app-owned report `path` with the user's default handler.
///
/// Portal `OpenFile` (with FD passing) first; `xdg-open` only when the
/// portal is absent. The portal's own refusal is returned, never hidden.
pub async fn open_plan(path: &Path) -> Result<(), String> {
    match open_via_openfile_portal(path).await {
        Ok(()) => Ok(()),
        Err(PortalError::Unavailable(why)) => match open_via_xdg_open(path) {
            Ok(()) => Ok(()),
            Err(fallback) => Err(format!(
                "ScreenCast/OpenFile portal unavailable ({why}); xdg-open fallback: {fallback}"
            )),
        },
        Err(PortalError::Failed(why)) => Err(why),
    }
}

/// Open an `http://` URL (ninth pass §2) with the user's default browser.
///
/// This is the loopback-served plan page route: portal `OpenURI` first
/// (default browser, no chooser, no FD handoff), `xdg-open` only when the
/// portal itself is unreachable. A portal refusal is surfaced, never
/// hidden.
pub async fn open_url(url: &str) -> Result<(), String> {
    match open_via_openuri_portal(url).await {
        Ok(()) => Ok(()),
        Err(PortalError::Unavailable(why)) => match open_via_xdg_open_url(url) {
            Ok(()) => Ok(()),
            Err(fallback) => Err(format!(
                "OpenURI portal unavailable ({why}); xdg-open fallback: {fallback}"
            )),
        },
        Err(PortalError::Failed(why)) => Err(why),
    }
}

/// `org.freedesktop.portal.OpenURI.OpenURI(parent_window, uri, options)`,
/// awaiting the request's `Response` signal.
async fn open_via_openuri_portal(url: &str) -> Result<(), PortalError> {
    let conn = zbus::Connection::session()
        .await
        .map_err(|e| PortalError::Unavailable(format!("session bus: {e}")))?;
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_owned())
        .ok_or_else(|| PortalError::Unavailable("session bus has no unique name".to_owned()))?;

    let token = handle_token();
    let req_path = request_path(&unique, &token);

    let mut options: HashMap<String, Value> = HashMap::new();
    options.insert("handle_token".to_owned(), Value::from(token));
    let body = ("", url, options);
    portal_open(&conn, &req_path, || async {
        conn.call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(OPENURI_IFACE),
            "OpenURI",
            &body,
        )
        .await
        .map(|_| ())
        .map_err(|e| PortalError::Unavailable(format!("OpenURI call: {e}")))
    })
    .await
}

/// `org.freedesktop.portal.OpenURI.OpenFile(parent_window, fd, options)`
/// with an FD opened from the app-owned file, awaiting the `Response`.
async fn open_via_openfile_portal(path: &Path) -> Result<(), PortalError> {
    let file = std::fs::File::open(path)
        .map_err(|e| PortalError::Failed(format!("open {}: {e}", path.display())))?;

    let conn = zbus::Connection::session()
        .await
        .map_err(|e| PortalError::Unavailable(format!("session bus: {e}")))?;

    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_owned())
        .ok_or_else(|| PortalError::Unavailable("session bus has no unique name".to_owned()))?;

    let token = handle_token();
    let req_path = request_path(&unique, &token);

    let mut options: HashMap<String, Value> = HashMap::new();
    options.insert("handle_token".to_owned(), Value::from(token));
    let fd = Fd::from(std::os::fd::OwnedFd::from(file));
    let body = ("", fd, options);
    portal_open(&conn, &req_path, || async {
        conn.call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(OPENURI_IFACE),
            "OpenFile",
            &body,
        )
        .await
        .map(|_| ())
        .map_err(|e| PortalError::Unavailable(format!("OpenFile call: {e}")))
    })
    .await
}

/// Subscribe to a portal request's `Response` *before* sending the call
/// (the request handle is predictable from the token, so the signal can
/// never race the subscription), send it, then await and classify the
/// answer. Shared by `OpenURI` and `OpenFile`.
async fn portal_open<F, Fut>(
    conn: &zbus::Connection,
    req_path: &str,
    send: F,
) -> Result<(), PortalError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), PortalError>>,
{
    let req_proxy = zbus::Proxy::new(conn, PORTAL_DEST, req_path, REQUEST_IFACE)
        .await
        .map_err(|e| PortalError::Unavailable(format!("portal request proxy: {e}")))?;
    let mut responses = req_proxy
        .receive_signal("Response")
        .await
        .map_err(|e| PortalError::Unavailable(format!("subscribe Response: {e}")))?;

    send().await?;

    let signal = tokio::time::timeout(RESPONSE_TIMEOUT, responses.next())
        .await
        .map_err(|_| PortalError::Failed("portal did not answer the open request in time".to_owned()))?
        .ok_or_else(|| PortalError::Failed("portal response stream ended early".to_owned()))?;

    let (code, _results): (u32, HashMap<String, OwnedValue>) = signal
        .body()
        .deserialize()
        .map_err(|e| PortalError::Failed(format!("malformed portal response: {e}")))?;
    classify_response(code)
}

/// Turn a portal `Response` code into an outcome. Pure + unit-tested.
fn classify_response(code: u32) -> Result<(), PortalError> {
    if code == RESPONSE_SUCCESS {
        Ok(())
    } else {
        Err(PortalError::Failed(match code {
            1 => "portal open cancelled (response 1)".to_owned(),
            2 => "portal open failed (response 2)".to_owned(),
            other => format!("portal open failed (response {other})"),
        }))
    }
}

/// Predict the portal request-object path for a connection's unique name
/// and handle token. Per the xdg-desktop-portal spec the initial `:` is
/// dropped and every `.` becomes `_`:
/// `:1.42` + `fracker_7_0` -> `/org/freedesktop/portal/desktop/request/1_42/fracker_7_0`.
pub fn request_path(unique_name: &str, token: &str) -> String {
    let sender: String = unique_name
        .trim_start_matches(':')
        .chars()
        .map(|c| if c == '.' { '_' } else { c })
        .collect();
    format!("/org/freedesktop/portal/desktop/request/{sender}/{token}")
}

/// A fresh handle token that is a valid object-path element (letters,
/// digits, underscore only): `fracker_<pid>_<counter>`. Unique per call
/// within the process, so overlapping opens cannot alias a request.
///
/// Shared with the ScreenCast negotiation (§2): one counter means a
/// ScreenCast `handle_token` can never alias an OpenURI one in the same
/// process.
pub(crate) fn handle_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("fracker_{}_{}", std::process::id(), n)
}

/// Dev/unsandboxed fallback: `xdg-open <file-uri>`. The child is reaped on
/// a detached thread so a long-lived browser handoff never blocks or leaks
/// a zombie.
fn open_via_xdg_open(path: &Path) -> Result<(), String> {
    open_via_xdg_open_url(&file_uri(path))
}

/// Dev/unsandboxed fallback for a plain URL (the loopback plan page).
fn open_via_xdg_open_url(url: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("xdg-open")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn xdg-open: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_uri_encodes_spaces_and_keeps_slashes() {
        assert_eq!(
            file_uri(Path::new("/home/seth/My Plans/plan.html")),
            "file:///home/seth/My%20Plans/plan.html"
        );
        assert_eq!(file_uri(Path::new("/tmp/plan.html")), "file:///tmp/plan.html");
    }

    #[test]
    fn file_uri_encodes_non_ascii_bytes() {
        // A UTF-8 path is encoded byte-wise; the URI stays ASCII.
        let uri = file_uri(Path::new("/home/seth/caf\u{e9}.html"));
        assert_eq!(uri, "file:///home/seth/caf%C3%A9.html");
    }

    #[test]
    fn request_path_matches_the_portal_spec() {
        // `:` stripped, `.` -> `_`, token appended as the last element.
        assert_eq!(
            request_path(":1.42", "fracker_7_0"),
            "/org/freedesktop/portal/desktop/request/1_42/fracker_7_0"
        );
        assert_eq!(
            request_path(":1.1234", "fracker_9_3"),
            "/org/freedesktop/portal/desktop/request/1_1234/fracker_9_3"
        );
    }

    #[test]
    fn handle_tokens_are_valid_path_elements_and_unique() {
        let a = handle_token();
        let b = handle_token();
        assert_ne!(a, b, "tokens must not alias across calls");
        for t in [&a, &b] {
            assert!(!t.is_empty());
            assert!(
                t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "token {t:?} must be a valid object-path element"
            );
        }
    }

    #[test]
    fn response_codes_classify_failures_visibly() {
        // Success is the only Ok; every refusal names the code, never silent.
        assert!(matches!(classify_response(0), Ok(())));
        for code in [1u32, 2, 7] {
            match classify_response(code) {
                Err(PortalError::Failed(msg)) => assert!(
                    msg.contains(&code.to_string()),
                    "message should name the code: {msg:?}"
                ),
                other => panic!("code {code} should be a visible failure, got {other:?}"),
            }
        }
    }
}
