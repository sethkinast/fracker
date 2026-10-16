//! Open the generated page (BRIEF §4.2, seventh pass §3).
//!
//! "Open plan page" calls the freedesktop **OpenURI portal**
//! (`org.freedesktop.portal.OpenURI`) on the session bus — that is the
//! path that works inside the Flatpak sandbox, where spawning `xdg-open`
//! is meaningless. On an unsandboxed dev build with no portal we fall
//! back to the `xdg-open` subprocess.
//!
//! The portal binding is a plain `zbus` call; no `ashpd` dependency, so
//! the offline vendored Flatpak build stays lean (pure-Rust zbus, no
//! libdbus C dependency).

use std::path::Path;

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

/// Open `uri` in the user's browser: OpenURI portal first, then the
/// `xdg-open` subprocess fallback. The combined error is returned only
/// when both paths fail (never fatal to the caller).
pub async fn open_uri(uri: &str) -> Result<(), String> {
    match open_via_portal(uri).await {
        Ok(()) => Ok(()),
        Err(portal_err) => match open_via_xdg_open(uri) {
            Ok(()) => Ok(()),
            Err(xdg_err) => Err(format!(
                "OpenURI portal: {portal_err}; xdg-open: {xdg_err}"
            )),
        },
    }
}

/// `org.freedesktop.portal.OpenURI.OpenURI(parent_window, uri, options)`.
async fn open_via_portal(uri: &str) -> Result<(), String> {
    use std::collections::HashMap;
    use zbus::zvariant::Value;

    let conn = zbus::Connection::session()
        .await
        .map_err(|e| format!("session bus: {e}"))?;
    let options: HashMap<String, Value> = HashMap::new();
    let body = ("", uri, options);
    conn.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.OpenURI"),
        "OpenURI",
        &body,
    )
    .await
    .map_err(|e| format!("OpenURI call: {e}"))?;
    Ok(())
}

/// Dev/unsandboxed fallback. The child is reaped on a detached thread so
/// a long-lived browser handoff never blocks or leaks a zombie.
fn open_via_xdg_open(uri: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("xdg-open")
        .arg(uri)
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
        assert_eq!(
            file_uri(Path::new("/tmp/plan.html")),
            "file:///tmp/plan.html"
        );
    }

    #[test]
    fn file_uri_encodes_non_ascii_bytes() {
        // A UTF-8 path is encoded byte-wise; the URI stays ASCII.
        let uri = file_uri(Path::new("/home/seth/caf\u{e9}.html"));
        assert_eq!(uri, "file:///home/seth/caf%C3%A9.html");
    }
}
