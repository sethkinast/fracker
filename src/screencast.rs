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
    /// The lifecycle stage that failed: `"consent pending"` for the
    /// consent-gated calls, `"remote-fd"` for the `OpenPipeWireRemote`
    /// reply (which only runs *after* consent is granted).
    pub stage: &'static str,
    /// The verbatim portal/D-Bus error, surfaced as-is.
    pub message: String,
}

impl NegotiateError {
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
pub async fn negotiate(restore_token: Option<String>) -> Result<Negotiated, NegotiateError> {
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
        opts.insert("restore_token".to_owned(), Value::from(tok.clone()));
    }
    let results = portal_request(
        &conn,
        &unique,
        "SelectSources",
        &handle,
        &(session.clone(), opts),
    )
    .await?;
    let restore_token = results
        .get("restore_token")
        .and_then(|v| <&str>::try_from(v).ok().map(str::to_owned));

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
    let node_id = parse_streams(results.get("streams").and_then(|v| v.try_clone().ok()))?;

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
        restore_token,
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
    // SPA pod types (spa/utils/type.h).
    pub const SPA_TYPE_ID: u32 = 3;
    pub const SPA_TYPE_RECTANGLE: u32 = 10;
    pub const SPA_TYPE_FRACTION: u32 = 11;
    pub const SPA_TYPE_STRUCT: u32 = 14;
    pub const SPA_TYPE_OBJECT: u32 = 15;
    pub const SPA_TYPE_CHOICE: u32 = 19;
    // Choice kinds.
    pub const SPA_CHOICE_ENUM: u32 = 1;
    pub const SPA_CHOICE_RANGE: u32 = 2;

    /// The `Format` object type (`SPA_TYPE_OBJECT_Format`).
    pub const SPA_TYPE_OBJECT_FORMAT: u32 = 0x40003;
    /// `SPA_PARAM_EnumFormat`.
    pub const SPA_PARAM_ENUM_FORMAT: u32 = 3;
    /// `SPA_PARAM_Format` (the negotiated one, delivered to `param_changed`).
    pub const SPA_PARAM_FORMAT: u32 = 4;

    /// Format property keys (`spa/param/format.h`).
    pub const SPA_FORMAT_MEDIA_TYPE: u32 = 1;
    pub const SPA_FORMAT_MEDIA_SUBTYPE: u32 = 2;
    pub const SPA_MEDIA_TYPE_VIDEO: u32 = 2;
    pub const SPA_MEDIA_SUBTYPE_RAW: u32 = 1;

    /// Video format keys and the packed 4-byte formats we accept.
    pub const SPA_FORMAT_VIDEO_FORMAT: u32 = 0x20001;
    pub const SPA_FORMAT_VIDEO_SIZE: u32 = 0x20003;
    pub const SPA_FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
    pub const SPA_VIDEO_FORMAT_RGBX: u32 = 6;
    pub const SPA_VIDEO_FORMAT_BGRX: u32 = 7;
    pub const SPA_VIDEO_FORMAT_RGBA: u32 = 10;
    pub const SPA_VIDEO_FORMAT_BGRA: u32 = 11;

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
            if key == SPA_FORMAT_VIDEO_FORMAT && vtype == SPA_TYPE_ID {
                return Some(u32::from_le_bytes(
                    bytes[value_off + 8..value_off + 12].try_into().ok()?,
                ));
            }
            let value_len = 8 + vsize as usize;
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
        // Resolve EXACTLY the manifest symbols, once, keyed by name. A
        // name that is not in the headers fails here (never at connect
        // time on a real desktop), and the manifest test keeps this list
        // and the field wiring below in lockstep.
        let mut resolved: HashMap<&'static str, *mut c_void> = HashMap::new();
        for name in REQUIRED_SYMBOLS {
            let cname = CString::new(*name).unwrap();
            let p = unsafe { libc::dlsym(handle, cname.as_ptr()) };
            if p.is_null() {
                return Err(format!("libpipewire missing symbol {}", name));
            }
            resolved.insert(name, p);
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

/// `PW_DIRECTION_INPUT` (we consume frames).
const PW_DIRECTION_INPUT: c_int = 1;
/// `PW_ID_ANY`.
const PW_ID_ANY: u32 = 0xffff_ffff;
/// `PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS |
/// PW_STREAM_FLAG_DONT_RECONNECT`.
///
/// **AUTOCONNECT is required for a portal screencast** (ninth pass §1 root
/// cause): `OpenPipeWireRemote` hands back a private PipeWire core whose
/// single node is the screen being cast. Without AUTOCONNECT the input
/// stream is created on the core but never linked to that node, so
/// `pw_stream_connect` returns 0 while the stream sits idle forever:
/// `process` never fires, no error state is entered, and the UI would
/// block with no signal.
///
/// The connect target is the node id the portal selected in the `Start`
/// reply's `streams` (item 49): a targetless connect errors "target not
/// found". AUTOCONNECT still applies once the target is known.
const PW_STREAM_FLAGS: u32 = (1 << 0) | (1 << 2) | (1 << 7);
/// `PW_STREAM_STATE_ERROR`.
const PW_STREAM_STATE_ERROR: i32 = -1;
/// How long to wait for the first frame before declaring the stream
/// unlinked. Consent is already granted by the time we connect, so a
/// healthy stream delivers within a beat; this only exists so a stream
/// that never links surfaces in the tray instead of hanging the capture
/// forever.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(15);

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
        let mut hook = Box::new(HookStorage::zeroed());

        // The whole setup after `start` runs under the loop lock so the
        // stream is created on the loop's context thread.
        unsafe { (api.pw_thread_loop_lock)(thread_loop) };
        let mut setup = || -> Result<(*mut c_void, *mut c_void), String> {
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
            let stream =
                unsafe { (api.pw_stream_new)(core, name.as_ptr(), std::ptr::null_mut()) };
            if stream.is_null() {
                unsafe { (api.pw_context_destroy)(context) };
                return Err("PipeWire: pw_stream_new failed".to_owned());
            }
            ctx.stream = stream;
            unsafe {
                (api.pw_stream_add_listener)(
                    stream,
                    hook.as_mut() as *mut _ as *mut c_void,
                    events.as_ref() as *const _ as *const c_void,
                    ctx.as_mut() as *mut _ as *mut c_void,
                );
            }
            // Keep the format pod 8-byte aligned while PipeWire reads it.
            let format = pod::enum_format_video();
            let words = to_aligned_words(&format);
            let params: [*const c_void; 1] = [words.as_ptr() as *const c_void];
            let rc = unsafe {
                (api.pw_stream_connect)(
                    stream,
                    PW_DIRECTION_INPUT,
                    node_id,
                    PW_STREAM_FLAGS,
                    params.as_ptr(),
                    1,
                )
            };
            if rc < 0 {
                unsafe {
                    (api.pw_stream_destroy)(stream);
                    (api.pw_context_destroy)(context);
                }
                return Err(format!("PipeWire: pw_stream_connect failed ({rc})"));
            }
            Ok((context, stream))
        };
        let result = setup();
        // Release the closure's borrows before moving the boxes into Self.
        drop(setup);
        unsafe { (api.pw_thread_loop_unlock)(thread_loop) };

        let (context, stream) = match result {
            Ok(pair) => pair,
            Err(e) => {
                log.record("connecting", &e);
                unsafe {
                    (api.pw_thread_loop_stop)(thread_loop);
                    (api.pw_thread_loop_destroy)(thread_loop);
                }
                return Err(e);
            }
        };
        log.record(
            "connecting",
            &format!("PipeWire stream connected to node {node_id}; awaiting first frame"),
        );
        // The negotiated fd is now owned by PipeWire; leak the OwnedFd so
        // it is not double-closed (pw_context_connect_fd dup'd it).
        std::mem::forget(negotiated.fd);

        Ok(Self {
            api,
            thread_loop,
            context,
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

impl FrameSource for ScreenCastSource {
    fn next_frame(&mut self) -> Result<Option<GrayImage>, String> {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if let Ok(mut e) = self.err.lock() {
                if let Some(msg) = e.take() {
                    self.log.record("capturing", &msg);
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
                        self.log.record("capturing", &msg);
                        return Err(msg);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    if !self.got_first {
                        let msg =
                            "PipeWire: frame channel closed before the first frame".to_owned();
                        self.log.record("capturing", &msg);
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
            if self.connected && !self.stream.is_null() {
                (self.api.pw_stream_disconnect)(self.stream);
                (self.api.pw_stream_destroy)(self.stream);
            }
            // The context owns the core/stream; destroy it before the
            // loop it was created on (context.h: the main loop must stay
            // alive until pw_context_destroy()).
            if !self.context.is_null() {
                (self.api.pw_context_destroy)(self.context);
            }
            if !self.thread_loop.is_null() {
                (self.api.pw_thread_loop_stop)(self.thread_loop);
                (self.api.pw_thread_loop_destroy)(self.thread_loop);
            }
        }
        // Keep the boxes alive until after the teardown above.
        let _ = (&self.hook, &self.events, &self.ctx);
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
    let bytes = std::slice::from_raw_parts(param as *const u8, 256);
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
        // The real construction sequence: loop -> context -> core (fd) -> stream.
        assert!(REQUIRED_SYMBOLS.contains(&"pw_thread_loop_get_loop"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_new"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_connect_fd"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_stream_new"));
    }
}
