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
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
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
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";
const SESSION_CLOSE_METHOD: &str = "Close";

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

/// How long `Session.Close` may take at teardown. Closing is best-effort:
/// the stream is already gone by then, so a wedged portal must not hold
/// capture shutdown open.
const SESSION_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// A completed negotiation: the PipeWire remote fd, the ScreenCast node
/// the portal selected (the stream must connect to THIS node — a targetless
/// connect errors "target not found"), the (optionally refreshed) restore
/// token, and the **live portal session**.
///
/// **Addendum #69b.** The session lives on the session-bus connection that
/// negotiated it. Dropping that connection at the end of `negotiate()`
/// orphaned the session: the portal had tied it to the caller's unique name,
/// so `Session.Close` could never be sent and the session lingered until the
/// portal reaped it. [`PortalSession`] carries both the connection and the
/// object path through the capture and closes the session at teardown.
pub struct Negotiated {
    pub fd: OwnedFd,
    pub node_id: u32,
    pub restore_token: Option<String>,
    pub session: PortalSession,
}

/// The portal session + its dedicated D-Bus connection, kept alive for as
/// long as the capture owns the PipeWire remote and closed on drop.
///
/// `zbus::Connection` is a shared handle; dropping it closes the bus
/// connection (and thereby the portal session's identity) even though the
/// object is reference-counted. Holding one clone here is what keeps the
/// unique name alive.
pub struct PortalSession {
    /// `None` once the session has been closed, so `Drop` is idempotent.
    connection: Option<zbus::Connection>,
    path: ObjectPath<'static>,
    log: CaptureLog,
}

impl PortalSession {
    fn new(connection: zbus::Connection, path: ObjectPath<'static>, log: CaptureLog) -> Self {
        Self {
            connection: Some(connection),
            path,
            log,
        }
    }

    /// The object path of the portal session (`/org/freedesktop/portal/
    /// desktop/session/...`).
    pub fn path(&self) -> &ObjectPath<'static> {
        &self.path
    }

    /// Send `org.freedesktop.portal.Session.Close` on the connection that
    /// owns the session, then drop that connection. Safe to call more than
    /// once (drop calls it when the owner did not).
    ///
    /// This runs at capture teardown on the blocking worker thread, which
    /// has no Tokio runtime of its own, so the async zbus call gets a
    /// short-lived current-thread runtime in a helper thread. The join is
    /// bounded by [`SESSION_CLOSE_TIMEOUT`]; a failed close is logged, never
    /// swallowed.
    pub fn close(&mut self) {
        let Some(conn) = self.connection.take() else {
            return;
        };
        let path = self.path.to_string();
        let log = self.log.clone();
        let spawned = std::thread::Builder::new()
            .name("fracker-portal-close".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        log.record(
                            "finalizing",
                            &format!("could not close ScreenCast session: runtime: {e}"),
                        );
                        return;
                    }
                };
                let result = runtime.block_on(async {
                    tokio::time::timeout(
                        SESSION_CLOSE_TIMEOUT,
                        conn.call_method(
                            Some(PORTAL_DEST),
                            path.as_str(),
                            Some(SESSION_IFACE),
                            SESSION_CLOSE_METHOD,
                            &(),
                        ),
                    )
                    .await
                });
                match result {
                    Ok(Ok(_)) => log.record(
                        "finalizing",
                        &format!("ScreenCast session closed ({path})"),
                    ),
                    Ok(Err(e)) => log.record(
                        "finalizing",
                        &format!("ScreenCast Session.Close failed: {e}"),
                    ),
                    Err(_) => log.record(
                        "finalizing",
                        &format!(
                            "ScreenCast Session.Close timed out after {}s",
                            SESSION_CLOSE_TIMEOUT.as_secs()
                        ),
                    ),
                }
            });
        match spawned {
            Ok(handle) => {
                let _ = handle.join();
            }
            Err(e) => self
                .log
                .record("finalizing", &format!("could not close ScreenCast session: {e}")),
        }
    }
}

impl Drop for PortalSession {
    fn drop(&mut self) {
        self.close();
    }
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
    let session = ObjectPath::try_from(session_path.as_str())
        .map_err(|e| {
            NegotiateError::consent(format!("ScreenCast session path {session_path:?}: {e}"))
        })?
        .into_owned();
    // Item 69b: from here on every exit path owns the session and closes it
    // — including a refusal or a failed remote-fd call, not just a
    // completed capture.
    let portal_session = PortalSession::new(conn.clone(), session.clone(), log.clone());

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
        // Item 69b: this keeps the session (and its connection) alive until
        // the capture source is dropped, which closes it.
        session: portal_session,
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
    /// `SPA_TYPE_Long` (zero-based enum: None=1, Bool=2, Id=3, Int=4,
    /// Long=5 — `spa/include/spa/utils/type.h:35-39`).
    pub const SPA_TYPE_LONG: u32 = 5;
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
    /// `SPA_PARAM_Buffers` and its object type (`spa/include/spa/param/
    /// param.h`; `spa/include/spa/utils/type.h` — ParamBuffers is the
    /// object right after Format).
    pub const SPA_PARAM_BUFFERS: u32 = 5;
    pub const SPA_TYPE_OBJECT_PARAM_BUFFERS: u32 = 0x40004;
    /// Buffer-param property keys (`spa/include/spa/param/buffers.h`;
    /// `SPA_PARAM_BUFFERS_START` is 0). Only dataType is sent — the
    /// canonical consumer answer (OBS's linux-pipewire plugin sends
    /// exactly this one prop; count/size/stride derive from the Format).
    pub const SPA_PARAM_BUFFERS_DATA_TYPE: u32 = 6;
    /// `SPA_PARAM_Meta` and its object type (`spa/include/spa/param/
    /// param.h` — Meta follows Buffers in the param enum;
    /// `spa/include/spa/utils/type.h` — ParamMeta follows ParamBuffers).
    pub const SPA_PARAM_META: u32 = 6;
    pub const SPA_TYPE_OBJECT_PARAM_META: u32 = 0x40005;
    /// Meta-param property keys (`spa/include/spa/param/buffers.h:32-36`;
    /// `SPA_PARAM_META_START` is 0).
    pub const SPA_PARAM_META_TYPE: u32 = 1;
    pub const SPA_PARAM_META_SIZE: u32 = 2;
    /// `enum spa_meta_type` (`spa/include/spa/buffer/meta.h:29-39`,
    /// zero-based): Header=1, VideoCrop=2, Cursor=5.
    pub const SPA_META_HEADER: u32 = 1;
    pub const SPA_META_VIDEO_CROP: u32 = 2;
    pub const SPA_META_CURSOR: u32 = 5;
    /// `sizeof(struct spa_meta_header)` = u32+u32+i64+i64+u64
    /// (`spa/include/spa/buffer/meta.h:77-92`).
    pub const SPA_META_HEADER_SIZE: i32 = 32;
    /// `sizeof(struct spa_meta_region)` = one `spa_region`
    /// (point + rectangle, 8+8) (`spa/include/spa/buffer/meta.h:93-95`).
    pub const SPA_META_VIDEO_CROP_SIZE: i32 = 16;
    /// OBS's cursor meta sizes: `sizeof(spa_meta_cursor)` (28) +
    /// `sizeof(spa_meta_bitmap)` (20) + w*h*4, for 64x64 / 1x1 /
    /// 1024x1024 (OBS linux-pipewire `CURSOR_META_SIZE`).
    pub const SPA_META_CURSOR_SIZE: i32 = 28 + 20 + 64 * 64 * 4;
    pub const SPA_META_CURSOR_SIZE_MIN: i32 = 28 + 20 + 1 * 1 * 4;
    pub const SPA_META_CURSOR_SIZE_MAX: i32 = 28 + 20 + 1024 * 1024 * 4;
    pub const SPA_TYPE_INT: u32 = 4;

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
    /// `SPA_FORMAT_VIDEO_modifier` is a **Long** property, and the header
    /// says to use it only with DMA-BUF (`format.h:126-127`; the enum
    /// starts at `SPA_FORMAT_START_Video = 0x20000`, so format=0x20001,
    /// modifier=0x20002). The 64-bit payloads are DRM format modifiers
    /// (`include/uapi/drm/drm_fourcc.h`), not SPA enums.
    pub const SPA_FORMAT_VIDEO_MODIFIER: u32 = 0x20002;
    pub const SPA_FORMAT_VIDEO_SIZE: u32 = 0x20003;
    pub const SPA_FORMAT_VIDEO_FRAMERATE: u32 = 0x20004;
    /// `DRM_FORMAT_MOD_LINEAR` = `fourcc_mod_code(NONE, 0)` = 0
    /// (`include/uapi/drm/drm_fourcc.h:554`) and
    /// `DRM_FORMAT_MOD_INVALID` = `fourcc_mod_code(NONE, (1ULL<<56)-1)`
    /// (`drm_fourcc.h:493,544`) = 0x00ff_ffff_ffff_ffff.
    pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
    pub const DRM_FORMAT_MOD_INVALID: u64 = (1u64 << 56) - 1;
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

    /// A bare `Id` pod (`size = 4`; `SPA_POD_INIT_Id` uses
    /// `sizeof(uint32_t)` — `spa/include/spa/pod/builder.h:269`).
    pub fn id(value: u32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 4, SPA_TYPE_ID);
        o.extend_from_slice(&value.to_le_bytes());
        o
    }

    /// A bare `Int` pod (`size = 4`).
    pub fn int(value: i32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 4, SPA_TYPE_INT);
        o.extend_from_slice(&value.to_le_bytes());
        o
    }

    /// The `SPA_PARAM_Buffers` answer a consumer sends once the node has
    /// proposed a fixed `Format`: a bare-Int `dataType` mask of the
    /// memory types it can read (`spa_data_type` bits). Count, block
    /// size and stride are the node's business, derived from the
    /// negotiated Format. OBS sends dataType alone, as MemPtr plus
    /// DmaBuf when the Format carries a modifier.
    pub fn param_buffers(data_type_mask: u32) -> Vec<u8> {
        object(
            SPA_TYPE_OBJECT_PARAM_BUFFERS,
            SPA_PARAM_BUFFERS,
            &[prop(SPA_PARAM_BUFFERS_DATA_TYPE, int(data_type_mask as i32))],
        )
    }

    /// One `SPA_PARAM_Meta` declaration: which buffer metadata the
    /// consumer supports, and at what maximum size.
    pub fn param_meta(meta_type: u32, size: Vec<u8>) -> Vec<u8> {
        object(
            SPA_TYPE_OBJECT_PARAM_META,
            SPA_PARAM_META,
            &[
                prop(SPA_PARAM_META_TYPE, id(meta_type)),
                prop(SPA_PARAM_META_SIZE, size),
            ],
        )
    }

    /// The full negotiation answer OBS's linux-pipewire plugin sends
    /// from `param_changed(Format)`, in its order: VideoCrop meta,
    /// Cursor meta, the Buffers dataType, Header meta — one
    /// `pw_stream_update_params` call carrying all four. v0.5.14 sent
    /// the Buffers pod alone: KWin accepted the format (two proposals,
    /// then quiet) but never allocated a buffer; the Meta declarations
    /// are the missing half of the canonical answer.
    pub fn negotiation_params(has_modifier: bool) -> [Vec<u8>; 4] {
        // `spa_data_type`: MemPtr = 1, DmaBuf = 3 (bit positions).
        let mut mask: u32 = 1 << 1;
        if has_modifier {
            mask |= 1 << 3;
        }
        [
            param_meta(SPA_META_VIDEO_CROP, int(SPA_META_VIDEO_CROP_SIZE)),
            param_meta(
                SPA_META_CURSOR,
                choice(
                    SPA_CHOICE_RANGE,
                    &[
                        int(SPA_META_CURSOR_SIZE),
                        int(SPA_META_CURSOR_SIZE_MIN),
                        int(SPA_META_CURSOR_SIZE_MAX),
                    ],
                ),
            ),
            param_buffers(mask),
            param_meta(SPA_META_HEADER, int(SPA_META_HEADER_SIZE)),
        ]
    }

    /// A bare `Long` pod (`size = 8`: `struct spa_pod_long` wraps an
    /// `int64_t` — `spa/include/spa/pod/pod.h:73-76`; `SPA_POD_INIT_Long`,
    /// `spa/include/spa/pod/builder.h:285`). SPA carries DMA-BUF modifiers
    /// as Long values (`format.h:126-127`), so their 64 bits are written
    /// verbatim.
    pub fn long(value: u64) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 8, SPA_TYPE_LONG);
        o.extend_from_slice(&(value as i64).to_le_bytes());
        o
    }

    /// A `Rectangle` pod (`size = 8`: `struct spa_rectangle` is two
    /// `uint32_t`s — `spa/include/spa/pod/builder.h:370`,
    /// `spa/include/spa/utils/defs.h:102-105`).
    pub fn rect(w: u32, h: u32) -> Vec<u8> {
        let mut o = Vec::new();
        header(&mut o, 8, SPA_TYPE_RECTANGLE);
        o.extend_from_slice(&w.to_le_bytes());
        o.extend_from_slice(&h.to_le_bytes());
        o
    }

    /// A `Fraction` pod (`size = 8`: `struct spa_fraction` is two
    /// `uint32_t`s — `spa/include/spa/pod/builder.h:379`,
    /// `spa/include/spa/utils/defs.h:119-123`).
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

    /// A `Choice` pod in the real SPA layout (addendum #69a):
    ///
    /// ```text
    /// { size, SPA_TYPE_CHOICE,
    ///   kind, flags,
    ///   child.size, child.type,   // full pod header, copied from value 0
    ///   value0 body, value1 body, ... }  // RAW values, no per-value headers
    /// ```
    ///
    /// `struct spa_pod_choice_body` is `{ uint32_t type; uint32_t flags;
    /// struct spa_pod child; }` (`spa/include/spa/pod/pod.h`), and the
    /// builder copies the first pushed pod's header into `child` before
    /// writing each value's body at `child.size` stride. There is no
    /// wrapper `Struct` pod: every value after the first is just its raw
    /// payload. The previous implementation wrapped all children in a
    /// Struct, which made every EnumFormat choice (format, size,
    /// framerate, modifier) malformed — the cause of "no more input
    /// formats".
    ///
    /// Defaults first for `Enum` (value 0); default/min/max for `Range`.
    /// The returned `Vec` is exactly `8 + size` bytes; callers embed it in
    /// a prop, which supplies the 8-byte container padding.
    pub fn choice(kind: u32, children: &[Vec<u8>]) -> Vec<u8> {
        let (child_size, child_type) = children
            .first()
            .and_then(|c| read_header(c))
            .expect("a Choice pod needs at least one well-formed value");
        let body_len = 8 + 8 + child_size as usize * children.len();
        let mut o = Vec::with_capacity(8 + body_len);
        header(&mut o, body_len as u32, SPA_TYPE_CHOICE);
        o.extend_from_slice(&kind.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes());
        o.extend_from_slice(&child_size.to_le_bytes());
        o.extend_from_slice(&child_type.to_le_bytes());
        for child in children {
            let (size, ty) = read_header(child).expect("a Choice value must be a pod");
            assert_eq!(
                (size, ty),
                (child_size, child_type),
                "every Choice value shares the child header"
            );
            o.extend_from_slice(&child[8..8 + child_size as usize]);
        }
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

    /// The single `EnumFormat` param we offer: raw video in one of the
    /// packed 4-byte formats, any size in `[1x1 .. 8192x8192]`, 30 fps,
    /// and the DRM modifiers we can accept.
    ///
    /// **Addendum #65b.** The modifier property was absent, so KWin's
    /// screencast node (KPipeWire) had no modifier to intersect for its
    /// DMA-BUF format. It is a `Choice(Enum(Long))` carrying
    /// `DRM_FORMAT_MOD_LINEAR` (mappable) and the modifier-less sentinel
    /// `DRM_FORMAT_MOD_INVALID` last.
    ///
    /// **Addendum #69a.** Enum choices repeat their default as the first
    /// value, exactly as `spa_pod_builder` emits them: formats are
    /// `[BGRx, BGRx, BGRA, RGBx, RGBA]` and modifiers
    /// `[LINEAR, LINEAR, INVALID]`.
    pub fn enum_format_video() -> Vec<u8> {
        let formats = choice(
            SPA_CHOICE_ENUM,
            &[
                id(SPA_VIDEO_FORMAT_BGRX),
                id(SPA_VIDEO_FORMAT_BGRX),
                id(SPA_VIDEO_FORMAT_BGRA),
                id(SPA_VIDEO_FORMAT_RGBX),
                id(SPA_VIDEO_FORMAT_RGBA),
            ],
        );
        let sizes = choice(
            SPA_CHOICE_RANGE,
            &[rect(1920, 1080), rect(1, 1), rect(8192, 8192)],
        );
        let modifiers = choice(
            SPA_CHOICE_ENUM,
            &[
                long(DRM_FORMAT_MOD_LINEAR),
                long(DRM_FORMAT_MOD_LINEAR),
                long(DRM_FORMAT_MOD_INVALID),
            ],
        );
        // Item 69e: a range from 0/1 (any rate), not the pinned 30/1
        // fraction this used to be — KWin may offer a variable rate.
        let framerates = choice(
            SPA_CHOICE_RANGE,
            &[fraction(30, 1), fraction(0, 1), fraction(1000, 1)],
        );
        object(
            SPA_TYPE_OBJECT_FORMAT,
            SPA_PARAM_ENUM_FORMAT,
            &[
                prop(SPA_FORMAT_MEDIA_TYPE, id(SPA_MEDIA_TYPE_VIDEO)),
                prop(SPA_FORMAT_MEDIA_SUBTYPE, id(SPA_MEDIA_SUBTYPE_RAW)),
                prop(SPA_FORMAT_VIDEO_FORMAT, formats),
                prop(SPA_FORMAT_VIDEO_MODIFIER, modifiers),
                prop(SPA_FORMAT_VIDEO_SIZE, sizes),
                prop(SPA_FORMAT_VIDEO_FRAMERATE, framerates),
            ],
        )
    }

    /// The fixed `Format` param is parsed from the node's `Format` pod; no
    /// `Format` echo is built (item 69d). Kept as the negotiated-format
    /// view for frame shaping.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct VideoInfo {
        pub format: u32,
        pub width: u32,
        pub height: u32,
        pub framerate_num: u32,
        pub framerate_denom: u32,
        pub modifier: Option<u64>,
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

    /// One `spa_pod_prop` inside an object: `(key, value_type,
    /// value_pod_offset, next_prop_offset)`. `spa_pod_prop` is `key`,
    /// `flags`, then the value pod inline (`spa/include/spa/pod/pod.h:115-130`).
    pub fn prop_at(bytes: &[u8], off: usize) -> Option<(u32, u32, usize, usize)> {
        if off + 16 > bytes.len() {
            return None;
        }
        let key = u32::from_le_bytes(bytes[off..off + 4].try_into().ok()?);
        let value_off = off + 8;
        let (vsize, vtype) = read_header(&bytes[value_off..])?;
        let value_len = 8usize.checked_add(vsize as usize)?;
        if value_off + value_len > bytes.len() {
            return None;
        }
        let next = off + 8 + (value_len + 7) / 8 * 8;
        Some((key, vtype, value_off, next))
    }

    /// Find an object property by key: `(value_type, value_pod_offset)`.
    pub fn find_prop(bytes: &[u8], key: u32) -> Option<(u32, usize)> {
        let (_size, ty) = read_header(bytes)?;
        if ty != SPA_TYPE_OBJECT {
            return None;
        }
        // Skip the object body (type + id).
        let mut off = 16usize;
        while off + 16 <= bytes.len() {
            let (k, vtype, value_off, next) = prop_at(bytes, off)?;
            if k == key {
                return Some((vtype, value_off));
            }
            off = next;
        }
        None
    }

    /// Parse the fixed video `Format` object delivered to `param_changed`.
    /// Best-effort: `None` on anything we do not recognise, so a malformed
    /// pod does not confirm a bogus format or panic in the callback.
    pub fn parse_video_info(bytes: &[u8]) -> Option<VideoInfo> {
        let (_size, ty) = read_header(bytes)?;
        if ty != SPA_TYPE_OBJECT {
            return None;
        }
        let mut info = VideoInfo::default();
        let mut off = 16usize;
        while off + 16 <= bytes.len() {
            let (key, vtype, value_off, next) = prop_at(bytes, off)?;
            // A fixed Format proposal produced by libspa wraps EVERY value
            // in a Choice pod (kind None for fixed values, sometimes with
            // several near-duplicate entries). Unwrap to the first value:
            // the effective type is the choice's child type and the value
            // bytes follow the choice preamble (kind, flags, child header).
            // (Verified against a captured KWin proposal, 2026-10-22: bare-
            // scalar matching rejected every real Format pod.)
            let (etype, body) = if vtype == SPA_TYPE_CHOICE {
                let cbody = value_off + 8;
                if cbody + 16 > bytes.len() {
                    return None;
                }
                let child_type =
                    u32::from_le_bytes(bytes[cbody + 12..cbody + 16].try_into().ok()?);
                (child_type, cbody + 16)
            } else {
                (vtype, value_off + 8)
            };
            match key {
                SPA_FORMAT_VIDEO_FORMAT if etype == SPA_TYPE_ID && body + 4 <= bytes.len() => {
                    info.format = u32::from_le_bytes(bytes[body..body + 4].try_into().ok()?);
                }
                SPA_FORMAT_VIDEO_SIZE if etype == SPA_TYPE_RECTANGLE && body + 8 <= bytes.len() => {
                    info.width = u32::from_le_bytes(bytes[body..body + 4].try_into().ok()?);
                    info.height = u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
                }
                SPA_FORMAT_VIDEO_FRAMERATE
                    if etype == SPA_TYPE_FRACTION && body + 8 <= bytes.len() =>
                {
                    info.framerate_num = u32::from_le_bytes(bytes[body..body + 4].try_into().ok()?);
                    info.framerate_denom =
                        u32::from_le_bytes(bytes[body + 4..body + 8].try_into().ok()?);
                }
                SPA_FORMAT_VIDEO_MODIFIER if etype == SPA_TYPE_LONG && body + 8 <= bytes.len() => {
                    info.modifier =
                        Some(u64::from_le_bytes(bytes[body..body + 8].try_into().ok()?));
                }
                _ => {}
            }
            off = next;
        }
        // Unknown (0) is not a negotiated format; size 0 cannot be
        // confirmed back.
        if info.format == 0 || info.width == 0 || info.height == 0 {
            return None;
        }
        Some(info)
    }

    /// Find `SPA_FORMAT_VIDEO_format` in a negotiated `Format` object pod
    /// (delivered to `param_changed`). Narrow accessor used by frame
    /// conversion; `parse_video_info` is the full negotiation view.
    pub fn parse_video_format(bytes: &[u8]) -> Option<u32> {
        let (_size, ty) = read_header(bytes)?;
        if ty != SPA_TYPE_OBJECT {
            return None;
        }
        let (vtype, value_off) = find_prop(bytes, SPA_FORMAT_VIDEO_FORMAT)?;
        if vtype != SPA_TYPE_ID {
            return None;
        }
        let body = value_off + 8;
        if body + 4 > bytes.len() {
            return None;
        }
        Some(u32::from_le_bytes(bytes[body..body + 4].try_into().ok()?))
    }

    /// DRM fourcc for the packed formats we accept, for the negotiated-
    /// format log line. KWin maps `SPA_VIDEO_FORMAT_BGRA` to
    /// `DRM_FORMAT_ARGB8888` ("AR24") and so on (KWin
    /// `src/plugins/screencast/screencaststream.cpp`, supportedFormats).
    pub fn fourcc(format: u32) -> &'static str {
        match format {
            SPA_VIDEO_FORMAT_BGRA => "AR24",
            SPA_VIDEO_FORMAT_BGRX => "XR24",
            SPA_VIDEO_FORMAT_RGBA => "AB24",
            SPA_VIDEO_FORMAT_RGBX => "XB24",
            _ => "????",
        }
    }

    /// Human name for the formats we accept (log evidence).
    pub fn format_name(format: u32) -> &'static str {
        match format {
            SPA_VIDEO_FORMAT_BGRA => "BGRA",
            SPA_VIDEO_FORMAT_BGRX => "BGRX",
            SPA_VIDEO_FORMAT_RGBA => "RGBA",
            SPA_VIDEO_FORMAT_RGBX => "RGBX",
            _ => "unknown",
        }
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
///
/// `fd` is `int64_t` in the SPA ABI (`struct spa_data` in
/// `spa/include/spa/buffer/buffer.h`). It used to be hand-defined as `i32`
/// here: on little-endian the low half happened to be read correctly and
/// `mapoffset`/`maxsize` filled the same 8 bytes before `data`, so the
/// pointer offsets survived — alignment luck, not correctness (item 69c).
#[repr(C)]
struct SpaData {
    ty: u32,
    flags: u32,
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
    data: *mut c_void,
    chunk: *mut SpaChunk,
}

/// `enum spa_data_type` values we can map (`spa/include/spa/buffer/
/// buffer.h`: Invalid 0, MemPtr 1, MemFd 2, DmaBuf 3, Mem 4). MemFd and
/// DmaBuf carry a file descriptor; the rest cannot be mapped by us.
const SPA_DATA_MEM_FD: u32 = 2;
const SPA_DATA_DMA_BUF: u32 = 3;

/// `struct spa_chunk`.
#[repr(C)]
struct SpaChunk {
    offset: u32,
    size: u32,
    stride: i32,
    flags: i32,
}

/// A page-aligned mmap window: `mmap_offset = base`, the caller's data
/// starts `delta` bytes into it, and `len` bytes are mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MapRange {
    base: usize,
    delta: usize,
    len: usize,
}

/// Page-align an mmap window of `len` bytes starting at `start`.
///
/// `mmap(2)` requires a page-aligned file offset, while SPA's
/// `mapoffset`/`chunk.offset` need not be. Pure and tested so the
/// arithmetic (the part that can underflow) never depends on a live
/// buffer.
fn map_range(start: usize, len: usize, page: usize) -> Option<MapRange> {
    if len == 0 || page == 0 {
        return None;
    }
    let base = (start / page) * page;
    let delta = start - base;
    let len = delta.checked_add(len)?;
    Some(MapRange { base, delta, len })
}

/// One live mmap of a portal buffer's memory.
///
/// `base` is the page-aligned mapping; the buffer's `spa_data.data` is
/// `base + delta`. Dropping the mapping unmaps it — `remove_buffer` removes
/// the entry, and `StreamCtx::clear_mappings` catches anything left at
/// teardown.
struct BufferMapping {
    base: *mut c_void,
    len: usize,
    delta: usize,
    /// Identity of what is mapped, so a reused `pw_buffer` pointer with a
    /// different fd/range is remapped instead of reusing stale memory.
    fd: i64,
    mapoffset: u32,
    maxsize: u32,
}

impl BufferMapping {
    fn data(&self) -> *const u8 {
        unsafe { (self.base as *const u8).add(self.delta) }
    }
}

impl Drop for BufferMapping {
    fn drop(&mut self) {
        if !self.base.is_null() && self.len != 0 {
            unsafe {
                libc::munmap(self.base, self.len);
            }
        }
    }
}

impl StreamCtx {
    /// Unmap every buffer still held at teardown.
    fn clear_mappings(&self) {
        if let Ok(mut maps) = self.maps.lock() {
            maps.clear();
        }
    }
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
    "pw_properties_new_string", // properties.h (stream properties, item 69e)
    "pw_stream_new",            // stream.h
    "pw_stream_add_listener",   // stream.h
    "pw_stream_connect",        // stream.h
    "pw_stream_update_params",  // stream.h (the SPA_PARAM_Buffers answer)
    "pw_stream_set_active",     // stream.h (start the graph after connect)
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
    /// `properties.h`: build stream properties from a `key=value` string.
    /// `pw_stream_new` takes ownership of the returned properties.
    pw_properties_new_string: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    pw_stream_new: unsafe extern "C" fn(*mut c_void, *const c_char, *mut c_void) -> *mut c_void,
    pw_stream_add_listener:
        unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *mut c_void),
    pw_stream_connect:
        unsafe extern "C" fn(*mut c_void, c_int, u32, u32, *const *const c_void, u32) -> c_int,
    /// `stream.h`: hand the node our buffer parameters once it proposes
    /// a fixed format (the canonical accept; NOT a Format echo).
    pw_stream_update_params: unsafe extern "C" fn(*mut c_void, *const *const c_void, u32) -> c_int,
    /// `stream.h`: activate the stream — the canonical consumer start
    /// (OBS calls it right after connect; without it the stream sits in
    /// PAUSED while formats cycle).
    pw_stream_set_active: unsafe extern "C" fn(*mut c_void, bool) -> c_int,
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
            pw_properties_new_string: sym!(
                "pw_properties_new_string",
                unsafe extern "C" fn(*const c_char) -> *mut c_void
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
            pw_stream_update_params: sym!(
                "pw_stream_update_params",
                unsafe extern "C" fn(*mut c_void, *const *const c_void, u32) -> c_int
            ),
            pw_stream_set_active: sym!(
                "pw_stream_set_active",
                unsafe extern "C" fn(*mut c_void, bool) -> c_int
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
    tx: SyncSender<GrayImage>,
    /// The negotiated `Format` video info (item 69e: frames are shaped by
    /// this, not by `stride / 4`). `None` until `param_changed` lands.
    info: Mutex<Option<pod::VideoInfo>>,
    /// mmap'd MemFd/DmaBuf buffers, keyed by `pw_buffer` pointer so
    /// `remove_buffer` can unmap on release (item 69c).
    maps: Mutex<HashMap<usize, BufferMapping>>,
    /// One-shot evidence when a buffer cannot be mapped; per-frame logging
    /// would flood the capture log.
    map_failed_logged: AtomicBool,
    /// Last `io_changed` id logged (transition-only logging).
    last_io: AtomicU32,
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
    /// Item 69b: the portal session the remote belongs to. Dropped (and
    /// therefore `Session.Close`d) after the PipeWire teardown in `Drop`.
    session: PortalSession,
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
        // Item 69b: keep the portal session (connection + object path) alive
        // for the whole stream. Every early return below drops `session`,
        // which sends Session.Close instead of orphaning it.
        let Negotiated {
            fd,
            node_id,
            restore_token: _,
            session,
        } = negotiated;
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

        // Item 69e: capacity 1. The PipeWire thread produces frames far
        // faster than OCR drains them; at most one frame may ever queue,
        // and a full queue drops the incoming frame at the producer.
        let (tx, rx) = sync_channel::<GrayImage>(1);
        let err = Arc::new(Mutex::new(None));
        let ctx = Box::new(StreamCtx {
            api,
            stream: std::ptr::null_mut(),
            tx,
            info: Mutex::new(None),
            maps: Mutex::new(HashMap::new()),
            map_failed_logged: AtomicBool::new(false),
            last_io: AtomicU32::new(u32::MAX),
            err: err.clone(),
            log: log.clone(),
        });
        let mut ctx = ctx;
        let events = Box::new(StreamEvents {
            version: 2,
            destroy: None,
            state_changed: Some(on_state_changed),
            control_info: None,
            io_changed: Some(on_io_changed),
            param_changed: Some(on_param_changed),
            add_buffer: Some(on_add_buffer),
            remove_buffer: Some(on_remove_buffer),
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
            let fd = fd.as_raw_fd();
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
            // Item 69e: name the stream's purpose for the session manager
            // (OBS-style) instead of passing NULL properties. Advisory:
            // if the properties cannot be built, fall back to NULL rather
            // than failing the capture. pw_stream_new takes ownership.
            let props = CString::new("media.type=Video media.category=Capture media.role=Screen")
                .map(|s| unsafe { (api.pw_properties_new_string)(s.as_ptr()) })
                .unwrap_or(std::ptr::null_mut());
            let created = unsafe { (api.pw_stream_new)(core, name.as_ptr(), props) };
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
            log.record(
                "connecting",
                &format!(
                    "PipeWire EnumFormat offer ({} bytes): {}",
                    format.len(),
                    hex_dump(&format)
                ),
            );
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
            // Start the graph (OBS's consumer flow): an inactive stream
            // negotiates forever and never leaves PAUSED.
            let arc = unsafe { (api.pw_stream_set_active)(created, true) };
            if arc < 0 {
                log.record(
                    "connecting",
                    &format!("PipeWire: pw_stream_set_active(true) failed ({arc})"),
                );
            }
            break (created, attempt_hook);
        };
        // The negotiated fd is now owned by PipeWire; leak the OwnedFd so
        // it is not double-closed (pw_context_connect_fd dup'd it).
        std::mem::forget(fd);

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
            session,
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
                            "PipeWire: no first frame within {}s — negotiation produced no frame",
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
        self.ctx.clear_mappings();
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

/// Lowercase hex, for capture-log evidence dumps of pod bytes.
fn hex_dump(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

unsafe extern "C" fn on_param_changed(data: *mut c_void, id: u32, param: *const c_void) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    // Capture diagnostics: EVERY param change is logged while format
    // negotiation is the open question — which ids arrive at all, and the
    // raw bytes of any Format pod we cannot parse. (Earlier revisions
    // bailed silently on both, which made "never negotiated" and
    // "negotiated but unparseable" indistinguishable from the log.)
    if param.is_null() {
        ctx.log
            .record("connecting", &format!("PipeWire param_changed id={id} (null pod)"));
        return;
    }
    // The pod's own `size` header bounds the read. Probe pods are often
    // ≥256B, but the SPA ABI lets a format pod be much smaller (and a
    // `SPA_TYPE_None` pod has no body at all); a fixed 256-byte slice
    // would read past the object in both cases.
    let header = std::slice::from_raw_parts(param as *const u8, 8);
    let Some(len) = pod::pod_total_len(header) else {
        let raw = std::slice::from_raw_parts(param as *const u8, 64);
        ctx.log.record(
            "connecting",
            &format!(
                "PipeWire param_changed id={id}: unreadable pod header; first 64B: {}",
                hex_dump(raw)
            ),
        );
        return;
    };
    let bytes = std::slice::from_raw_parts(param as *const u8, len);
    if id != pod::SPA_PARAM_FORMAT {
        ctx.log.record(
            "connecting",
            &format!("PipeWire param_changed id={id} ({len} bytes, not Format)"),
        );
        return;
    }
    let Some(info) = pod::parse_video_info(bytes) else {
        ctx.log.record(
            "connecting",
            &format!(
                "PipeWire Format pod FAILED to parse ({len} bytes): {}",
                hex_dump(&bytes[..bytes.len().min(768)])
            ),
        );
        return;
    };
    // Item 69d, refined by the 2026-10-22 capture: no Format echo (that
    // was the pass-21 mechanism, and it is not the canonical flow) — but
    // the handshake is only complete once the consumer answers with its
    // buffer parameters. Record the format, then send SPA_PARAM_Buffers.
    // A node mid-negotiation re-proposes often; answer every time, but
    // log only when the format actually changes.
    let changed = if let Ok(mut slot) = ctx.info.lock() {
        let c = *slot != Some(info);
        *slot = Some(info);
        c
    } else {
        true
    };
    let pods = pod::negotiation_params(info.modifier.is_some());
    let words: Vec<Vec<u64>> = pods.iter().map(|p| to_aligned_words(p)).collect();
    let params: Vec<*const c_void> = words
        .iter()
        .map(|w| w.as_ptr() as *const c_void)
        .collect();
    let brc = (ctx.api.pw_stream_update_params)(ctx.stream, params.as_ptr(), params.len() as u32);
    if brc < 0 {
        ctx.log.record(
            "connecting",
            &format!("PipeWire: pw_stream_update_params(negotiation) failed ({brc})"),
        );
    }

    // Addendum #65: the negotiated size is also the practical evidence
    // that the restored portal source is the expected window (a 2560x1440
    // game surface, not the desktop monitor). Fourcc makes it comparable
    // with KWin/KPipeWire logs.
    let (num, denom) = if info.framerate_denom == 0 {
        (0, 1)
    } else {
        (info.framerate_num, info.framerate_denom)
    };
    let modifier = match info.modifier {
        Some(m) if m == pod::DRM_FORMAT_MOD_LINEAR => ", modifier linear (0x0)".to_owned(),
        Some(m) if m == pod::DRM_FORMAT_MOD_INVALID => {
            format!(", modifier invalid (0x{m:016x})")
        }
        Some(m) => format!(", modifier 0x{m:016x}"),
        None => ", no modifier".to_owned(),
    };
    if !changed {
        return;
    }
    ctx.log.record(
        "connecting",
        &format!(
            "PipeWire negotiated format: {} (fourcc {}, spa {:#x}) {}x{} @ {}/{} fps{}; \
             answered with SPA_PARAM_Buffers + Meta (VideoCrop/Cursor/Header)",
            pod::format_name(info.format),
            pod::fourcc(info.format),
            info.format,
            info.width,
            info.height,
            num,
            denom,
            modifier
        ),
    );
}

/// `io_changed`: an IO area was assigned — negotiation evidence. A node
/// stuck mid-negotiation re-fires the same id thousands of times, so log
/// only transitions.
unsafe extern "C" fn on_io_changed(data: *mut c_void, id: u32, _area: *mut c_void, _size: u32) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    if ctx.last_io.swap(id, Ordering::Relaxed) != id {
        ctx.log
            .record("connecting", &format!("PipeWire io_changed id={id}"));
    }
}

/// `add_buffer`: the node allocated a buffer — negotiation evidence; the
/// mapping itself happens lazily in `buffer_bytes`.
unsafe extern "C" fn on_add_buffer(data: *mut c_void, _buffer: *mut c_void) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    ctx.log
        .record("connecting", "PipeWire add_buffer: node allocated a buffer");
}

/// `remove_buffer`: the node is done with a buffer, so unmap any memory we
/// mapped for it (item 69c).
unsafe extern "C" fn on_remove_buffer(data: *mut c_void, buffer: *mut c_void) {
    if data.is_null() {
        return;
    }
    let ctx = &mut *(data as *mut StreamCtx);
    if let Ok(mut maps) = ctx.maps.lock() {
        // Dropping the entry munmaps it.
        maps.remove(&(buffer as usize));
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
        // Bounded queue (capacity 1): a full queue means OCR has not
        // drained the previous frame yet — drop this one, never block
        // the PipeWire thread and never let frames pile up.
        match ctx.tx.try_send(img) {
            Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
        }
    }
    (api.pw_stream_queue_buffer)(ctx.stream, b);
}

/// The page size used to align buffer mmaps (`sysconf` is the portable
/// spelling; 4096 is the fallback).
fn page_size() -> usize {
    let p = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if p > 0 {
        p as usize
    } else {
        4096
    }
}

/// Shape one frame from the **negotiated format** (item 69e), never from
/// `stride / 4`: a padded stride makes that width wrong. Returns
/// `(width, height, stride)` when the chunk is fully inside `data_len`.
fn frame_geometry(
    info: pod::VideoInfo,
    chunk: &SpaChunk,
    data_len: usize,
) -> Option<(usize, usize, usize)> {
    let width = info.width as usize;
    let height = info.height as usize;
    if width == 0 || height == 0 || width * height > 16_000_000 {
        return None;
    }
    if chunk.stride <= 0 {
        return None;
    }
    let stride = chunk.stride as usize;
    let row = width.checked_mul(4)?;
    if stride < row {
        return None;
    }
    let offset = chunk.offset as usize;
    let size = if chunk.size == 0 {
        data_len.checked_sub(offset)?
    } else {
        chunk.size as usize
    };
    let needed = (height - 1).checked_mul(stride)?.checked_add(row)?;
    if needed > size || offset.checked_add(needed)? > data_len {
        return None;
    }
    Some((width, height, stride))
}

/// Get a readable pointer + length for one `spa_data` (item 69c).
///
/// PipeWire may have mapped the buffer already (`PW_STREAM_FLAG_MAP_BUFFERS`
/// — `data.data` non-null), in which case that pointer is used directly.
/// When it has not, MemFd and DmaBuf datas are mmap'd from `data.fd` for
/// the buffer's `maxsize` starting at `mapoffset`; the mapping is cached
/// per `pw_buffer` and unmapped by `remove_buffer`.
unsafe fn buffer_bytes(
    ctx: &mut StreamCtx,
    b: *mut c_void,
    d: *mut SpaData,
    data_len: usize,
) -> Option<(*const u8, usize)> {
    let data = &mut *d;
    if !data.data.is_null() {
        return Some((data.data as *const u8, data_len));
    }
    if !matches!(data.ty, SPA_DATA_MEM_FD | SPA_DATA_DMA_BUF) || data.fd < 0 {
        return None;
    }
    let range = match map_range(data.mapoffset as usize, data_len, page_size()) {
        Some(r) => r,
        None => return None,
    };
    let key = b as usize;
    let mut maps = match ctx.maps.lock() {
        Ok(m) => m,
        Err(_) => return None,
    };
    let same = maps
        .get(&key)
        .map(|m| m.fd == data.fd && m.mapoffset == data.mapoffset && m.maxsize == data.maxsize)
        .unwrap_or(false);
    if !same {
        let ptr = libc::mmap(
            std::ptr::null_mut(),
            range.len,
            libc::PROT_READ,
            libc::MAP_SHARED,
            data.fd as c_int,
            range.base as libc::off_t,
        );
        if ptr == libc::MAP_FAILED {
            let err = std::io::Error::last_os_error();
            if !ctx.map_failed_logged.swap(true, Ordering::Relaxed) {
                ctx.log.record(
                    "capturing",
                    &format!(
                        "PipeWire: cannot map buffer (data type {}, fd {}, mapoffset {}, \
                         maxsize {}): {err}",
                        data.ty, data.fd, data.mapoffset, data.maxsize
                    ),
                );
            }
            return None;
        }
        let mapping = BufferMapping {
            base: ptr,
            len: range.len,
            delta: range.delta,
            fd: data.fd,
            mapoffset: data.mapoffset,
            maxsize: data.maxsize,
        };
        // Replacing the entry drops (unmaps) any stale mapping for this
        // reused buffer pointer.
        maps.insert(key, mapping);
    }
    maps.get(&key).map(|m| (m.data(), data_len))
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
    let info = (*ctx.info.lock().ok()?)?;
    let d = (*spa).datas;
    let chunk = (*d).chunk;
    if chunk.is_null() {
        return None;
    }
    // A non-linear DmaBuf is not readable as pixels. The offer carries
    // LINEAR (and the modifier-less sentinel); if the node still picks a
    // tiled modifier, drop the frame with one line of evidence rather
    // than reading garbage.
    if let Some(m) = info.modifier {
        if m != pod::DRM_FORMAT_MOD_LINEAR && m != pod::DRM_FORMAT_MOD_INVALID {
            if !ctx.map_failed_logged.swap(true, Ordering::Relaxed) {
                ctx.log.record(
                    "capturing",
                    &format!("PipeWire: non-linear buffer modifier 0x{m:016x}; cannot read pixels"),
                );
            }
            return None;
        }
    }
    let offset = (*chunk).offset as usize;
    let data_len = if (*d).maxsize != 0 {
        (*d).maxsize as usize
    } else {
        offset.checked_add((*chunk).size as usize)?
    };
    if data_len == 0 {
        return None;
    }
    let (ptr, len) = buffer_bytes(ctx, b, d, data_len)?;
    read_frame(info, ptr, len, &*chunk)
}

/// Shape and convert one already-mapped frame into grayscale.
///
/// Pure apart from the source bytes, so the geometry (negotiated width/
/// height, padded stride, chunk bounds) is unit-tested with a plain byte
/// buffer.
fn read_frame(
    info: pod::VideoInfo,
    ptr: *const u8,
    data_len: usize,
    chunk: &SpaChunk,
) -> Option<GrayImage> {
    let (width, height, stride) = frame_geometry(info, chunk, data_len)?;
    // Byte order of the three colour channels: BGRA/BGRx store B,G,R;
    // RGBA/RGBx store R,G,B. Alpha is ignored either way.
    let (r, g, bl) = match info.format {
        pod::SPA_VIDEO_FORMAT_BGRA | pod::SPA_VIDEO_FORMAT_BGRX => (2usize, 1usize, 0usize),
        pod::SPA_VIDEO_FORMAT_RGBA | pod::SPA_VIDEO_FORMAT_RGBX => (0usize, 1usize, 2usize),
        _ => return None,
    };
    let mut pixels = vec![0u8; width * height];
    // The caller guarantees `ptr` covers `data_len`, and `frame_geometry`
    // proved the chunk (stride/height) fits inside it.
    unsafe {
        let src = ptr.add(chunk.offset as usize);
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
                "SPA_TYPE_LONG",
                pod::SPA_TYPE_LONG,
                5,
                "spa/include/spa/utils/type.h:35-39",
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
                "SPA_FORMAT_VIDEO_MODIFIER",
                pod::SPA_FORMAT_VIDEO_MODIFIER,
                0x20002,
                "spa/include/spa/param/format.h:124,126-127",
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
        // DMA-BUF modifiers are 64-bit DRM tokens carried as SPA Long.
        // `DRM_FORMAT_MOD_LINEAR` = fourcc_mod_code(NONE, 0) = 0
        // (`include/uapi/drm/drm_fourcc.h:554`);
        // `DRM_FORMAT_MOD_INVALID` = fourcc_mod_code(NONE, (1ULL<<56)-1)
        // (`drm_fourcc.h:493,544`).
        assert_eq!(pod::DRM_FORMAT_MOD_LINEAR, 0, "drm_fourcc.h:554");
        assert_eq!(
            pod::DRM_FORMAT_MOD_INVALID,
            0x00ff_ffff_ffff_ffff,
            "drm_fourcc.h:493,544"
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

    /// Pod body sizes are the C struct sizes, not the aligned struct size:
    /// Id = 4 (`SPA_POD_INIT_Id`, `spa/include/spa/pod/builder.h:269`),
    /// Rectangle = 8 (`SPA_POD_INIT_Rectangle`, `builder.h:370`),
    /// Fraction = 8 (`SPA_POD_INIT_Fraction`, `builder.h:379`), with
    /// `struct spa_rectangle`/`spa_fraction` in `spa/utils/defs.h:102-123`.
    #[test]
    fn pod_bodies_have_the_c_abi_layout() {
        let id = pod::id(11);
        assert_eq!(pod::read_header(&id), Some((4, pod::SPA_TYPE_ID)));
        assert_eq!(id.len(), 12, "header + u32 id");

        let r = pod::rect(2560, 1440);
        assert_eq!(pod::read_header(&r), Some((8, pod::SPA_TYPE_RECTANGLE)));
        assert_eq!(r.len(), 16, "header + struct spa_rectangle");
        assert_eq!(u32::from_le_bytes(r[8..12].try_into().unwrap()), 2560);
        assert_eq!(u32::from_le_bytes(r[12..16].try_into().unwrap()), 1440);

        let f = pod::fraction(30, 1);
        assert_eq!(pod::read_header(&f), Some((8, pod::SPA_TYPE_FRACTION)));
        assert_eq!(f.len(), 16, "header + struct spa_fraction");
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

    /// Choice layout (addendum #69a): the header after the pod header is
    /// `kind, flags, child.size, child.type`, then the raw value bodies at
    /// `child.size` stride. There must be no Struct wrapper and no
    /// per-value pod headers.
    #[test]
    fn choice_pods_carry_kind_a_child_header_and_raw_values() {
        let c = pod::choice(pod::SPA_CHOICE_ENUM, &[pod::id(11), pod::id(10)]);
        let (size, ty) = pod::read_header(&c).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_CHOICE);
        // size = kind + flags + child header + two 4-byte Id bodies.
        assert_eq!(size as usize, 8 + 8 + 2 * 4);
        assert_eq!(c.len(), 8 + size as usize, "no padding inside the pod");
        assert_eq!(
            u32::from_le_bytes(c[8..12].try_into().unwrap()),
            pod::SPA_CHOICE_ENUM
        );
        assert_eq!(u32::from_le_bytes(c[12..16].try_into().unwrap()), 0);
        assert_eq!(
            u32::from_le_bytes(c[16..20].try_into().unwrap()),
            4,
            "child.size is the first value's body size"
        );
        assert_eq!(
            u32::from_le_bytes(c[20..24].try_into().unwrap()),
            pod::SPA_TYPE_ID,
            "child.type is the first value's pod type"
        );
        assert_eq!(u32::from_le_bytes(c[24..28].try_into().unwrap()), 11);
        assert_eq!(u32::from_le_bytes(c[28..32].try_into().unwrap()), 10);

        // The Long (modifier) shape has the same five-word header and
        // 8-byte raw values.
        let l = pod::choice(
            pod::SPA_CHOICE_ENUM,
            &[
                pod::long(pod::DRM_FORMAT_MOD_LINEAR),
                pod::long(pod::DRM_FORMAT_MOD_LINEAR),
                pod::long(pod::DRM_FORMAT_MOD_INVALID),
            ],
        );
        let (size, ty) = pod::read_header(&l).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_CHOICE);
        assert_eq!(size as usize, 8 + 8 + 3 * 8);
        assert_eq!(
            u32::from_le_bytes(l[20..24].try_into().unwrap()),
            pod::SPA_TYPE_LONG
        );
        assert_eq!(
            u64::from_le_bytes(l[24..32].try_into().unwrap()),
            pod::DRM_FORMAT_MOD_LINEAR
        );
        assert_eq!(
            u64::from_le_bytes(l[32..40].try_into().unwrap()),
            pod::DRM_FORMAT_MOD_LINEAR
        );
        assert_eq!(
            u64::from_le_bytes(l[40..48].try_into().unwrap()),
            pod::DRM_FORMAT_MOD_INVALID
        );

        // Range choices use the same layout with three values.
        let r = pod::choice(
            pod::SPA_CHOICE_RANGE,
            &[pod::rect(1920, 1080), pod::rect(1, 1), pod::rect(8192, 8192)],
        );
        let (size, ty) = pod::read_header(&r).unwrap();
        assert_eq!(ty, pod::SPA_TYPE_CHOICE);
        assert_eq!(size as usize, 8 + 8 + 3 * 8);
        assert_eq!(
            u32::from_le_bytes(r[20..24].try_into().unwrap()),
            pod::SPA_TYPE_RECTANGLE
        );
    }

    /// Every EnumFormat choice is a real SPA Choice: default duplicated
    /// first, child header from value 0, raw values packed behind it.
    #[test]
    fn enum_format_choices_have_the_spa_choice_layout() {
        let p = pod::enum_format_video();
        let values = |key: u32| -> (u32, u32, Vec<u64>) {
            let (vtype, voff) = pod::find_prop(&p, key).expect("property present");
            assert_eq!(vtype, pod::SPA_TYPE_CHOICE);
            let (size, ty) = pod::read_header(&p[voff..]).unwrap();
            assert_eq!(ty, pod::SPA_TYPE_CHOICE);
            assert_eq!(
                pod::pod_total_len(&p[voff..voff + 8]),
                Some(size as usize + 8),
                "the choice's size header covers its whole body"
            );
            let kind = u32::from_le_bytes(p[voff + 8..voff + 12].try_into().unwrap());
            let child_size =
                u32::from_le_bytes(p[voff + 16..voff + 20].try_into().unwrap()) as usize;
            let child_type =
                u32::from_le_bytes(p[voff + 20..voff + 24].try_into().unwrap());
            let count = (size as usize - 16) / child_size;
            let mut out = Vec::new();
            for i in 0..count {
                let at = voff + 24 + i * child_size;
                out.push(match child_size {
                    4 => u32::from_le_bytes(p[at..at + 4].try_into().unwrap()) as u64,
                    8 => u64::from_le_bytes(p[at..at + 8].try_into().unwrap()),
                    _ => panic!("unexpected child size {child_size}"),
                });
            }
            (kind, child_type, out)
        };

        // Format: default BGRx first, then the accepted set.
        let (kind, ctype, formats) = values(pod::SPA_FORMAT_VIDEO_FORMAT);
        assert_eq!(kind, pod::SPA_CHOICE_ENUM);
        assert_eq!(ctype, pod::SPA_TYPE_ID);
        assert_eq!(
            formats,
            vec![
                pod::SPA_VIDEO_FORMAT_BGRX as u64,
                pod::SPA_VIDEO_FORMAT_BGRX as u64,
                pod::SPA_VIDEO_FORMAT_BGRA as u64,
                pod::SPA_VIDEO_FORMAT_RGBX as u64,
                pod::SPA_VIDEO_FORMAT_RGBA as u64,
            ]
        );

        // Modifier: LINEAR default first and repeated, INVALID last.
        let (kind, ctype, modifiers) = values(pod::SPA_FORMAT_VIDEO_MODIFIER);
        assert_eq!(kind, pod::SPA_CHOICE_ENUM);
        assert_eq!(ctype, pod::SPA_TYPE_LONG);
        assert_eq!(
            modifiers,
            vec![
                pod::DRM_FORMAT_MOD_LINEAR,
                pod::DRM_FORMAT_MOD_LINEAR,
                pod::DRM_FORMAT_MOD_INVALID,
            ]
        );

        // Size: Range with default then min/max.
        let (kind, ctype, _) = values(pod::SPA_FORMAT_VIDEO_SIZE);
        assert_eq!(kind, pod::SPA_CHOICE_RANGE);
        assert_eq!(ctype, pod::SPA_TYPE_RECTANGLE);
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

    /// Item 69c: `spa_data.fd` is 64-bit. The old `i32` field survived
    /// only because `mapoffset`/`maxsize` happened to fill the same eight
    /// bytes before `data`; the pointer offsets must stay pinned.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn spa_data_uses_a_64_bit_fd_at_the_c_abi_offsets() {
        use std::mem::offset_of;
        assert_eq!(std::mem::size_of::<SpaData>(), 40);
        assert_eq!(offset_of!(SpaData, ty), 0);
        assert_eq!(offset_of!(SpaData, flags), 4);
        assert_eq!(offset_of!(SpaData, fd), 8);
        assert_eq!(offset_of!(SpaData, mapoffset), 16);
        assert_eq!(offset_of!(SpaData, maxsize), 20);
        assert_eq!(offset_of!(SpaData, data), 24);
        assert_eq!(offset_of!(SpaData, chunk), 32);
    }

    /// `mmap` needs a page-aligned file offset; SPA offsets need not be.
    #[test]
    fn map_range_page_aligns_the_file_offset() {
        assert_eq!(
            map_range(0, 4096, 4096),
            Some(MapRange {
                base: 0,
                delta: 0,
                len: 4096
            })
        );
        assert_eq!(
            map_range(4096 + 100, 4000, 4096),
            Some(MapRange {
                base: 4096,
                delta: 100,
                len: 4100
            })
        );
        assert_eq!(map_range(123, 0, 4096), None);
        assert_eq!(map_range(123, 10, 0), None);
    }

    /// Item 69e: dimensions come from the negotiated format, not
    /// `stride / 4`. A padded stride (20 bytes for a 4-pixel row) must
    /// still yield a 4-pixel-wide frame.
    #[test]
    fn frame_geometry_uses_the_negotiated_dimensions_not_the_stride() {
        let info = pod::VideoInfo {
            format: pod::SPA_VIDEO_FORMAT_BGRX,
            width: 4,
            height: 2,
            framerate_num: 30,
            framerate_denom: 1,
            modifier: None,
        };
        let chunk = SpaChunk {
            offset: 0,
            size: 40,
            stride: 20,
            flags: 0,
        };
        assert_eq!(frame_geometry(info, &chunk, 40), Some((4, 2, 20)));
        assert_ne!(chunk.stride as usize / 4, 4, "the old derivation was wrong");

        // The chunk must actually contain the rows the format describes.
        let short = SpaChunk { size: 20, ..chunk };
        assert_eq!(frame_geometry(info, &short, 20), None);
        assert_eq!(frame_geometry(info, &chunk, 35), None);
        let narrow = SpaChunk { stride: 12, ..chunk };
        assert_eq!(frame_geometry(info, &narrow, 40), None);
        let empty = pod::VideoInfo { width: 0, ..info };
        assert_eq!(frame_geometry(empty, &chunk, 40), None);
    }

    /// The conversion reads exactly the negotiated width and skips stride
    /// padding. BGRA stores B,G,R; RGBA stores R,G,B.
    #[test]
    fn read_frame_reads_bgra_rows_with_padding() {
        let info = pod::VideoInfo {
            format: pod::SPA_VIDEO_FORMAT_BGRA,
            width: 2,
            height: 2,
            framerate_num: 30,
            framerate_denom: 1,
            modifier: None,
        };
        let stride = 12usize; // 2 pixels + 4 bytes of padding per row
        let mut bytes = vec![0xffu8; stride * 2];
        for y in 0..2 {
            for x in 0..2 {
                let at = y * stride + x * 4;
                // B,G,R,A = 0,0,255,255 -> red.
                bytes[at] = 0;
                bytes[at + 1] = 0;
                bytes[at + 2] = 255;
                bytes[at + 3] = 255;
            }
            // Padding bytes stay 0xff and must not be sampled.
        }
        let chunk = SpaChunk {
            offset: 0,
            size: (stride * 2) as u32,
            stride: stride as i32,
            flags: 0,
        };
        let img = read_frame(info, bytes.as_ptr(), bytes.len(), &chunk).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.pixels.len(), 4);
        let red = ((77 * 255) >> 8) as u8;
        assert!(img.pixels.iter().all(|&p| p == red), "{:?}", img.pixels);

        // RGBA: the same bytes are read as blue (B in the last channel),
        // so only the channel order changes.
        let rgba = pod::VideoInfo {
            format: pod::SPA_VIDEO_FORMAT_RGBA,
            ..info
        };
        let img = read_frame(rgba, bytes.as_ptr(), bytes.len(), &chunk).unwrap();
        let blue = ((29 * 255) >> 8) as u8;
        assert!(img.pixels.iter().all(|&p| p == blue));
    }

    /// Regression (2026-10-22 live capture): a REAL fixed Format pod from
    /// KWin. Every value is wrapped in a Choice pod (kind None), the
    /// format choice carries five near-duplicate ids, an unknown
    /// maxFramerate prop is present, and the modifier is INVALID. The
    /// bare-scalar parser rejected this pod (and every real peer pod),
    /// so negotiation never completed.
    #[test]
    fn kwin_format_pod_parses() {
        let hex = "400100000f00000003000400040000000100000000000000140000001300000000000000000000000400000003000000020000000000000002000000000000001400000013000000000000000000000004000000030000000100000000000000030002000000000018000000130000000000000000000000080000000a000000000a0000a0050000040002000000000018000000130000000000000000000000080000000b0000000000000001000000050002000000000028000000130000000000000000000000080000000b0000008f0000000100000000000000010000008f0000000100000001000200000000002400000013000000000000000000000004000000030000000800000008000000080000000c0000000c000000000000000200020000000000180000001300000000000000000000000800000005000000ffffffffffffff00";
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(bytes.len(), 328);
        let info = pod::parse_video_info(&bytes).expect("a real KWin Format pod must parse");
        assert_eq!(info.format, pod::SPA_VIDEO_FORMAT_BGRX);
        assert_eq!((info.width, info.height), (2560, 1440));
        assert_eq!((info.framerate_num, info.framerate_denom), (0, 1));
        assert_eq!(info.modifier, Some(0x00ff_ffff_ffff_ffff));
    }

    /// The Buffers answer is the canonical minimal one: a single
    /// dataType prop carrying the caller's mask verbatim.
    #[test]
    fn param_buffers_is_datatype_only() {
        for mask in [0b0010u32, 0b1010] {
            let p = pod::param_buffers(mask);
            let (size, ty) = pod::read_header(&p).unwrap();
            assert_eq!(ty, pod::SPA_TYPE_OBJECT);
            assert_eq!(size as usize + 8, p.len());
            let body_type = u32::from_le_bytes(p[8..12].try_into().unwrap());
            let body_id = u32::from_le_bytes(p[12..16].try_into().unwrap());
            assert_eq!(body_type, pod::SPA_TYPE_OBJECT_PARAM_BUFFERS);
            assert_eq!(body_id, pod::SPA_PARAM_BUFFERS);
            let (key, vtype, value_off, next) = pod::prop_at(&p, 16).unwrap();
            assert_eq!(key, pod::SPA_PARAM_BUFFERS_DATA_TYPE);
            assert_eq!(vtype, pod::SPA_TYPE_INT);
            let v = i32::from_le_bytes(p[value_off + 8..value_off + 12].try_into().unwrap());
            assert_eq!(v, mask as i32);
            assert_eq!(next, p.len(), "exactly one prop");
        }
    }

    /// The negotiation answer is OBS's four-param set, in order:
    /// VideoCrop meta, Cursor meta, Buffers, Header meta — with the
    /// header-verified meta type ids and struct sizes.
    #[test]
    fn negotiation_params_match_obs_shape() {
        let pods = pod::negotiation_params(true);
        let meta_head = |p: &[u8]| {
            let (size, ty) = pod::read_header(p).unwrap();
            assert_eq!(ty, pod::SPA_TYPE_OBJECT);
            assert_eq!(size as usize + 8, p.len());
            let body_type = u32::from_le_bytes(p[8..12].try_into().unwrap());
            let body_id = u32::from_le_bytes(p[12..16].try_into().unwrap());
            assert_eq!(body_type, pod::SPA_TYPE_OBJECT_PARAM_META);
            assert_eq!(body_id, pod::SPA_PARAM_META);
            let (k1, t1, v1, n1) = pod::prop_at(p, 16).unwrap();
            assert_eq!(k1, pod::SPA_PARAM_META_TYPE);
            assert_eq!(t1, pod::SPA_TYPE_ID);
            let meta_type = u32::from_le_bytes(p[v1 + 8..v1 + 12].try_into().unwrap());
            let (k2, t2, v2, n2) = pod::prop_at(p, n1).unwrap();
            assert_eq!(k2, pod::SPA_PARAM_META_SIZE);
            assert_eq!(n2, p.len(), "exactly two props");
            (meta_type, t2, v2)
        };
        // [0] VideoCrop: bare Int size = sizeof(spa_meta_region) = 16.
        let (mt, t2, v2) = meta_head(&pods[0]);
        assert_eq!(mt, pod::SPA_META_VIDEO_CROP);
        assert_eq!(t2, pod::SPA_TYPE_INT);
        assert_eq!(
            i32::from_le_bytes(pods[0][v2 + 8..v2 + 12].try_into().unwrap()),
            16
        );
        // [1] Cursor: Choice(Range) of Int, OBS's cursor sizes.
        let (mt, t2, v2) = meta_head(&pods[1]);
        assert_eq!(mt, pod::SPA_META_CURSOR);
        assert_eq!(t2, pod::SPA_TYPE_CHOICE);
        let p = &pods[1];
        let kind = u32::from_le_bytes(p[v2 + 8..v2 + 12].try_into().unwrap());
        assert_eq!(kind, pod::SPA_CHOICE_RANGE);
        let vals: Vec<i32> = (0..3)
            .map(|i| i32::from_le_bytes(p[v2 + 24 + 4 * i..v2 + 28 + 4 * i].try_into().unwrap()))
            .collect();
        assert_eq!(vals, vec![16432, 52, 4194352]);
        // [2] Buffers with modifier: MemPtr|DmaBuf.
        let (key, vtype, value_off, _) = pod::prop_at(&pods[2], 16).unwrap();
        assert_eq!(key, pod::SPA_PARAM_BUFFERS_DATA_TYPE);
        assert_eq!(vtype, pod::SPA_TYPE_INT);
        assert_eq!(
            i32::from_le_bytes(pods[2][value_off + 8..value_off + 12].try_into().unwrap()),
            0b1010
        );
        // [3] Header: bare Int size = sizeof(spa_meta_header) = 32.
        let (mt, t2, v2) = meta_head(&pods[3]);
        assert_eq!(mt, pod::SPA_META_HEADER);
        assert_eq!(t2, pod::SPA_TYPE_INT);
        assert_eq!(
            i32::from_le_bytes(pods[3][v2 + 8..v2 + 12].try_into().unwrap()),
            32
        );
        // Without a modifier the mask is MemPtr alone.
        let pods = pod::negotiation_params(false);
        let (_, _, value_off, _) = pod::prop_at(&pods[2], 16).unwrap();
        assert_eq!(
            i32::from_le_bytes(pods[2][value_off + 8..value_off + 12].try_into().unwrap()),
            0b0010
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
    fn portal_session_close_targets_the_standard_session_interface() {
        // org.freedesktop.portal.Session.xml: `Close` is a no-argument
        // method on the session handle returned by CreateSession. The
        // session lives on the connection that created it (item 69b).
        assert_eq!(SESSION_IFACE, "org.freedesktop.portal.Session");
        assert_eq!(SESSION_CLOSE_METHOD, "Close");
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
        // Item 69d removed the Format echo; the 2026-10-22 capture showed
        // the handshake also needs the SPA_PARAM_Buffers answer, which
        // uses the same symbol for the canonical purpose.
        assert!(
            REQUIRED_SYMBOLS.contains(&"pw_stream_update_params"),
            "the Buffers answer completes format negotiation"
        );
        // The real construction sequence: loop -> context -> core (fd) -> stream.
        assert!(REQUIRED_SYMBOLS.contains(&"pw_thread_loop_get_loop"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_new"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_context_connect_fd"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_stream_new"));
        assert!(REQUIRED_SYMBOLS.contains(&"pw_stream_connect"));
    }
}
