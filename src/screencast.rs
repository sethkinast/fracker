//! ScreenCast → PipeWire frame source for Capture Inventory (eighth pass §2).
//!
//! When Seth clicks **Capture Inventory** the resident opens ONE
//! `org.freedesktop.portal.ScreenCast` session and then pulls frames off
//! PipeWire. This module owns both halves:
//!
//! 1. [`negotiate`] drives the portal over zbus exactly per the spec:
//!    `CreateSession` → `SelectSources` (monitor/window, persistent
//!    consent) → `Start` → `OpenPipeWireRemote`, waiting on each
//!    request's `Response` signal (subscribed *before* the call, using
//!    the predictable handle path) and persisting the returned
//!    `restore_token` in app data so consent is not re-demanded every
//!    run.
//! 2. [`ScreenCastSource`] consumes the PipeWire remote fd and yields
//!    grayscale frames through the same [`crate::capture::FrameSource`]
//!    trait the box-side sequence test uses.
//!
//! **Why `dlopen` and not a link-time dependency.** The brief says the
//! freedesktop 24.08 runtime ships `libpipewire`; the *build box* does
//! not (no `libpipewire-0.3.so`, no `pipewire` crate vendored, no crates.io
//! to add one — the offline Flatpak build must stay green from the
//! committed `cargo-sources.json`). Loading `libpipewire-0.3.so.0` at
//! runtime keeps the default `cargo build` green on the box while still
//! using the runtime's real library inside the Flatpak. No network
//! syscalls are added; frames are converted to grayscale in-process and
//! never leave the app.
//!
//! Everything that can be checked without the runtime — the SPA pod byte
//! layout, the portal request paths, the frame conversion — is pure and
//! unit-tested here. The portal consent dialog, the live PipeWire stream
//! and the actual scroll pacing are only verifiable on Seth's machine.

#![allow(dead_code)]

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CString};
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Serialize;
use zbus::zvariant::{DynamicType, ObjectPath, OwnedValue, Value};

use crate::capture::{CaptureLog, FrameSource};
use crate::ocr::GrayImage;

// ---------------------------------------------------------------------------
// Portal (org.freedesktop.portal.ScreenCast)
// ---------------------------------------------------------------------------

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";

/// Portal response code for "the user agreed" (0); everything else is a
/// refusal and is surfaced, never swallowed.
const RESPONSE_SUCCESS: u32 = 0;

/// The D-Bus reply signature of `OpenPipeWireRemote`: a **bare Unix file
/// descriptor**, `h`.
///
/// **The trap (eleventh pass).** `CreateSession`, `SelectSources` and
/// `Start` all reply with a `a{sv}` results dict, which zbus hands back as
/// a Variant. `OpenPipeWireRemote` is the odd one out: it replies with the
/// fd itself. Deserializing that reply into `OwnedValue` (whose `Type` is
/// `v`) makes zbus refuse it with
/// `Signature mismatch: got 'h', expected 'v'` *before* the fd is ever
/// extracted. The reply is therefore typed as an fd end to end, and this
/// constant — pinned by a unit test — names the signature that path
/// expects so the regression cannot silently return.
const OPEN_PIPEWIRE_REMOTE_REPLY_SIGNATURE: &str = "h";

/// A ScreenCast negotiation failure that **names the stage it happened
/// at** (eleventh pass §2, addendum 40a).
///
/// Pre-fix, every negotiation failure was flattened to a `String`, so the
/// tray had no choice but to render it under whatever stage the machine
/// was last in — "consent pending" — even when the failure was the
/// post-consent `OpenPipeWireRemote` fd. Carrying the stage lets the tray
/// name what actually failed, never the last state the machine was in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiateError {
    /// The lifecycle stage that failed: `"pipewire"` for the up-front
    /// symbol preflight (before any portal dialog), `"consent pending"`
    /// for the consent-gated calls, `"remote-fd"` for the
    /// `OpenPipeWireRemote` reply (which only runs *after* consent is
    /// granted).
    pub stage: &'static str,
    /// The verbatim portal/D-Bus error, surfaced as-is.
    pub message: String,
}

impl NegotiateError {
    /// A failure resolving the capture FFI before the portal flow starts
    /// (item 60b). Every missing symbol is named in one message.
    pub fn pipewire(message: impl Into<String>) -> Self {
        Self {
            stage: "pipewire",
            message: message.into(),
        }
    }

    /// A failure in a consent-gated call: `CreateSession`, `SelectSources`
    /// or `Start` (each waits on the user's dialog).
    pub fn consent(message: impl Into<String>) -> Self {
        Self {
            stage: "consent pending",
            message: message.into(),
        }
    }

    /// A failure fetching the PipeWire remote fd from a session whose
    /// consent has **already** been granted.
    pub fn remote_fd(message: impl Into<String>) -> Self {
        Self {
            stage: "remote-fd",
            message: message.into(),
        }
    }
}

impl std::fmt::Display for NegotiateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// How long any one portal round-trip may take. `SelectSources` waits on a
/// human clicking a consent dialog, so this is generous; it only exists to
/// stop a wedged portal from hanging the tray forever.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(180);

/// `types` bitmask for `SelectSources`: monitor (1) or window (2). We ask
/// for both and let the portal offer the picker.
const SOURCE_TYPE_MONITOR: u32 = 1;
const SOURCE_TYPE_WINDOW: u32 = 2;
/// `cursor_mode`: hide the cursor (1).
const CURSOR_HIDDEN: u32 = 1;
/// `persist_mode`: persist the restore token indefinitely (2).
const PERSIST_INDEFINITELY: u32 = 2;

/// A completed negotiation: the PipeWire remote fd, the ScreenCast node
/// the portal selected (the stream must connect to THIS node — a targetless
/// connect errors "target not found"), and the (optionally refreshed)
/// restore token.
pub struct Negotiated {
    pub fd: OwnedFd,
    pub node_id: u32,
    pub restore_token: Option<String>,
}

/// Drive the full ScreenCast portal handshake and return the PipeWire
/// remote. Any failure is a [`NegotiateError`] naming the stage that
/// failed, so the tray can surface exactly where the handshake broke (a
/// missing portal, a refusal, a timeout, or the post-consent remote fd)
/// rather than the last state the machine was in.
pub async fn negotiate(
    restore_token: Option<String>,
    data_dir: &Path,
    log: &CaptureLog,
) -> Result<Negotiated, NegotiateError> {
    let conn = zbus::Connection::session().await.map_err(|e| {
        NegotiateError::consent(format!("ScreenCast portal unavailable (no session bus): {e}"))
    })?;
    let unique = conn
        .unique_name()
        .map(|n| n.as_str().to_owned())
        .ok_or_else(|| NegotiateError::consent("session bus has no unique name"))?;

    // 1. CreateSession — the handle_token names the *request* and the
    //    session_handle_token names the *session*.
    let handle = crate::portal::handle_token();
    let session_token = crate::portal::handle_token();
    let mut opts: HashMap<String, Value> = HashMap::new();
    opts.insert("handle_token".to_owned(), Value::from(handle.clone()));
    opts.insert(
        "session_handle_token".to_owned(),
        Value::from(session_token.clone()),
    );
    let results = portal_request(&conn, &unique, "CreateSession", &handle, &(opts,)).await?;
    let session = results.get("session_handle").ok_or_else(|| {
        NegotiateError::consent("ScreenCast CreateSession returned no session handle")
    })?;
    let session_path: String = <&str>::try_from(session)
        .map_err(|e| NegotiateError::consent(format!("ScreenCast session handle: {e}")))?
        .to_owned();
    let session = ObjectPath::try_from(session_path.as_str()).map_err(|e| {
        NegotiateError::consent(format!("ScreenCast session path {session_path:?}: {e}"))
    })?;

    // 2. SelectSources — monitors + windows, hidden cursor, persistent.
    // Item 55: constrain the picker and steer the choice. Window frames are
    // the pure game surface (no overlapping windows/notifications, and the
    // 2560x1440 geometry the OCR layouts were tuned on).
    log.record(
        "consent pending",
        "requesting WINDOW | MONITOR sources with the cursor hidden; in the portal picker \
         choose the Warframe WINDOW (not a whole monitor)",
    );
    let handle = crate::portal::handle_token();
    let mut opts: HashMap<String, Value> = HashMap::new();
    opts.insert("handle_token".to_owned(), Value::from(handle.clone()));
    opts.insert(
        "types".to_owned(),
        Value::from(SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW),
    );
    opts.insert("multiple".to_owned(), Value::from(false));
    opts.insert("cursor_mode".to_owned(), Value::from(CURSOR_HIDDEN));
    opts.insert(
        "persist_mode".to_owned(),
        Value::from(PERSIST_INDEFINITELY),
    );
    if let Some(tok) = restore_token.as_ref() {
        // Item 60c: replay the persisted token with persist mode so the
        // portal can skip the full consent dialog. Never log the token
        // itself — only its tail.
        log.record(
            "consent pending",
            &format!(
                "replaying saved restore_token {} with persist_mode {PERSIST_INDEFINITELY}",
                token_hint(tok)
            ),
        );
        opts.insert("restore_token".to_owned(), Value::from(tok.clone()));
    }
    portal_request(
        &conn,
        &unique,
        "SelectSources",
        &handle,
        &(session.clone(), opts),
    )
    .await?;

    // 3. Start — the user picks a screen/window; `streams` describes them.
    let handle = crate::portal::handle_token();
    let mut opts: HashMap<String, Value> = HashMap::new();
    opts.insert("handle_token".to_owned(), Value::from(handle.clone()));
    let results = portal_request(
        &conn,
        &unique,
        "Start",
        &handle,
        &(session.clone(), String::new(), opts),
    )
    .await?;
    // Item 54: log the full Start reply. The raw `streams` payload is the
    // evidence for "what the portal actually selected" — a node id, its
    // properties (position/size/source_type), or an empty/targetless
    // stream. It is recorded before parsing so even a parse failure leaves
    // the payload in the log. Item 60c: the reply also carries the
    // restore_token, so the logger redacts that one value and prints only
    // its tail.
    let raw_reply = redacted_results(&results);
    log.record(
        "consent pending",
        &format!("ScreenCast Start raw reply: {{{raw_reply}}}"),
    );
    // Item 60c: the token in the Start reply is the one to keep. Persist
    // it HERE, before parsing streams or fetching the remote fd, so a
    // later failure cannot throw away consent the user just granted.
    let issued_token = persist_start_token(&results, data_dir, log);
    let node_id = parse_streams(results.get("streams").and_then(|v| v.try_clone().ok()))?;
    log.record(
        "consent pending",
        &format!("ScreenCast Start selected node {node_id}"),
    );

    // 4. OpenPipeWireRemote — returns the fd directly (no Response).
    //
    // THE TRAP (eleventh pass §1): this reply's D-Bus signature is a bare
    // Unix file descriptor, `h`. Every other call in the handshake
    // (CreateSession / SelectSources / Start) replies with a `a{sv}`
    // results dict; this one is the odd one out. Handling it as a Variant
    // (`OwnedValue`, `Type` = `v`) makes zbus reject the reply with
    // "Signature mismatch: got 'h', expected 'v'" BEFORE the fd is ever
    // extracted. The reply must be typed as an fd end to end — see
    // OPEN_PIPEWIRE_REMOTE_REPLY_SIGNATURE, which pins it.
    let msg = conn
        .call_method(
            Some(PORTAL_DEST),
            PORTAL_PATH,
            Some(SCREENCAST_IFACE),
            "OpenPipeWireRemote",
            &(session.clone(), HashMap::<String, Value>::new()),
        )
        .await
        .map_err(|e| {
            NegotiateError::remote_fd(format!("ScreenCast OpenPipeWireRemote failed: {e}"))
        })?;
    // The reply signature is checked here (debug builds) and again by the
    // typed deserialize below, so the `h`-not-`v` trap can never slip by.
    debug_assert_eq!(
        msg.body().signature().to_string(),
        OPEN_PIPEWIRE_REMOTE_REPLY_SIGNATURE,
        "OpenPipeWireRemote must reply with a bare fd (`h`), not a variant"
    );
    // zbus negotiates Unix FD passing (SCM_RIGHTS) on the session bus and
    // resolves the out-of-band `h` index for us; `zvariant::OwnedFd`
    // duplicates the descriptor into owned storage.
    let reply_fd: zbus::zvariant::OwnedFd = msg
        .body()
        .deserialize()
        .map_err(|e| NegotiateError::remote_fd(format!("ScreenCast remote fd: {e}")))?;
    let fd: OwnedFd = reply_fd.into();

    Ok(Negotiated {
        fd,
        node_id,
        restore_token: issued_token,
    })
}

/// One portal request: subscribe to the predictable `Response` path
/// *before* the call, then call, then wait for the signal. Returns the
/// response results dict on success.
async fn portal_request<B>(
    conn: &zbus::Connection,
    unique: &str,
    method: &str,
    handle_token: &str,
    args: &B,
) -> Result<HashMap<String, OwnedValue>, NegotiateError>
where
    B: Serialize + DynamicType,
{
    let req_path = crate::portal::request_path(unique, handle_token);
    let proxy = zbus::Proxy::new(conn, PORTAL_DEST, req_path.as_str(), REQUEST_IFACE)
        .await
        .map_err(|e| NegotiateError::consent(format!("ScreenCast request proxy ({method}): {e}")))?;
    let mut responses = proxy
        .receive_signal("Response")
        .await
        .map_err(|e| {
            NegotiateError::consent(format!("ScreenCast subscribe Response ({method}): {e}"))
        })?;

    conn.call_method(
        Some(PORTAL_DEST),
        PORTAL_PATH,
        Some(SCREENCAST_IFACE),
        method,
        args,
    )
    .await
    .map_err(|e| NegotiateError::consent(format!("ScreenCast {method} call: {e}")))?;

    let signal = tokio::time::timeout(RESPONSE_TIMEOUT, responses.next())
        .await
        .map_err(|_| {
            NegotiateError::consent(format!(
                "ScreenCast {method}: no response within {}s",
                RESPONSE_TIMEOUT.as_secs()
            ))
        })?
        .ok_or_else(|| {
            NegotiateError::consent(format!("ScreenCast {method}: response stream ended early"))
        })?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = signal
        .body()
        .deserialize()
        .map_err(|e| {
            NegotiateError::consent(format!("ScreenCast {method}: malformed response: {e}"))
        })?;
    if code != RESPONSE_SUCCESS {
        return Err(NegotiateError::consent(format!(
            "ScreenCast {method} refused (response {code})"
        )));
    }
    Ok(results)
}

/// Extract the selected ScreenCast node id from the `Start` response's
/// `streams` (`a(ua{sv})`). A capture with zero streams is a failure
/// (nothing to read), not a silent empty run; a stream with no node id is
/// also a failure — `pw_stream_connect` must target the node the portal
/// selected or the remote errors "target not found" (item 49).
fn parse_streams(streams: Option<OwnedValue>) -> Result<u32, NegotiateError> {
    let streams = streams
        .ok_or_else(|| NegotiateError::consent("ScreenCast Start returned no streams"))?;
    let arr = zbus::zvariant::Array::<'static>::try_from(streams)
        .map_err(|e| NegotiateError::consent(format!("ScreenCast streams: {e}")))?;
    if arr.is_empty() {
        return Err(NegotiateError::consent(
            "ScreenCast Start selected no streams",
        ));
    }
    // Each element is `(node_id, properties)`; take the first stream's id.
    let first = arr
        .inner()
        .first()
        .ok_or_else(|| NegotiateError::consent("ScreenCast streams: empty tuple"))?;
    let structure = zbus::zvariant::Structure::try_from(first).map_err(|e| {
        NegotiateError::consent(format!("ScreenCast stream tuple: {e}"))
    })?;
    let node = structure
        .fields()
        .first()
        .ok_or_else(|| NegotiateError::consent("ScreenCast stream tuple has no node id"))?;
    let node_id = u32::try_from(node).map_err(|e| {
        NegotiateError::consent(format!("ScreenCast stream node id: {e}"))
    })?;
    if node_id == PW_ID_ANY {
        return Err(NegotiateError::consent(
            "ScreenCast Start returned a targetless stream",
        ));
    }
    Ok(node_id)
}

/// The last few characters of a restore token, safe to log.
///
/// Item 60c: the token is a credential — the portal uses it to skip the
/// consent dialog — so logs may only carry enough to correlate events
/// (the tail plus the length), never the value itself.
pub fn token_hint(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return "<empty>".to_owned();
    }
    let keep = chars.len().min(4);
    let tail: String = chars[chars.len() - keep..].iter().collect();
    format!("…{tail} ({} chars)", chars.len())
}

/// Read the restore token from the `Start` reply's results. Per item 60c
/// this is the reply that carries it when the user checked "remember for
/// future sessions"; `SelectSources` only consumes a replay.
fn restore_token_from_results(results: &HashMap<String, OwnedValue>) -> Option<String> {
    results
        .get("restore_token")
        .and_then(|v| <&str>::try_from(v).ok().map(str::to_owned))
        .filter(|t| !t.is_empty())
}

/// Persist the token carried by the `Start` reply (item 60c). Returns the
/// issued token so the caller can log the outcome and hand it back.
///
/// A write failure is not fatal — the capture can still proceed — but it
/// is logged: an unpersisted token means the next run asks for full
/// consent again.
fn persist_start_token(
    results: &HashMap<String, OwnedValue>,
    data_dir: &Path,
    log: &CaptureLog,
) -> Option<String> {
    let token = restore_token_from_results(results)?;
    log.record(
        "consent pending",
        &format!(
            "ScreenCast Start issued restore_token {}",
            token_hint(&token)
        ),
    );
    match save_restore_token(data_dir, &token) {
        Ok(()) => log.record(
            "consent pending",
            &format!(
                "persisted restore_token {} for future sessions",
                token_hint(&token)
            ),
        ),
        Err(e) => log.record(
            "consent pending",
            &format!("could not persist restore_token: {e}"),
        ),
    }
    Some(token)
}

/// Render a portal reply for the capture log with the `restore_token`
/// value redacted to its tail. Every other field stays verbatim: `streams`
/// is the evidence item 54 exists for.
fn redacted_results(results: &HashMap<String, OwnedValue>) -> String {
    results
        .iter()
        .map(|(k, v)| {
            if k == "restore_token" {
                let hint = <&str>::try_from(v)
                    .map(token_hint)
                    .unwrap_or_else(|_| "<redacted>".to_owned());
                format!("{k}={hint}")
            } else {
                format!("{k}={v:?}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Persist/read the ScreenCast restore token in app data.
pub fn restore_token_path(data_dir: &Path) -> PathBuf {
    data_dir.join("screencast-token")
}

pub fn load_restore_token(data_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(restore_token_path(data_dir)).ok()?;
    let tok = text.trim().to_owned();
    (!tok.is_empty()).then_some(tok)
}

pub fn save_restore_token(data_dir: &Path, token: &str) -> Result<(), String> {
    let path = restore_token_path(data_dir);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("token dir: {e}"))?;
        }
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, token).map_err(|e| format!("token write: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("token publish: {e}"))
}

// ---------------------------------------------------------------------------
// SPA pod construction (pure — no PipeWire needed to test the byte layout)
// ---------------------------------------------------------------------------

/// Minimal SPA pod builder for the one message we send: an
/// `EnumFormat` video object. PipeWire's `spa_pod_builder_*` helpers are
/// `static inline` in the C headers and therefore absent from the shared
/// library, so we lay the bytes out ourselves.
mod pod {
    // SPA pod types (`spa/include/spa/utils/type.h:34-53`; the enum is
    // zero-based from `SPA_TYPE_START = 0`, so `SPA_TYPE_None` is 1).
    /// `SPA_TYPE_None`: a valid pod with no body (`size = 0`). Used by
    /// PipeWire to say "no value"; it must never be read as a format.
    pub const SPA_TYPE_NONE: u32 = 1;
    pub const SPA_TYPE_ID: u32 = 3;
    pub const SPA_TYPE_RECTANGLE: u32 = 10;
    pub const SPA_TYPE_FRACTION: u32 = 11;
    pub const SPA_TYPE_STRUCT: u32 = 14;
    pub const SPA_TYPE_OBJECT: u32 = 15;
    pub const SPA_TYPE_CHOICE: u32 = 19;
    // Choice kinds (`spa/include/spa/pod/pod.h:132-137`: None=0,
    // Range=1, Step=2, Enum=3, Flags=4).
    pub const SPA_CHOICE_ENUM: u32 = 3;
    pub const SPA_CHOICE_RANGE: u32 = 1;

    /// The `Format` object type (`SPA_TYPE_OBJECT_Format`), the third
    /// object after `SPA_TYPE_OBJECT_START = 0x40000`
    /// (`spa/include/spa/utils/type.h:77-80`).
    pub const SPA_TYPE_OBJECT_FORMAT: u32 = 0x40003;
    /// `SPA_PARAM_EnumFormat` (`spa/include/spa/param/param.h:24-28`;
    /// `SPA_PARAM_Invalid` is 0).
    pub const SPA_PARAM_ENUM_FORMAT: u32 = 3;
    /// `SPA_PARAM_Format` (the negotiated one, delivered to `param_changed`;
    /// `spa/include/spa/param/param.h:29`).
    pub const SPA_PARAM_FORMAT: u32 = 4;

    /// Format property keys (`spa/include/spa/param/format.h:91-95`).
    pub const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
    pub const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
    /// `SPA_MEDIA_TYPE_video` and `SPA_MEDIA_SUBTYPE_raw`
    /// (`spa/include/spa/param/format.h:20-33`; both enums are zero-based).
    pub const SPA_MEDIA_TYPE_VIDEO: u32 = 2;
    pub const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;

    /// Video format keys and the packed 4-byte formats we accept
    /// (`spa/include/spa/param/format.h:124-129`).
    pub const SPA_FORMAT_VIDEO_FORMAT: u32 = 0x20001;
    pub const SPA_FORMAT_VIDEO_SIZE: u32 = 0x20003;
    pub const SPA_FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
    /// `enum spa_video_format` is zero-based from
    /// `SPA_VIDEO_FORMAT_UNKNOWN = 0`
    /// (`spa/include/spa/param/video/raw.h:36-50`): RGBx=7, BGRx=8,
    /// RGBA=11, BGRA=12. The previous 6/7/10/11 values were off by one
    /// (they named AYUV/RGBx/xBGR/RGBA upstream).
    pub const SPA_VIDEO_FORMAT_RGBX: u32 = 7;
    pub const SPA_VIDEO_FORMAT_BGRX: u32 = 8;
    pub const SPA_VIDEO_FORMAT_RGBA: u32 = 11;
    pub const SPA_VIDEO_FORMAT_BGRA: u32 = 12;

    fn header(out: &mut Vec<u8>, size: u32, ty: u32) {
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&ty.to_le_bytes());
    }

    fn pad8(out: &mut Vec<u8>) {
        while out.len() % 8 != 0 {
            out.push(0);
        }
    }

    /// A bare `Id` pod (`size = 4`).
    pub fn id(value: u32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 4, SPA_TYPE_ID);
        o.extend_from_slice(&value.to_le_bytes());
        o
    }

    /// A `Rectangle` pod (`size = 16`).
    pub fn rect(w: u32, h: u32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 16, SPA_TYPE_RECTANGLE);
        o.extend_from_slice(&w.to_le_bytes());
        o.extend_from_slice(&h.to_le_bytes());
        o
    }

    /// A `Fraction` pod (`size = 8`).
    pub fn fraction(num: u32, den: u32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 8, SPA_TYPE_FRACTION);
        o.extend_from_slice(&num.to_le_bytes());
        o.extend_from_slice(&den.to_le_bytes());
        o
    }

    /// A `Struct` pod wrapping already-built child pods.
    pub fn structure(children: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for c in children {
            body.extend_from_slice(c);
        }
        pad8(&mut body);
        let mut o = Vec::new();
        header(&mut o, body.len() as u32, SPA_TYPE_STRUCT);
        o.extend_from_slice(&body);
        o
    }

    /// A `Choice` pod: `[type, flags, child]` where `child` is a `Struct`
    /// of the candidate pods (default first for `Enum`; default/min/max
    /// for `Range`).
    pub fn choice(kind: u32, children: &[Vec<u8>]) -> Vec<u8> {
        let child = structure(children);
        let mut o = Vec::new();
        header(&mut o, 8 + child.len() as u32, SPA_TYPE_CHOICE);
        o.extend_from_slice(&kind.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&child);
        pad8(&mut o);
        o
    }

    /// A `spa_pod_prop`: `key`, `flags`, then the value pod inline.
    pub fn prop(key: u32, value: Vec<u8>) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&key.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&value);
        pad8(&mut o);
        o
    }

    /// An object pod (`SPA_TYPE_Object` + `body {type, id}` + props).
    pub fn object(body_type: u32, body_id: u32, props: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&body_type.to_le_bytes());
        body.extend_from_slice(&body_id.to_le_bytes());
        for p in props {
            body.extend_from_slice(p);
        }
        pad8(&mut body);
        let mut o = Vec::new();
        header(&mut o, body.len() as u32, SPA_TYPE_OBJECT);
        o.extend_from_slice(&body);
        o
    }

    /// The single `EnumFormat` param we offer: raw video, one of the
    /// packed 4-byte formats, any size in `[1x1 .. 8192x8192]`, 30 fps.
    pub fn enum_format_video() -> Vec<u8> {
        let formats = choice(
            SPA_CHOICE_ENUM,
            &[
                id(SPA_VIDEO_FORMAT_BGRA),
                id(SPA_VIDEO_FORMAT_BGRX),
                id(SPA_VIDEO_FORMAT_RGBA),
                id(SPA_VIDEO_FORMAT_RGBX),
            ],
        );
        let sizes = choice(
            SPA_CHOICE_RANGE,
            &[rect(1920, 1080), rect(1, 1), rect(8192, 8192)],
        );
        object(
            SPA_TYPE_OBJECT_FORMAT,
            SPA_PARAM_ENUM_FORMAT,
            &[
                prop(SPA_FORMAT_MEDIA_TYPE, id(SPA_MEDIA_TYPE_VIDEO)),
                prop(SPA_FORMAT_MEDIA_SUBTYPE, id(SPA_MEDIA_SUBTYPE_RAW)),
                prop(SPA_FORMAT_VIDEO_FORMAT, formats),
                prop(SPA_FORMAT_VIDEO_SIZE, sizes),
                prop(SPA_FORMAT_VIDEO_FRAMERATE, fraction(30, 1)),
            ],
        )
    }

    /// Read a pod header: `(size, type)`.
    pub fn read_header(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.len() < 8 {
            return None;
        }
        let size = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let ty = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        Some((size, ty))
    }

    /// Total byte length of a serialized pod from its 8-byte header.
    ///
    /// `spa_pod.size` is the body length, so the object occupies
    /// `size + 8` bytes. `SPA_TYPE_None` is a header-only pod (no body):
    /// there is nothing to read and `None` is returned. A body size that
    /// cannot be represented is rejected as malformed.
    pub fn pod_total_len(header: &[u8]) -> Option<usize> {
        let (size, ty) = read_header(header)?;
        if ty == SPA_TYPE_NONE {
            return None;
        }
        (size as usize).checked_add(8)
    }

    /// Find `SPA_FORMAT_VIDEO_format` in a negotiated `Format` object pod
    /// (delivered to `param_changed`). Best-effort: `None` on anything we
    /// do not recognise, so a malformed pod degrades to the BGRA default
    /// rather than a panic.
    pub fn parse_video_format(bytes: &[u8]) -> Option<u32> {
        let (_size, ty) = read_header(bytes)?;
        if ty != SPA_TYPE_OBJECT {
            return None;
        }
        // Skip the object body (type + id).
        let mut off = 16usize;
        while off + 16 <= bytes.len() {
            let key = u32::from_le_bytes(bytes[off..off + 4].try_into().ok()?);
            let value_off = off + 8;
            let (vsize, vtype) = read_header(&bytes[value_off..])?;
            let value_len = 8usize.checked_add(vsize as usize)?;
            if key == SPA_FORMAT_VIDEO_FORMAT && vtype == SPA_TYPE_ID {
                // An Id body is 4 bytes; reject a truncated pod instead
                // of reading past the slice the caller bounded.
                if value_len < 12 || value_off + 12 > bytes.len() {
                    return None;
                }
                return Some(u32::from_le_bytes(
                    bytes[value_off + 8..value_off + 12].try_into().ok()?,
                ));
            }
            off = value_off + (value_len + 7) / 8 * 8;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// PipeWire FFI (loaded at runtime; the box has no libpipewire)
// ---------------------------------------------------------------------------

/// Opaque handle storage for a `struct spa_hook` (we never touch its
/// fields, only pass a stable pointer to `pw_stream_add_listener`).
#[repr(C, align(8))]
struct HookStorage([u8; 128]);

impl HookStorage {
    fn zeroed() -> Self {
        Self([0u8; 128])
    }
}

/// `struct pw_stream_events` (pipewire/stream.h), version 2. A null
/// function pointer is a valid "not implemented" callback.
#[repr(C)]
struct StreamEvents {
    version: u32,
    destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    state_changed: Option<unsafe extern "C" fn(*mut c_void, i32, i32, *const c_char)>,
    control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const c_void)>,
    add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    process: Option<unsafe extern "C" fn(*mut c_void)>,
    drained: Option<unsafe extern "C" fn(*mut c_void)>,
    command: Option<unsafe extern "C" fn(*mut c_void, *const c_void)>,
}

/// `struct pw_buffer` prefix.
#[repr(C)]
struct PwBuffer {
    buffer: *mut SpaBuffer,
}

/// `struct spa_buffer` prefix.
#[repr(C)]
struct SpaBuffer {
    n_metas: u32,
    n_datas: u32,
    metas: *mut c_void,
    datas: *mut SpaData,
}

/// `struct spa_data`.
#[repr(C)]
struct SpaData {
    ty: u32,
    flags: u32,
    fd: i32,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

/// `struct spa_chunk`.
#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

/// Every `libpipewire` symbol the consumer resolves with `dlsym`, in the
/// order they are used. This is the **single source of truth**: the loader
/// below resolves exactly these names, and a test pins the list against
/// the checked-in manifest `data/capture/pipewire-symbols.txt` (item 47)
/// so a rename — or an invented name — fails the build, not Seth's
/// evening on a real desktop.
///
/// **Item 47 root cause.** The previous list carried
/// `pw_thread_loop_get_context`, which is **not** part of PipeWire's
/// public API: `pipewire/thread-loop.h` exports
/// `pw_thread_loop_get_loop()` (returning a `struct pw_loop *`), and a
/// `struct pw_context *` is created from that loop with
/// `pw_context_new()` (`pipewire/context.h`). `dlopen`/`dlsym` resolves
/// names at runtime, so the wrong name compiled clean and only failed on
/// a real desktop — exactly Seth's `v0.4.0` report. Every name below was
/// checked against the current 1.x public headers:
/// `pipewire.h`, `thread-loop.h`, `context.h`, `core.h`, `stream.h`.
///
/// **Item 60a.** `pw_core_get_registry` is declared by `core.h` but is
/// *not* exported by Seth's installed `libpipewire`, so v0.5.2 died while
/// resolving it. This consumer therefore has no registry dependency at
/// all: it connects the stream straight to the portal's node and retries
/// `pw_stream_connect` on "target not found" while the private remote is
/// still publishing that node. `pw_core_get_registry` and
/// `pw_registry_add_listener` are deliberately absent, and the registry
/// structs/callbacks are gone with them.
pub const REQUIRED_SYMBOLS: &[&str] = &[
    "pw_init",                  // pipewire.h
    "pw_thread_loop_new",       // thread-loop.h
    "pw_thread_loop_start",     // thread-loop.h
    "pw_thread_loop_lock",      // thread-loop.h
    "pw_thread_loop_unlock",    // thread-loop.h
    "pw_thread_loop_get_loop",  // thread-loop.h  (NOT pw_thread_loop_get_context)
    "pw_context_new",           // context.h
    "pw_context_destroy",       // context.h
    "pw_context_connect_fd",    // core.h
    "pw_stream_new",            // stream.h
    "pw_stream_add_listener",   // stream.h
    "pw_stream_connect",        // stream.h
    "pw_stream_disconnect",     // stream.h
    "pw_stream_destroy",        // stream.h
    "pw_stream_dequeue_buffer", // stream.h
    "pw_stream_queue_buffer",   // stream.h
    "pw_thread_loop_stop",      // thread-loop.h
    "pw_thread_loop_destroy",   // thread-loop.h
];

/// The resolved `libpipewire` entry points. Kept in a leaked `OnceLock`
/// so every stream shares one `pw_init`.
struct PwApi {
    _lib: *mut c_void,
    pw_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    pw_thread_loop_new: unsafe extern "C" fn(*const c_char, *const c_void) -> *mut c_void,
    pw_thread_loop_start: unsafe extern "C" fn(*mut c_void) -> c_int,
    pw_thread_loop_lock: unsafe extern "C" fn(*mut c_void),
    pw_thread_loop_unlock: unsafe extern "C" fn(*mut c_void),
    /// `thread-loop.h`: the `struct pw_loop *` wrapped by the thread loop.
    pw_thread_loop_get_loop: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    /// `context.h`: create the context the stream is built on.
    pw_context_new: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void,
    pw_context_destroy: unsafe extern "C" fn(*mut c_void),
    pw_context_connect_fd:
        unsafe extern "C" fn(*mut c_void, c_int, *mut c_void, usize) -> *mut c_void,
    pw_stream_new: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> *mut c_void,
    pw_stream_add_listener:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *mut c_void),
    pw_stream_connect:
        unsafe extern "C" fn(*mut c_void, c_int, u32, u32, *const *const c_void, u32) -> c_int,
    pw_stream_disconnect: unsafe extern "C" fn(*mut c_void) -> c_int,
    pw_stream_destroy: unsafe extern "C" fn(*mut c_void),
    pw_stream_dequeue_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pw_stream_queue_buffer: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int,
    pw_thread_loop_stop: unsafe extern "C" fn(*mut c_void),
    pw_thread_loop_destroy: unsafe extern "C" fn(*mut c_void),
}

// The library handle and function pointers are process-global and
// thread-safe (PipeWire's own API is).
unsafe impl Send for PwApi {}
unsafe impl Sync for PwApi {}

impl PwApi {
    fn load() -> Result<&'static PwApi, String> {
        use std::sync::OnceLock;
        static CELL: OnceLock<Result<PwApi, String>> = OnceLock::new();
        static API: OnceLock<&'static PwApi> = OnceLock::new();
        if let Some(api) = API.get() {
            return Ok(api);
        }
        let loaded = CELL.get_or_init(Self::dlopen);
        match loaded {
            Ok(a) => {
                let leaked: &'static PwApi = unsafe { std::mem::transmute::<&PwApi, &'static PwApi>(a) };
                Ok(API.get_or_init(|| leaked))
            }
            Err(e) => Err(e.clone()),
        }
    }

    fn dlopen() -> Result<PwApi, String> {
        // The freedesktop runtime ships the soname; a dev box may have the
        // unversioned name. Try both.
        let mut handle: *mut c_void = std::ptr::null_mut();
        for name in ["libpipewire-0.3.so.0", "libpipewire-0.3.so", "libpipewire-0.3.so.0.3.0"] {
            let Ok(cname) = CString::new(name) else { continue };
            handle = unsafe { libc::dlopen(cname.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
            if !handle.is_null() {
                break;
            }
        }
        if handle.is_null() {
            return Err(
                "libpipewire-0.3.so.0 not found (ScreenCast needs the freedesktop runtime)"
                    .to_owned(),
            );
        }
        // Resolve EXACTLY the manifest symbols, once, keyed by name. Item
        // 60b: collect EVERY missing name and report them in one error, so
        // a live run can never discover a second missing symbol after the
        // first one is fixed. The manifest test keeps this list and the
        // field wiring below in lockstep.
        let mut resolved: HashMap<&'static str, *mut c_void> = HashMap::new();
        let mut missing: Vec<&'static str> = Vec::new();
        for name in REQUIRED_SYMBOLS {
            let cname = CString::new(*name).unwrap();
            let p = unsafe { libc::dlsym(handle, cname.as_ptr()) };
            if p.is_null() {
                missing.push(name);
            } else {
                resolved.insert(name, p);
            }
        }
        if !missing.is_empty() {
            return Err(missing_symbols_error(&missing));
        }
        let get = |name: &'static str| -> *mut c_void {
            *resolved
                .get(name)
                .unwrap_or_else(|| panic!("dlsym list does not carry {name}"))
        };
        macro_rules! sym {
            ($name:literal, $ty:ty) => {{
                unsafe { std::mem::transmute::<*mut c_void, $ty>(get($name)) }
            }};
        }
        let api = PwApi {
            _lib: handle,
            pw_init: sym!("pw_init", unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char)),
            pw_thread_loop_new: sym!(
                "pw_thread_loop_new",
                unsafe extern "C" fn(*const c_char, *const c_void) -> *mut c_void
            ),
            pw_thread_loop_start: sym!(
                "pw_thread_loop_start",
                unsafe extern "C" fn(*mut c_void) -> c_int
            ),
            pw_thread_loop_lock: sym!("pw_thread_loop_lock", unsafe extern "C" fn(*mut c_void)),
            pw_thread_loop_unlock: sym!("pw_thread_loop_unlock", unsafe extern "C" fn(*mut c_void)),
            pw_thread_loop_get_loop: sym!(
                "pw_thread_loop_get_loop",
                unsafe extern "C" fn(*mut c_void) -> *mut c_void
            ),
            pw_context_new: sym!(
                "pw_context_new",
                unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> *mut c_void
            ),
            pw_context_destroy: sym!("pw_context_destroy", unsafe extern "C" fn(*mut c_void)),
            pw_context_connect_fd: sym!(
                "pw_context_connect_fd",
                unsafe extern "C" fn(*mut c_void, c_int, *mut c_void, usize) -> *mut c_void
            ),
            pw_stream_new: sym!(
                "pw_stream_new",
                unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> *mut c_void
            ),
            pw_stream_add_listener: sym!(
                "pw_stream_add_listener",
                unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *mut c_void)
            ),
            pw_stream_connect: sym!(
                "pw_stream_connect",
                unsafe extern "C" fn(*mut c_void, c_int, u32, u32, *const *const c_void, u32) -> c_int
            ),
            pw_stream_disconnect: sym!(
                "pw_stream_disconnect",
                unsafe extern "C" fn(*mut c_void) -> c_int
            ),
            pw_stream_destroy: sym!("pw_stream_destroy", unsafe extern "C" fn(*mut c_void)),
            pw_stream_dequeue_buffer: sym!(
                "pw_stream_dequeue_buffer",
                unsafe extern "C" fn(*mut c_void) -> *mut c_void
            ),
            pw_stream_queue_buffer: sym!(
                "pw_stream_queue_buffer",
                unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int
            ),
            pw_thread_loop_stop: sym!("pw_thread_loop_stop", unsafe extern "C" fn(*mut c_void)),
            pw_thread_loop_destroy: sym!(
                "pw_thread_loop_destroy",
                unsafe extern "C" fn(*mut c_void)
            ),
        };
        Ok(api)
    }
}

/// Resolve every capture symbol before the portal flow starts (item 60b).
/// On failure the message names **all** missing names in one line; the
/// caller surfaces it as a `pipewire` stage error, so no consent dialog is
/// ever shown for a run that cannot possibly connect.
pub fn preflight_pipewire() -> Result<(), String> {
    PwApi::load().map(|_| ())
}

/// One error naming every missing `libpipewire` symbol (item 60b).
fn missing_symbols_error(missing: &[&str]) -> String {
    format!(
        "libpipewire missing {} required symbol(s): {}",
        missing.len(),
        missing.join(", ")
    )
}

/// `PW_DIRECTION_INPUT` — a stream that **consumes** frames (we are the
/// consumer of the portal's source node). `port.h` aliases
/// `PW_DIRECTION_INPUT` to `SPA_DIRECTION_INPUT` (`src/pipewire/port.h:43`),
/// whose value is **0** (`spa/include/spa/utils/defs.h:95`).
/// `SPA_DIRECTION_OUTPUT` is 1 (`defs.h:96`).
const PW_DIRECTION_INPUT: c_int = 0;
/// `PW_ID_ANY` (`src/pipewire/core.h:60`).
const PW_ID_ANY: u32 = 0xffff_ffff;
/// `PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS |
/// PW_STREAM_FLAG_DONT_RECONNECT` (`src/pipewire/stream.h:465,470,479`).
///
/// **AUTOCONNECT links the stream to the node named in the `Start`
/// reply's `streams`** (item 49). With the connect target supplied, the
/// session manager makes the link to the portal's source node; without
/// AUTOCONNECT the stream would be created and never linked.
///
/// **Pass-9 record correction (pass 20).** This block used to blame a
/// "private remote" whose node was not yet published, and the retry loop
/// below was added to rationalize "target not found". That theory is
/// wrong. `OpenPipeWireRemote` returns an ordinary remote and the portal's
/// source node is present. The failure was `PW_DIRECTION_INPUT` being
/// hand-defined as 1 — which is `SPA_DIRECTION_OUTPUT`
/// (`spa/include/spa/utils/defs.h:95-96`). We were connecting an OUTPUT
/// (producer) stream and asking the session manager to link that producer
/// *into* the portal's source node, which has no input ports. "target not
/// found" was deterministic, not a registration race.
///
/// The retry/settle code is retained only as a guard for a genuinely late
/// node; it was never the fix for the direction defect.
const PW_STREAM_FLAGS: u32 = (1 << 0) | (1 << 2) | (1 << 7);
/// `PW_STREAM_STATE_ERROR` (`src/pipewire/stream.h:247`).
const PW_STREAM_STATE_ERROR: i32 = -1;
/// How long a "target not found" connect may be retried (item 60a), as a
/// guard against a genuinely late node publication. The pass-20 direction
/// fix is what makes a healthy connect succeed; this bound keeps an absent
/// target a visible failure, not a hang.
const CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(5);
/// Pause between connect attempts while the target is not there yet.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(250);
/// How long to watch a connected stream for the asynchronous
/// "target not found" error before declaring the connect good. This is a
/// settle window, not the first-frame timeout: Seth's v0.5.0 error landed
/// in ~5 ms.
const CONNECT_SETTLE: Duration = Duration::from_millis(200);
/// How long to wait for the first frame before declaring the stream
/// unlinked. Consent is already granted by the time we connect, so a
/// healthy stream delivers within a beat; this only exists so a stream
/// that never links surfaces in the tray instead of hanging the capture
/// forever.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// Whether a PipeWire stream error is "target not found": a node that is
/// genuinely absent, or genuinely not published yet.
fn is_target_not_found(message: &str) -> bool {
    message.to_ascii_lowercase().contains("target not found")
}

/// State shared with the PipeWire callbacks. The pointers into `api` and
/// `stream` are only ever touched from the PipeWire thread; the channel
/// and the atomics are the cross-thread surface.
struct StreamCtx {
    api: &'static PwApi,
    stream: *mut c_void,
    tx: Sender<GrayImage>,
    format: AtomicU32,
    err: Arc<Mutex<Option<String>>>,
    /// Verbatim PipeWire stream-state + error log (ninth pass §1).
    log: CaptureLog,
}

/// A live PipeWire screencast stream yielding grayscale frames. Dropping
/// it tears the stream down.
pub struct ScreenCastSource {
    api: &'static PwApi,
    thread_loop: *mut c_void,
    context: *mut c_void,
    core: *mut c_void,
    stream: *mut c_void,
    hook: Box<HookStorage>,
    events: Box<StreamEvents>,
    ctx: Box<StreamCtx>,
    rx: Receiver<GrayImage>,
    stop: Arc<AtomicBool>,
    err: Arc<Mutex<Option<String>>>,
    connected: bool,
    /// Deadline for the first frame; a stream that never links fails here
    /// instead of stalling the capture forever.
    first_frame_deadline: Instant,
    got_first: bool,
    log: CaptureLog,
}

// The source is created and consumed on one thread; PipeWire stream
// objects are not themselves `Sync`. Sending the whole source to the
// capture thread is fine because nothing else touches the raw pointers.
unsafe impl Send for ScreenCastSource {}

impl ScreenCastSource {
    /// Build and connect a stream on the negotiated remote fd. Call from
    /// a non-async context (it starts a PipeWire thread loop). `log`
    /// receives every PipeWire state transition + error verbatim.
    pub fn new(
        negotiated: Negotiated,
        stop: Arc<AtomicBool>,
        log: CaptureLog,
    ) -> Result<Self, String> {
        let api = PwApi::load()?;
        unsafe {
            // pw_init is idempotent-ish; passing null keeps argv untouched.
            (api.pw_init)(std::ptr::null_mut(), std::ptr::null_mut());
        }

        let name = CString::new("fracker-capture").map_err(|e| e.to_string())?;
        let thread_loop = unsafe { (api.pw_thread_loop_new)(name.as_ptr(), std::ptr::null()) };
        if thread_loop.is_null() {
            return Err("PipeWire: pw_thread_loop_new failed".to_owned());
        }
        if unsafe { (api.pw_thread_loop_start)(thread_loop) } < 0 {
            unsafe { (api.pw_thread_loop_destroy)(thread_loop) };
            return Err("PipeWire: pw_thread_loop_start failed".to_owned());
        }

        let (tx, rx) = channel::<GrayImage>();
        let err = Arc::new(Mutex::new(None));
        let node_id = negotiated.node_id;
        let ctx = Box::new(StreamCtx {
            api,
            stream: std::ptr::null_mut(),
            tx,
            format: AtomicU32::new(pod::SPA_VIDEO_FORMAT_BGRA),
            err: err.clone(),
            log: log.clone(),
        });
        let mut ctx = ctx;
        let events = Box::new(StreamEvents {
            version: 2,
            destroy: None,
            state_changed: Some(on_state_changed),
            control_info: None,
            io_changed: None,
            param_changed: Some(on_param_changed),
            add_buffer: None,
            remove_buffer: None,
            process: Some(on_process),
            drained: None,
            command: None,
        });
        // Setup under the loop lock (the stream must be created on the
        // loop's context thread), then release it while waiting.
        unsafe { (api.pw_thread_loop_lock)(thread_loop) };
        let setup = || -> Result<(*mut c_void, *mut c_void), String> {
            // Real PipeWire sequence (item 47): the thread loop wraps a
            // `pw_loop`; the context is built from that loop, then the
            // portal fd connects a core, and the stream rides that core.
            // There is no `pw_thread_loop_get_context` in the public API.
            let loop_ = unsafe { (api.pw_thread_loop_get_loop)(thread_loop) };
            if loop_.is_null() {
                return Err("PipeWire: pw_thread_loop_get_loop returned no loop".to_owned());
            }
            let context = unsafe { (api.pw_context_new)(loop_, std::ptr::null_mut(), 0) };
            if context.is_null() {
                return Err("PipeWire: pw_context_new failed".to_owned());
            }
            let fd = negotiated.fd.as_raw_fd();
            let core = unsafe { (api.pw_context_connect_fd)(context, fd, std::ptr::null_mut(), 0) };
            if core.is_null() {
                unsafe { (api.pw_context_destroy)(context) };
                return Err("PipeWire: pw_context_connect_fd failed".to_owned());
            }
            Ok((context, core))
        };
        let result = setup();
        unsafe { (api.pw_thread_loop_unlock)(thread_loop) };

        let (context, core) = match result {
            Ok(parts) => parts,
            Err(e) => {
                log.record("connecting", &e);
                unsafe {
                    (api.pw_thread_loop_stop)(thread_loop);
                    (api.pw_thread_loop_destroy)(thread_loop);
                }
                return Err(e);
            }
        };

        // Tear the stream down under the loop lock; the context/loop stay
        // alive across retries.
        let destroy_stream = |stream: *mut c_void| unsafe {
            if stream.is_null() {
                return;
            }
            (api.pw_thread_loop_lock)(thread_loop);
            (api.pw_stream_destroy)(stream);
            (api.pw_thread_loop_unlock)(thread_loop);
        };
        let teardown = |context: *mut c_void| unsafe {
            (api.pw_thread_loop_lock)(thread_loop);
            if !context.is_null() {
                (api.pw_context_destroy)(context);
            }
            (api.pw_thread_loop_unlock)(thread_loop);
            (api.pw_thread_loop_stop)(thread_loop);
            (api.pw_thread_loop_destroy)(thread_loop);
        };

        // Item 60a: no registry. The remote publishes the portal's node
        // asynchronously, so the first connect can race it and the stream
        // reports "target not found" (state 0→1→2→-1 in ~5ms). Retry the
        // connect itself for a few seconds instead of resolving the
        // registry symbol Seth's library does not export.
        let deadline = Instant::now() + CONNECT_RETRY_WINDOW;
        let mut attempt = 0u32;
        let (stream, hook) = loop {
            attempt += 1;
            unsafe { (api.pw_thread_loop_lock)(thread_loop) };
            let created = unsafe { (api.pw_stream_new)(core, name.as_ptr(), std::ptr::null_mut()) };
            if created.is_null() {
                unsafe { (api.pw_thread_loop_unlock)(thread_loop) };
                teardown(context);
                let msg = "PipeWire: pw_stream_new failed".to_owned();
                log.record("connecting", &msg);
                return Err(msg);
            }
            ctx.stream = created;
            let mut attempt_hook = Box::new(HookStorage::zeroed());
            unsafe {
                (api.pw_stream_add_listener)(
                    created,
                    attempt_hook.as_mut() as *mut _ as *mut c_void,
                    events.as_ref() as *const _ as *const c_void,
                    ctx.as_mut() as *mut _ as *mut c_void,
                );
            }
            // A previous attempt's error must not taint this one.
            if let Ok(mut e) = err.lock() {
                *e = None;
            }
            // Keep the format pod 8-byte aligned while PipeWire reads it.
            let format = pod::enum_format_video();
            let words = to_aligned_words(&format);
            let params: [*const c_void; 1] = [words.as_ptr() as *const c_void];
            let rc = unsafe {
                (api.pw_stream_connect)(
                    created,
                    PW_DIRECTION_INPUT,
                    node_id,
                    PW_STREAM_FLAGS,
                    params.as_ptr(),
                    1,
                )
            };
            unsafe { (api.pw_thread_loop_unlock)(thread_loop) };

            if rc < 0 {
                destroy_stream(created);
                let msg = format!("PipeWire: pw_stream_connect failed ({rc})");
                // `-ENOENT` is the synchronous spelling of the same
                // target-not-found race; anything else is not retryable.
                if rc == -libc::ENOENT && Instant::now() < deadline {
                    log.record(
                        "connecting",
                        &format!(
                            "pw_stream_connect attempt {attempt} for node {node_id} failed \
                             ({rc}); target not found yet, retrying"
                        ),
                    );
                    std::thread::sleep(CONNECT_RETRY_INTERVAL);
                    continue;
                }
                teardown(context);
                log.record("connecting", &msg);
                return Err(msg);
            }

            // The connect call returning 0 only means the request was
            // queued; target-not-found arrives asynchronously as a stream
            // state error. Watch a short settle window before accepting
            // the connection so the retry can still fire.
            if let Some(msg) = wait_for_connect_error(&err, CONNECT_SETTLE) {
                destroy_stream(created);
                if is_target_not_found(&msg) && Instant::now() < deadline {
                    log.record(
                        "connecting",
                        &format!(
                            "pw_stream_connect attempt {attempt} for node {node_id}: {msg}; \
                             retrying while the target appears"
                        ),
                    );
                    std::thread::sleep(CONNECT_RETRY_INTERVAL);
                    continue;
                }
                teardown(context);
                log.record("connecting", &msg);
                return Err(msg);
            }

            log.record(
                "connecting",
                &format!(
                    "PipeWire stream connected to node {node_id} on attempt {attempt}; \
                     awaiting first frame"
                ),
            );
            break (created, attempt_hook);
        };
        // The negotiated fd is now owned by PipeWire; leak the OwnedFd so
        // it is not double-closed (pw_context_connect_fd dup'd it).
        std::mem::forget(negotiated.fd);

        Ok(Self {
            api,
            thread_loop,
            context,
            core,
            stream,
            hook,
            events,
            ctx,
            rx,
            stop,
            err,
            connected: true,
            first_frame_deadline: Instant::now() + FIRST_FRAME_TIMEOUT,
            got_first: false,
            log,
        })
    }
}

impl ScreenCastSource {
    /// Item 54: an error before the first frame is a "connecting" failure,
    /// not "capturing". v0.5.0 stamped `target not found` as capturing even
    /// though no frame (and no successful connect) had happened yet.
    fn error_stage(&self) -> &'static str {
        failure_stage(self.got_first)
    }
}

/// The lifecycle stage an error belongs to, from whether a frame ever
/// arrived. Shared with the resident worker so both loggers agree.
pub(crate) fn failure_stage(first_frame_seen: bool) -> &'static str {
    if first_frame_seen {
        "capturing"
    } else {
        "connecting"
    }
}

impl FrameSource for ScreenCastSource {
    fn next_frame(&mut self) -> Result<Option<GrayImage>, String> {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if let Ok(mut e) = self.err.lock() {
                if let Some(msg) = e.take() {
                    let stage = self.error_stage();
                    self.log.record(stage, &msg);
                    return Err(msg);
                }
            }
            match self.rx.recv_timeout(Duration::from_millis(250)) {
                Ok(img) => {
                    if !self.got_first {
                        self.got_first = true;
                        self.log.record(
                            "capturing",
                            "first frame received; stream is linked",
                        );
                    }
                    return Ok(Some(img));
                }
                Err(RecvTimeoutError::Timeout) => {
                    // A stream that never links (no AUTOCONNECT, wrong
                    // node, permission) produces no error state — only
                    // silence. Surface the stall instead of blocking.
                    if !self.got_first && Instant::now() >= self.first_frame_deadline {
                        let msg = format!(
                            "PipeWire: no first frame within {}s — stream never linked",
                            FIRST_FRAME_TIMEOUT.as_secs()
                        );
                        self.log.record(self.error_stage(), &msg);
                        return Err(msg);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if !self.got_first {
                        let msg =
                            "PipeWire: frame channel closed before the first frame".to_owned();
                        self.log.record(self.error_stage(), &msg);
                        return Err(msg);
                    }
                    return Ok(None);
                }
            }
        }
    }
}

impl Drop for ScreenCastSource {
    fn drop(&mut self) {
        unsafe {
            // Teardown uses the same thread-loop lock discipline as the
            // rest of this file: `disconnect`/`destroy` run on the loop's
            // context thread, so a callback cannot dispatch against the
            // objects while they are being freed. Do not hold the lock
            // across `pw_thread_loop_stop` (it waits for the loop, which
            // needs the lock).
            let locked = !self.thread_loop.is_null();
            if locked {
                (self.api.pw_thread_loop_lock)(self.thread_loop);
            }
            if self.connected && !self.stream.is_null() {
                (self.api.pw_stream_disconnect)(self.stream);
                (self.api.pw_stream_destroy)(self.stream);
                self.connected = false;
            }
            // The context owns the core/stream; destroy it before the
            // loop it was created on (context.h: the main loop must stay
            // alive until pw_context_destroy()).
            if !self.context.is_null() {
                (self.api.pw_context_destroy)(self.context);
                self.context = std::ptr::null_mut();
            }
            if locked {
                (self.api.pw_thread_loop_unlock)(self.thread_loop);
                (self.api.pw_thread_loop_stop)(self.thread_loop);
                (self.api.pw_thread_loop_destroy)(self.thread_loop);
            }
        }
        // Keep the boxes alive until after the teardown above.
        let _ = (&self.hook, &self.events, &self.ctx, self.core);
    }
}

/// Watch a just-connected stream for an asynchronous "target not found"
/// error during the settle window (item 60a). Returns the error if one
/// arrived, leaving a healthy stream's slot empty.
fn wait_for_connect_error(err: &Mutex<Option<String>>, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(mut e) = err.lock() {
            if let Some(msg) = e.take() {
                return Some(msg);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

unsafe extern "C" fn on_state_changed(
    data: *mut c_void,
    old: i32,
    state: i32,
    error: *const c_char,
) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    let detail = if error.is_null() {
        String::new()
    } else {
        let s = std::ffi::CStr::from_ptr(error).to_string_lossy();
        format!(" ({s})")
    };
    // Every transition goes to the capture log verbatim (ninth pass §1).
    ctx.log.record(
        "connecting",
        &format!("PipeWire stream state {old} -> {state}{detail}"),
    );
    if state != PW_STREAM_STATE_ERROR {
        return;
    }
    let msg = if error.is_null() {
        "PipeWire stream entered the error state".to_owned()
    } else {
        let s = std::ffi::CStr::from_ptr(error).to_string_lossy();
        format!("PipeWire stream error: {s}")
    };
    if let Ok(mut e) = ctx.err.lock() {
        *e = Some(msg);
    }
}

unsafe extern "C" fn on_param_changed(data: *mut c_void, id: u32, param: *const c_void) {
    if data.is_null() || param.is_null() || id != pod::SPA_PARAM_FORMAT {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    // The pod's own `size` header bounds the read. Probe pods are often
    // ≥256B, but the SPA ABI lets a format pod be much smaller (and a
    // `SPA_TYPE_None` pod has no body at all); a fixed 256-byte slice
    // would read past the object in both cases.
    let header = std::slice::from_raw_parts(param as *const u8, 8);
    let Some(len) = pod::pod_total_len(header) else {
        return;
    };
    let bytes = std::slice::from_raw_parts(param as *const u8, len);
    if let Some(fmt) = pod::parse_video_format(bytes) {
        ctx.format.store(fmt, Ordering::Relaxed);
    }
}

unsafe extern "C" fn on_process(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    let api = ctx.api;
    let b = (api.pw_stream_dequeue_buffer)(ctx.stream);
    if b.is_null() {
        return;
    }
    if let Some(img) = convert_buffer(ctx, b) {
        // Unbounded: a slow consumer drops nothing but also never blocks
        // the PipeWire thread.
        let _ = ctx.tx.send(img);
    }
    (api.pw_stream_queue_buffer)(ctx.stream, b);
}

/// Turn one dequeued `pw_buffer` into a grayscale frame. Bails out (but
/// still queues the buffer) on anything unexpected, so a format we did
/// not anticipate degrades to dropped frames, never a panic in the
/// PipeWire thread.
unsafe fn convert_buffer(ctx: &mut StreamCtx, b: *mut c_void) -> Option<GrayImage> {
    let pw = b as *mut PwBuffer;
    let spa = (*pw).buffer;
    if spa.is_null() || (*spa).n_datas == 0 || (*spa).datas.is_null() {
        return None;
    }
    let d = (*spa).datas;
    let ptr = (*d).data;
    let chunk = (*d).chunk;
    if ptr.is_null() || chunk.is_null() {
        return None;
    }
    let offset = (*chunk).offset as usize;
    let size = (*chunk).size as usize;
    let stride = (*chunk).stride;
    if size == 0 || stride <= 0 {
        return None;
    }
    let stride = stride as usize;
    let width = stride / 4;
    let height = size / stride;
    if width == 0 || height == 0 || width * height > 16_000_000 {
        return None;
    }
    let src = (ptr as *const u8).add(offset);
    let format = ctx.format.load(Ordering::Relaxed);
    // Byte order of the three colour channels: BGRA/BGRx store B,G,R;
    // RGBA/RGBx store R,G,B. Alpha is ignored either way.
    let (r, g, bl) = match format {
        pod::SPA_VIDEO_FORMAT_BGRA | pod::SPA_VIDEO_FORMAT_BGRX => (2usize, 1usize, 0usize),
        _ => (0usize, 1usize, 2usize),
    };
    let mut pixels = vec![0u8; width * height];
    for y in 0..height {
        let row = src.add(y * stride);
        for x in 0..width {
            let px = row.add(x * 4);
            let rr = *px.add(r) as u32;
            let gg = *px.add(g) as u32;
            let bb = *px.add(bl) as u32;
            pixels[y * width + x] = ((77 * rr + 150 * gg + 29 * bb) >> 8) as u8;
        }
    }
    Some(GrayImage {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

/// Copy pod bytes into a `Vec<u64>` so the buffer PipeWire reads is
/// 8-byte aligned (a `Vec<u8>` makes no alignment promise).
fn to_aligned_words(bytes: &[u8]) -> Vec<u64> {
    let mut words = vec![0u64; (bytes.len() + 7) / 8];
    unsafe {
        std::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            words.as_mut_ptr() as *mut u8,
            bytes.len(),
        );
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every hand-defined numeric in the capture FFI surface, pinned to the
    /// value in the upstream header named beside it (pass 20 §2).
    ///
    /// The pass-9/19 audits checked symbol names and pod shapes; enum and
    /// flag *values* were checked by nothing, which is how the direction
    /// constant survived. This test exists so a value can never drift
    /// silently again — update it only with a header citation.
    #[test]
    fn hand_defined_ffi_values_match_upstream_headers() {
        // (name, our value, header value, header citation)
        let values: &[(&str, u32, u32, &str)] = &[
            (
                "PW_DIRECTION_INPUT",
                PW_DIRECTION_INPUT as u32,
                0,
                "SPA_DIRECTION_INPUT, spa/include/spa/utils/defs.h:95 (port.h:43)",
            ),
            (
                "PW_ID_ANY",
                PW_ID_ANY,
                0xffff_ffff,
                "src/pipewire/core.h:60",
            ),
            (
                "PW_STREAM_FLAGS",
                PW_STREAM_FLAGS,
                (1 << 0) | (1 << 2) | (1 << 7),
                "AUTOCONNECT|MAP_BUFFERS|DONT_RECONNECT, src/pipewire/stream.h:465,470,479",
            ),
            (
                "SPA_TYPE_NONE",
                pod::SPA_TYPE_NONE,
                1,
                "spa/include/spa/utils/type.h:34-35",
            ),
            (
                "SPA_TYPE_ID",
                pod::SPA_TYPE_ID,
                3,
                "spa/include/spa/utils/type.h:37",
            ),
            (
                "SPA_TYPE_RECTANGLE",
                pod::SPA_TYPE_RECTANGLE,
                10,
                "spa/include/spa/utils/type.h:44",
            ),
            (
                "SPA_TYPE_FRACTION",
                pod::SPA_TYPE_FRACTION,
                11,
                "spa/include/spa/utils/type.h:45",
            ),
            (
                "SPA_TYPE_STRUCT",
                pod::SPA_TYPE_STRUCT,
                14,
                "spa/include/spa/utils/type.h:48",
            ),
            (
                "SPA_TYPE_OBJECT",
                pod::SPA_TYPE_OBJECT,
                15,
                "spa/include/spa/utils/type.h:49",
            ),
            (
                "SPA_TYPE_CHOICE",
                pod::SPA_TYPE_CHOICE,
                19,
                "spa/include/spa/utils/type.h:53",
            ),
            (
                "SPA_TYPE_OBJECT_FORMAT",
                pod::SPA_TYPE_OBJECT_FORMAT,
                0x40003,
                "spa/include/spa/utils/type.h:77-80",
            ),
            (
                "SPA_CHOICE_RANGE",
                pod::SPA_CHOICE_RANGE,
                1,
                "spa/include/spa/pod/pod.h:132-137",
            ),
            (
                "SPA_CHOICE_ENUM",
                pod::SPA_CHOICE_ENUM,
                3,
                "spa/include/spa/pod/pod.h:132-137",
            ),
            (
                "SPA_PARAM_ENUM_FORMAT",
                pod::SPA_PARAM_ENUM_FORMAT,
                3,
                "spa/include/spa/param/param.h:24-28",
            ),
            (
                "SPA_PARAM_FORMAT",
                pod::SPA_PARAM_FORMAT,
                4,
                "spa/include/spa/param/param.h:29",
            ),
            (
                "SPA_FORMAT_MEDIA_TYPE",
                pod::SPA_FORMAT_MEDIA_TYPE,
                1,
                "spa/include/spa/param/format.h:91-94",
            ),
            (
                "SPA_FORMAT_MEDIA_SUBTYPE",
                pod::SPA_FORMAT_MEDIA_SUBTYPE,
                2,
                "spa/include/spa/param/format.h:92-95",
            ),
            (
                "SPA_MEDIA_TYPE_VIDEO",
                pod::SPA_MEDIA_TYPE_VIDEO,
                2,
                "spa/include/spa/param/format.h:20-23",
            ),
            (
                "SPA_MEDIA_SUBTYPE_RAW",
                pod::SPA_MEDIA_SUBTYPE_RAW,
                1,
                "spa/include/spa/param/format.h:31-33",
            ),
            (
                "SPA_FORMAT_VIDEO_FORMAT",
                pod::SPA_FORMAT_VIDEO_FORMAT,
                0x20001,
                "spa/include/spa/param/format.h:124-125",
            ),
            (
                "SPA_FORMAT_VIDEO_SIZE",
                pod::SPA_FORMAT_VIDEO_SIZE,
                0x20003,
                "spa/include/spa/param/format.h:124,128",
            ),
            (
                "SPA_FORMAT_VIDEO_FRAMERATE",
                pod::SPA_FORMAT_VIDEO_FRAMERATE,
                0x20004,
                "spa/include/spa/param/format.h:124,129",
            ),
            (
                "SPA_VIDEO_FORMAT_RGBX",
                pod::SPA_VIDEO_FORMAT_RGBX,
                7,
                "SPA_VIDEO_FORMAT_RGBx, spa/include/spa/param/video/raw.h:36-45",
            ),
            (
                "SPA_VIDEO_FORMAT_BGRX",
                pod::SPA_VIDEO_FORMAT_BGRX,
                8,
                "SPA_VIDEO_FORMAT_BGRx, spa/include/spa/param/video/raw.h:36-46",
            ),
            (
                "SPA_VIDEO_FORMAT_RGBA",
                pod::SPA_VIDEO_FORMAT_RGBA,
                11,
                "spa/include/spa/param/video/raw.h:36-49",
            ),
            (
                "SPA_VIDEO_FORMAT_BGRA",
                pod::SPA_VIDEO_FORMAT_BGRA,
                12,
                "spa/include/spa/param/video/raw.h:36-50",
            ),
        ];
        for &(name, ours, header, citation) in values {
            assert_eq!(ours, header, "{name} must match {citation}");
        }
        assert_eq!(
            PW_STREAM_STATE_ERROR, -1,
            "PW_STREAM_STATE_ERROR must match src/pipewire/stream.h:247"
        );

        // Portal (D-Bus) values — hand-defined here too, and part of the
        // same capture surface.
        assert_eq!(
            RESPONSE_SUCCESS, 0,
            "Request::Response 0 = success (org.freedesktop.portal.Request.xml:68)"
        );
        assert_eq!(
            SOURCE_TYPE_MONITOR, 1,
            "AvailableSourceTypes 1 = MONITOR (ScreenCast.xml:337)"
        );
        assert_eq!(
            SOURCE_TYPE_WINDOW, 2,
            "AvailableSourceTypes 2 = WINDOW (ScreenCast.xml:338)"
        );
        assert_eq!(
            CURSOR_HIDDEN, 1,
            "AvailableCursorModes 1 = Hidden (ScreenCast.xml:349)"
        );
        assert_eq!(
            PERSIST_INDEFINITELY, 2,
            "persist_mode 2 = persist until revoked (ScreenCast.xml:174)"
        );
    }

    #[test]
    fn pod_ids_and_rects_have_the_c_abi_layout() {
        assert_eq!(pod::read_header(&pod::id(11)), Some((4, pod::SPA_TYPE_ID)));
        assert_eq!(
            pod::read_header(&pod::rect(1920, 1080)),
            Some((16, pod::SPA_TYPE_RECTANGLE))
        );
        let r = pod::rect(2560, 1440);
        assert_eq!(u32::from_le_bytes(r[8..12].try_into().unwrap()), 2560);
        assert_eq!(u32::from_le_bytes(r[12..16].try_into().unwrap()), 1440);
    }

    #[test]
    fn struct_pod_size_covers_its_children_and_stays_aligned() {
        let s = pod::structure(&[pod::id(1), pod::id(2)]);
        let (size, ty) = pod::read_header(&s).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_STRUCT);
        assert_eq!(size as usize + 8, s.len());
        assert_eq!(s.len() % 8, 0, "pods are 8-byte aligned");
    }

    #[test]
    fn the_video_enum_format_object_is_well_formed() {
        let p = pod::enum_format_video();
        let (size, ty) = pod::read_header(&p).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_OBJECT);
        assert_eq!(size as usize + 8, p.len());
        assert_eq!(p.len() % 8, 0);
        // Object body type == SPA_TYPE_OBJECT_Format.
        assert_eq!(
            u32::from_le_bytes(p[8..12].try_into().unwrap()),
            pod::SPA_TYPE_OBJECT_FORMAT
        );
        // The object declares the EnumFormat param.
        assert_eq!(
            u32::from_le_bytes(p[12..16].try_into().unwrap()),
            pod::SPA_PARAM_ENUM_FORMAT
        );
    }

    #[test]
    fn parse_video_format_round_trips_the_format_prop() {
        // A `Format` object (as delivered to param_changed) with the
        // negotiated video format set to RGBA.
        let f = pod::object(
            pod::SPA_TYPE_OBJECT_FORMAT,
            pod::SPA_PARAM_FORMAT,
            &[
                pod::prop(pod::SPA_FORMAT_MEDIA_TYPE, pod::id(pod::SPA_MEDIA_TYPE_VIDEO)),
                pod::prop(pod::SPA_FORMAT_MEDIA_SUBTYPE, pod::id(pod::SPA_MEDIA_SUBTYPE_RAW)),
                pod::prop(pod::SPA_FORMAT_VIDEO_FORMAT, pod::id(pod::SPA_VIDEO_FORMAT_RGBA)),
                pod::prop(pod::SPA_FORMAT_VIDEO_SIZE, pod::rect(2560, 1440)),
            ],
        );
        assert_eq!(
            pod::parse_video_format(&f),
            Some(pod::SPA_VIDEO_FORMAT_RGBA)
        );
        // Garbage in -> None (never a panic).
        assert_eq!(pod::parse_video_format(&[0, 1, 2]), None);
    }

    #[test]
    fn a_small_pod_is_bounded_by_its_own_size_header() {
        // Sixteenth pass §1: the callback used to copy a fixed 256 bytes.
        // This Format pod is deliberately much smaller than that, so the
        // size-extraction logic must return its real length and the
        // parser must still read the format prop without overrunning.
        let small = pod::object(
            pod::SPA_TYPE_OBJECT_FORMAT,
            pod::SPA_PARAM_FORMAT,
            &[pod::prop(
                pod::SPA_FORMAT_VIDEO_FORMAT,
                pod::id(pod::SPA_VIDEO_FORMAT_RGBA),
            )],
        );
        assert!(small.len() < 256, "test pod must be smaller than the old fixed read");
        assert_eq!(pod::pod_total_len(&small[..8]), Some(small.len()));
        assert_eq!(
            pod::parse_video_format(&small),
            Some(pod::SPA_VIDEO_FORMAT_RGBA)
        );

        // `SPA_TYPE_None` is header-only: nothing to read, never parsed.
        let none = {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&pod::SPA_TYPE_NONE.to_le_bytes());
            bytes
        };
        assert_eq!(pod::pod_total_len(&none), None);
        assert_eq!(pod::parse_video_format(&none), None);

        // A truncated/garbage header is rejected rather than underflowed.
        assert_eq!(pod::pod_total_len(&[0, 1, 2]), None);
    }

    #[test]
    fn choice_pods_carry_their_kind_and_child_struct() {
        let c = pod::choice(pod::SPA_CHOICE_ENUM, &[pod::id(11), pod::id(10)]);
        let (size, ty) = pod::read_header(&c).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_CHOICE);
        assert_eq!(size as usize + 8, c.len());
        assert_eq!(
            u32::from_le_bytes(c[8..12].try_into().unwrap()),
            pod::SPA_CHOICE_ENUM
        );
        // The child struct starts right after the choice body.
        assert_eq!(
            pod::read_header(&c[16..]).unwrap().1,
            pod::SPA_TYPE_STRUCT
        );
    }

    #[test]
    fn restore_token_round_trips_through_app_data() {
        let dir = std::env::temp_dir().join(format!("fracker-sc-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load_restore_token(&dir), None);
        save_restore_token(&dir, "tok-abc\n").unwrap();
        assert_eq!(load_restore_token(&dir).as_deref(), Some("tok-abc"));
        // A blank file means "no token", not an empty-string token.
        std::fs::write(restore_token_path(&dir), "  \n").unwrap();
        assert_eq!(load_restore_token(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_token_hint_keeps_only_the_tail() {
        let token = "restore-token-0123456789abcdef";
        let hint = token_hint(token);
        assert!(!hint.contains(token), "the full token must not be logged: {hint}");
        assert!(hint.contains("cdef"), "the tail is the correlation clue: {hint}");
        assert!(hint.contains("30 chars"), "the length is useful evidence: {hint}");
        assert_eq!(token_hint(""), "<empty>");
    }

    #[test]
    fn start_reply_token_is_read_and_redacted_in_the_raw_log() {
        let mut results = HashMap::new();
        results.insert(
            "restore_token".to_owned(),
            OwnedValue::try_from(Value::from("restore-token-0123456789abcdef")).unwrap(),
        );
        results.insert(
            "streams".to_owned(),
            OwnedValue::try_from(Value::from(7u32)).unwrap(),
        );
        assert_eq!(
            restore_token_from_results(&results).as_deref(),
            Some("restore-token-0123456789abcdef")
        );
        let logged = redacted_results(&results);
        assert!(!logged.contains("restore-token-0123456789abcdef"), "{logged}");
        assert!(logged.contains("restore_token=…cdef"), "{logged}");
        // Other reply fields stay verbatim: streams is the evidence.
        assert!(logged.contains("streams="), "{logged}");
        // No token in the reply is simply no token.
        results.remove("restore_token");
        assert_eq!(restore_token_from_results(&results), None);
        assert!(!redacted_results(&results).contains("restore_token"));
    }

    #[test]
    fn start_reply_token_is_persisted_from_the_start_reply() {
        let dir = log_dir("start-token");
        let log = CaptureLog::open(&dir);
        let token = "restore-token-0123456789abcdef";
        let mut results = HashMap::new();
        results.insert(
            "restore_token".to_owned(),
            OwnedValue::try_from(Value::from(token)).unwrap(),
        );
        assert_eq!(
            persist_start_token(&results, &dir, &log).as_deref(),
            Some(token)
        );
        // The Start reply's token is now in the app-data store, ready to
        // be replayed by the next SelectSources.
        assert_eq!(load_restore_token(&dir).as_deref(), Some(token));
        // The capture log got the event but only the token tail.
        let text = std::fs::read_to_string(log.path()).unwrap();
        assert!(!text.contains(token), "{text}");
        assert!(text.contains("issued restore_token …cdef"), "{text}");
        assert!(text.contains("persisted restore_token …cdef"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn aligned_copy_preserves_the_first_bytes() {
        let bytes = pod::enum_format_video();
        let words = to_aligned_words(&bytes);
        assert_eq!(words.as_ptr() as usize % 8, 0, "aligned to 8");
        let view = unsafe { std::slice::from_raw_parts(words.as_ptr() as *const u8, bytes.len()) };
        assert_eq!(view, &bytes[..]);
    }

    #[test]
    fn parse_streams_rejects_empty_or_missing() {
        assert!(parse_streams(None).is_err());
        let sig = zbus::zvariant::Signature::try_from("(ua{sv})").unwrap();
        let arr = zbus::zvariant::Array::new(&sig);
        let empty = OwnedValue::try_from(Value::from(arr)).unwrap();
        assert!(parse_streams(Some(empty)).is_err());
    }

    /// Item 49: the node id in the `Start` reply's `streams` is the target
    /// `pw_stream_connect` must use. Build the real `a(ua{sv})` shape and
    /// prove we read the id (and reject a targetless `PW_ID_ANY` stream).
    #[test]
    fn parse_streams_reads_the_selected_node_id() {
        fn streams(nodes: &[u32]) -> OwnedValue {
            let sig = zbus::zvariant::Signature::try_from("(ua{sv})").unwrap();
            let mut arr = zbus::zvariant::Array::new(&sig);
            for &n in nodes {
                let props = std::collections::HashMap::<String, Value>::new();
                arr.append(Value::from((n, props))).unwrap();
            }
            OwnedValue::try_from(Value::from(arr)).unwrap()
        }
        assert_eq!(parse_streams(Some(streams(&[7]))).unwrap(), 7);
        // A real portal reply can carry several streams; the first is ours.
        assert_eq!(parse_streams(Some(streams(&[3, 9]))).unwrap(), 3);
        // A targetless stream is a failure, not a silent no-op connect.
        assert!(parse_streams(Some(streams(&[PW_ID_ANY]))).is_err());
    }

    /// Item 54: an error before the first frame must be logged/raised as
    /// `connecting`, never `capturing`.
    #[test]
    fn errors_before_the_first_frame_are_connecting() {
        assert_eq!(failure_stage(false), "connecting");
        assert_eq!(failure_stage(true), "capturing");
    }

    fn log_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fracker-screencast-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Item 60a: only the target-not-found race is retryable; a real
    /// stream error must fail immediately.
    #[test]
    fn only_target_not_found_errors_are_retryable() {
        assert!(is_target_not_found("PipeWire stream error: target not found"));
        assert!(is_target_not_found("TARGET NOT FOUND"));
        assert!(!is_target_not_found("permission denied"));
        assert!(!is_target_not_found(""));
    }

    /// The connect retry watches the same async error slot the state
    /// callback fills, so a "target not found" that lands after the
    /// connect call returns 0 is still observed.
    #[test]
    fn wait_for_connect_error_observes_the_async_state_error() {
        let err = Arc::new(Mutex::new(None));
        let setter = err.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            *setter.lock().unwrap() = Some("PipeWire stream error: target not found".to_owned());
        });
        assert_eq!(
            wait_for_connect_error(&err, Duration::from_secs(2)).as_deref(),
            Some("PipeWire stream error: target not found")
        );
        writer.join().unwrap();
        // The slot is consumed, so a healthy next attempt starts clean.
        assert_eq!(wait_for_connect_error(&err, Duration::from_millis(20)), None);
    }

    #[test]
    fn wait_for_connect_error_is_bounded_when_nothing_arrives() {
        let err = Mutex::new(None);
        let started = Instant::now();
        assert_eq!(
            wait_for_connect_error(&err, Duration::from_millis(150)),
            None
        );
        assert!(started.elapsed() < Duration::from_secs(2), "bounded");
    }

    /// Item 60b: one error names every missing symbol, not just the first.
    #[test]
    fn missing_symbols_error_names_every_missing_name() {
        let msg = missing_symbols_error(&["pw_core_get_registry", "pw_registry_add_listener"]);
        assert!(msg.contains("2 required symbol(s)"), "{msg}");
        assert!(msg.contains("pw_core_get_registry"), "{msg}");
        assert!(msg.contains("pw_registry_add_listener"), "{msg}");
    }

    #[test]
    fn open_pipewire_remote_reply_signature_is_a_bare_fd() {
        // The trap that produced
        // `ScreenCast remote fd: Signature mismatch: got 'h', expected 'v'`:
        // OpenPipeWireRemote replies with a bare Unix FD (`h`), not the `v`
        // variant that OwnedValue demands. Own the reply signature as a
        // constant and assert the type we deserialize into carries exactly
        // it, so a regression to a Variant path fails right here.
        assert_eq!(OPEN_PIPEWIRE_REMOTE_REPLY_SIGNATURE, "h");
        assert_eq!(
            <zbus::zvariant::OwnedFd as zbus::zvariant::Type>::SIGNATURE.to_string(),
            OPEN_PIPEWIRE_REMOTE_REPLY_SIGNATURE,
            "the OpenPipeWireRemote reply must be typed as an fd, not a variant"
        );
    }

    #[test]
    fn negotiate_failures_name_the_stage_that_failed() {
        // Consent-gated calls name consent; the fd call runs AFTER consent
        // is granted and must name remote-fd, so the tray can never render
        // "error (consent pending)" for a remote-fd failure (item 40a).
        assert_eq!(NegotiateError::consent("boom").stage, "consent pending");
        assert_eq!(
            NegotiateError::consent("boom").to_string(),
            "boom",
            "the verbatim message survives"
        );
        let fd = NegotiateError::remote_fd("ScreenCast remote fd: mismatched signature");
        assert_eq!(fd.stage, "remote-fd");
        assert_eq!(fd.message, "ScreenCast remote fd: mismatched signature");
        let sym = NegotiateError::pipewire("libpipewire missing 1 required symbol(s): x");
        assert_eq!(sym.stage, "pipewire");
    }

    /// Item 47: the checked-in manifest is the dlsym list, so a future
    /// rename (or an invented symbol) breaks the build instead of the
    /// live capture.
    #[test]
    fn dlsym_manifest_pins_every_required_symbol() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("data/capture/pipewire-symbols.txt");
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let listed: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert_eq!(
            listed, REQUIRED_SYMBOLS,
            "data/capture/pipewire-symbols.txt must list exactly the symbols the consumer dlsyms"
        );
    }

    /// The bug was a symbol that is not in the headers at all. Pin the
    /// corrected sequence's names and prove the invented one is gone.
    #[test]
    fn required_symbols_exclude_the_invented_name_and_include_the_real_ones() {
        let mut seen = std::collections::HashSet::new();
        for s in REQUIRED_SYMBOLS {
            assert!(seen.insert(*s), "duplicate symbol {s}");
        }
        assert!(
            !REQUIRED_SYMBOLS.contains(&"pw_thread_loop_get_context"),
            "pw_thread_loop_get_context is not part of PipeWire's public thread-loop API"
        );
        // Item 60a: header-declared is not library-exported. Seth's
        // libpipewire does not export this name, so the registry path (and
        // its listener) is gone rather than resolved.
        assert!(
            !REQUIRED_SYMBOLS.contains(&"pw_core_get_registry"),
            "pw_core_get_registry is not exported by the installed libpipewire"
        );
        assert!(
            !REQUIRED_SYMBOLS.contains(&"pw_registry_add_listener"),
            "the registry dependency was dropped in item 60a"
        );
        // The real construction sequence: loop -> context -> core (fd) -> stream.
        assert!(REQUIRED_SYMBOLS.contains(&"pw_thread_loop_get_loop"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_new"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_connect_fd"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_stream_new"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_stream_connect"));
    }
}
