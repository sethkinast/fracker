//! KDE StatusNotifierItem tray runtime (seventh pass §1).
//!
//! A resident process owns the tray icon via [`ksni`] (pure-Rust D-Bus,
//! no libdbus C dependency so the offline Flatpak build stays simple).
//! The menu carries at least "Regenerate now", "Open plan page", and
//! "Quit", plus a status line (a disabled entry and the tooltip) showing
//! the last report time and whether the EE.log watch is live.
//!
//! The menu callbacks never block: they push a [`TrayCommand`] onto a
//! channel the resident loop consumes. The status string is shared with
//! the resident through a `Mutex<TrayStatus>`, read when the menu is
//! shown / the tooltip is queried.
//!
//! Only Seth's Plasma session can verify the icon actually appears; this
//! module is structured so everything else (command routing, status
//! formatting, the headless-parity guarantee) is testable on the box.

use std::path::Path;
use std::sync::{Arc, Mutex};

use ksni::menu::StandardItem;
use ksni::{Category, Handle, MenuItem, Status, ToolTip, Tray, TrayMethods};
use tokio::sync::mpsc;

use crate::capture::{CaptureLifecycle, CaptureStage};

/// Verbs the tray menu asks the resident loop to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    /// Force one report run now.
    Regenerate,
    /// Open the current plan page (generate it first when missing).
    OpenPage,
    /// Toggle the Capture Inventory screen-capture flow (eighth pass §2).
    CaptureInventory,
    /// Exit the resident cleanly.
    Quit,
}

/// Live status shown in the tooltip / disabled menu entry. Owned by the
/// resident loop and read by the tray on demand.
#[derive(Debug, Clone, Default)]
pub struct TrayStatus {
    /// The watch thread is running.
    pub watching: bool,
    /// The watch is driven by the inotify directory watch (vs poll).
    pub dir_watch: bool,
    /// Last successful report, formatted for display.
    pub last_report: Option<String>,
    /// Last report failure (cleared by the next success).
    pub last_error: Option<String>,
    /// Resolved EE.log path (for the tooltip).
    pub log_path: Option<String>,
    /// The Capture Inventory lifecycle (ninth pass §1): the explicit state
    /// machine Seth reads from the tray.
    pub capture: CaptureLifecycle,
    /// The loopback-served plan URL (ninth pass §2), recorded when the
    /// embedded HTTP server binds so "Open plan page" opens it directly.
    pub page_url: Option<String>,
}

impl TrayStatus {
    /// True while a capture is in flight (relabels the tray toggle).
    pub fn capturing(&self) -> bool {
        self.capture.stage().is_active()
    }

    /// The one-line human status for tooltip + menu.
    pub fn line(&self) -> String {
        let watch = if self.watching {
            if self.dir_watch {
                "watching EE.log (directory events)"
            } else {
                "watching EE.log (poll fallback)"
            }
        } else {
            "not watching"
        };
        let report = match (&self.last_report, &self.last_error) {
            (_, Some(err)) => format!("last report failed: {err}"),
            (Some(when), None) => format!("last report {when}"),
            (None, None) => "no report yet".to_owned(),
        };
        if matches!(self.capture.stage(), CaptureStage::Idle) {
            format!("{watch} · {report}")
        } else {
            format!("{} · {watch} · {report}", self.capture.status_line())
        }
    }
}

/// The ksni tray item. Holds the command sender and the shared status.
pub struct FrackerTray {
    pub status: Arc<Mutex<TrayStatus>>,
    pub cmds: mpsc::UnboundedSender<TrayCommand>,
}

impl FrackerTray {
    fn status_line(&self) -> String {
        self.status
            .lock()
            .map(|s| s.line())
            .unwrap_or_else(|_| "status unavailable".to_owned())
    }
}

impl Tray for FrackerTray {
    /// Left-click opens the menu (the menu is the whole interface).
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "fracker".to_owned()
    }

    fn title(&self) -> String {
        "Fracker".to_owned()
    }

    fn icon_name(&self) -> String {
        "io.github.sethkinast.Fracker".to_owned()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn status(&self) -> Status {
        Status::Active
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: self.icon_name(),
            title: "Fracker".to_owned(),
            description: self.status_line(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let regen = self.cmds.clone();
        let open = self.cmds.clone();
        let capture = self.cmds.clone();
        let quit = self.cmds.clone();
        let capturing = self
            .status
            .lock()
            .map(|s| s.capturing())
            .unwrap_or(false);
        // One verb, two labels: Capture Inventory ↔ Stop capture.
        let capture_label = if capturing {
            "Stop capture"
        } else {
            "Capture Inventory"
        };
        vec![
            menu_action("Regenerate now", move || {
                let _ = regen.send(TrayCommand::Regenerate);
            }),
            menu_action("Open plan page", move || {
                let _ = open.send(TrayCommand::OpenPage);
            }),
            menu_action(capture_label, move || {
                let _ = capture.send(TrayCommand::CaptureInventory);
            }),
            MenuItem::Separator,
            // Status line: present but not clickable.
            MenuItem::Standard(StandardItem {
                label: self.status_line(),
                enabled: false,
                ..Default::default()
            }),
            // Build identity (item 57): disabled. The crate version is the
            // single source of truth, so the menu can never disagree with
            // what was installed.
            MenuItem::Standard(StandardItem {
                label: version_label(),
                enabled: false,
                ..Default::default()
            }),
            MenuItem::Separator,
            menu_action("Quit", move || {
                let _ = quit.send(TrayCommand::Quit);
            }),
        ]
    }
}

/// The running build label for the tray menu (item 57). The crate version
/// is the source of truth: release merges bump `Cargo.toml` in step with
/// the tag, so this line can never lie about what is installed.
pub fn version_label() -> String {
    format!("Fracker v{}", env!("CARGO_PKG_VERSION"))
}

/// A standard menu entry whose activation pushes a command onto the
/// channel (never blocks the tray service).
fn menu_action<F>(label: &str, action: F) -> MenuItem<FrackerTray>
where
    F: Fn() + Send + 'static,
{
    MenuItem::Standard(StandardItem {
        label: label.to_owned(),
        activate: Box::new(move |_tray: &mut FrackerTray| action()),
        ..Default::default()
    })
}

/// Spawn the tray service. `FLATPAK_ID` / `/.flatpak-info` marks a
/// sandbox, where owning the well-known bus name is forbidden (ksni
/// documents this); `assume_sni_available` keeps the item alive across a
/// shell restart instead of failing at startup.
pub async fn spawn(
    status: Arc<Mutex<TrayStatus>>,
    cmds: mpsc::UnboundedSender<TrayCommand>,
) -> Result<Handle<FrackerTray>, String> {
    let sandboxed =
        std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists();
    let tray = FrackerTray { status, cmds };
    tray.disable_dbus_name(sandboxed)
        .assume_sni_available(true)
        .spawn()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Labels of a rendered menu, in order ("---" for separators).
    fn labels(tray: &FrackerTray) -> Vec<(String, bool)> {
        tray.menu()
            .into_iter()
            .map(|item| match item {
                MenuItem::Standard(s) => (s.label, s.enabled),
                MenuItem::Separator => ("---".to_owned(), false),
                _ => ("?".to_owned(), false),
            })
            .collect()
    }

    #[test]
    fn menu_carries_the_required_verbs_and_a_status_line() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = FrackerTray {
            status: Arc::new(Mutex::new(TrayStatus {
                watching: true,
                dir_watch: true,
                ..Default::default()
            })),
            cmds: tx,
        };
        let items = labels(&tray);
        let names: Vec<&str> = items.iter().map(|(l, _)| l.as_str()).collect();
        assert!(names.contains(&"Regenerate now"));
        assert!(names.contains(&"Open plan page"));
        assert!(names.contains(&"Capture Inventory"), "the §2 toggle is present");
        assert!(names.contains(&"Quit"));
        // The status line is a present-but-disabled entry.
        assert!(items
            .iter()
            .any(|(l, enabled)| l.contains("watching EE.log") && !*enabled));
    }

    /// Item 57: the build identity is visible and non-interactive, and it
    /// reads the crate version rather than a hand-copied constant.
    #[test]
    fn menu_shows_the_crate_version_as_a_disabled_item() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = FrackerTray {
            status: Arc::new(Mutex::new(TrayStatus::default())),
            cmds: tx,
        };
        let items = labels(&tray);
        let version = version_label();
        assert_eq!(version, format!("Fracker v{}", env!("CARGO_PKG_VERSION")));
        assert!(
            version.ends_with(env!("CARGO_PKG_VERSION")),
            "the menu reads the crate version: {version}"
        );
        assert!(
            items.iter().any(|(l, enabled)| *l == version && !*enabled),
            "version line is present and disabled: {items:?}"
        );
        // It is not one of the clickable verbs.
        assert!(!items
            .iter()
            .any(|(l, enabled)| *enabled && *l == version));
    }

    #[test]
    fn menu_activation_pushes_the_matching_command() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tray = FrackerTray {
            status: Arc::new(Mutex::new(TrayStatus::default())),
            cmds: tx,
        };
        // Fire each verb, in order.
        for item in tray.menu() {
            if let MenuItem::Standard(s) = item {
                (s.activate)(&mut tray);
            }
        }
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::Regenerate);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::OpenPage);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::CaptureInventory);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::Quit);
        assert!(rx.try_recv().is_err(), "only the four verbs fired");
    }

    #[test]
    fn capture_toggle_relabels_and_surfaces_live_counts() {
        // While capturing the same verb reads "Stop capture" and the
        // status line shows the live per-bucket counts.
        let mut lc = CaptureLifecycle::new();
        lc.set_stage(CaptureStage::Capturing);
        lc.set_counts(crate::capture::CaptureCounts {
            relic_stacks: 512,
            parts: 0,
            arcanes: 12,
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut tray = FrackerTray {
            status: Arc::new(Mutex::new(TrayStatus {
                watching: true,
                dir_watch: true,
                capture: lc,
                ..Default::default()
            })),
            cmds: tx,
        };
        let names: Vec<String> = labels(&tray).into_iter().map(|(l, _)| l).collect();
        assert!(names.contains(&"Stop capture".to_owned()));
        assert!(!names.contains(&"Capture Inventory".to_owned()));
        assert!(tray.status_line().contains("Capture: capturing — 512 relic stacks"));
        // Firing it still sends the single toggle verb.
        for item in tray.menu() {
            if let MenuItem::Standard(s) = item {
                (s.activate)(&mut tray);
            }
        }
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::Regenerate);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::OpenPage);
        assert_eq!(rx.try_recv().unwrap(), TrayCommand::CaptureInventory);
    }

    #[test]
    fn capture_error_stage_is_visible_in_the_tray_line() {
        // Ninth pass §1: a failed capture names its stage in the tray, not
        // just the resident log.
        let mut lc = CaptureLifecycle::new();
        lc.fail("connecting", "PipeWire: no first frame within 15s");
        let (tx, _rx) = mpsc::unbounded_channel();
        let tray = FrackerTray {
            status: Arc::new(Mutex::new(TrayStatus {
                watching: true,
                dir_watch: true,
                capture: lc,
                ..Default::default()
            })),
            cmds: tx,
        };
        let line = tray.status_line();
        assert!(
            line.contains("error (connecting)") && line.contains("no first frame"),
            "capture failure must be visible: {line}"
        );
        // The toggle is back to "Capture Inventory" (not active).
        let names: Vec<String> = labels(&tray).into_iter().map(|(l, _)| l).collect();
        assert!(names.contains(&"Capture Inventory".to_owned()));
    }

    #[test]
    fn status_line_surfaces_state_and_failures() {
        assert_eq!(TrayStatus::default().line(), "not watching · no report yet");
        let ok = TrayStatus {
            watching: true,
            dir_watch: true,
            last_report: Some("12:34:56".to_owned()),
            last_error: None,
            log_path: Some("/x/EE.log".to_owned()),
            ..Default::default()
        };
        assert_eq!(ok.line(), "watching EE.log (directory events) · last report 12:34:56");
        let poll = TrayStatus {
            watching: true,
            dir_watch: false,
            ..ok.clone()
        };
        assert!(poll.line().contains("poll fallback"));
        let bad = TrayStatus {
            last_error: Some("network down".to_owned()),
            ..ok
        };
        assert_eq!(
            bad.line(),
            "watching EE.log (directory events) · last report failed: network down"
        );
    }
}
