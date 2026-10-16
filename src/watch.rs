//! Live EE.log watch (BRIEF §5/§7, addendum #35).
//!
//! EE.log is UNLINKED and recreated every session (addendum #35): the
//! final lines of a session (the mission-end picks!) can be lost when
//! the game replaces the file. So this module:
//!
//! - watches the containing DIRECTORY for file events (inotify) —
//!   never a naive path tail;
//! - on unlink/recreate DRAINS the old inode to EOF through its
//!   still-open FD before opening the new file;
//! - persists `(inode, offset)` so startup reconciles by processing any
//!   unprocessed tail;
//! - keeps a polling fallback that ALSO carries inode-change detection.
//!
//! The event consumer it feeds (`elog`) is implemented and unit-tested
//! against synthetic lines; the runtime glue here is exercised with
//! synthetic log files (see the tests).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// Persisted tail position: `(inode, byte offset)`. Stored in the app
/// state dir (`AppState.log_position`) so a restart resumes without
/// reprocessing a prefix or dropping an unprocessed tail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPosition {
    #[serde(default)]
    pub inode: u64,
    #[serde(default)]
    pub offset: u64,
}

// ---------------------------------------------------------------------------
// Directory watch (inotify)
// ---------------------------------------------------------------------------

/// A watch on the EE.log CONTAINING DIRECTORY (addendum #35a: the folder
/// grant is primary). Kernel events wake the tail so a file recreation
/// is seen promptly; failures degrade to the polling fallback.
pub struct DirWatch {
    fd: i32,
}

impl DirWatch {
    /// Best-effort watch. `None` when inotify is unavailable — callers
    /// then rely on the polling fallback.
    pub fn new(dir: &Path) -> Option<Self> {
        use std::ffi::CString;
        let c = CString::new(dir.as_os_str().as_bytes()).ok()?;
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let mask = libc::IN_MODIFY
            | libc::IN_ATTRIB
            | libc::IN_CLOSE_WRITE
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_FROM
            | libc::IN_MOVED_TO
            | libc::IN_DELETE_SELF
            | libc::IN_MOVE_SELF;
        let wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), mask) };
        if wd < 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(Self { fd })
    }

    /// Drain queued events, returning the file names touched (as the
    /// kernel reports them). Non-blocking; empty when nothing happened.
    pub fn drain(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe {
                libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n <= 0 {
                break;
            }
            let n = n as usize;
            let mut off = 0usize;
            while off + std::mem::size_of::<libc::inotify_event>() <= n {
                let ev = unsafe {
                    &*(buf[off..].as_ptr() as *const libc::inotify_event)
                };
                let name_len = ev.len as usize;
                let name_start = off + std::mem::size_of::<libc::inotify_event>();
                if name_len > 0 && name_start + name_len <= buf.len() {
                    let raw = &buf[name_start..name_start + name_len];
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    if end > 0 {
                        names.push(String::from_utf8_lossy(&raw[..end]).into_owned());
                    }
                }
                off = name_start + name_len;
            }
        }
        names
    }

    /// Block up to `timeout_ms` for at least one event, then return the
    /// names available. The timeout doubles as the polling fallback tick.
    pub fn wait(&self, timeout_ms: i32) -> Vec<String> {
        let mut pfd = libc::pollfd {
            fd: self.fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let r = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if r > 0 && (pfd.revents & libc::POLLIN) != 0 {
            self.drain()
        } else {
            Vec::new()
        }
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

// ---------------------------------------------------------------------------
// Rotating tail
// ---------------------------------------------------------------------------

/// Offset-tracked reader that survives truncation AND unlink/recreate
/// (addendum #35b). Holds the open FD for the inode it is tracking so a
/// rotation can drain the old file to EOF before switching.
pub struct RotatingTail {
    dir: PathBuf,
    file_name: String,
    inode: u64,
    offset: u64,
    reader: Option<File>,
    /// Bytes after the last newline; held until the line ends (or until
    /// the old inode is drained at rotation).
    pending: String,
}

impl RotatingTail {
    pub fn new(dir: &Path, file_name: &str) -> Self {
        Self {
            dir: dir.to_path_buf(),
            file_name: file_name.to_owned(),
            inode: 0,
            offset: 0,
            reader: None,
            pending: String::new(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(&self.file_name)
    }

    pub fn position(&self) -> LogPosition {
        LogPosition {
            inode: self.inode,
            offset: self.offset,
        }
    }

    /// Reconcile on startup: when the file still has the persisted
    /// inode, reopen it and resume at the persisted offset (clamped to
    /// EOF). A different inode means the file rotated while we were
    /// down — start fresh on the new inode. Missing file is fine (the
    /// directory watch will see it appear).
    pub fn resume(&mut self, pos: LogPosition) -> Result<(), String> {
        if pos.inode == 0 {
            return Ok(());
        }
        let path = self.path();
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if meta.ino() == pos.inode {
            let f = File::open(&path)
                .map_err(|e| format!("tail {}: {e}", path.display()))?;
            self.reader = Some(f);
            self.inode = pos.inode;
            self.offset = pos.offset.min(meta.len());
        }
        Ok(())
    }

    /// Read whatever is new, following rotation and truncation. Returns
    /// the complete lines; a trailing partial line is held until it
    /// ends, except when draining an old inode (rotation/unlink), where
    /// it is flushed — that tail is the session's final write.
    pub fn poll(&mut self) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();
        let path = self.path();
        let meta = std::fs::metadata(&path).ok();
        match meta {
            Some(m) if m.ino() == self.inode && self.reader.is_some() => {
                if m.len() < self.offset {
                    // Truncated in place: restart at 0 on the same inode.
                    self.offset = 0;
                }
                self.read_to_eof(&mut lines, false);
            }
            Some(m) => {
                // Rotation (or first sight): drain the old inode to EOF
                // through its still-open FD, then open the new file.
                if self.reader.is_some() {
                    self.read_to_eof(&mut lines, true);
                }
                if let Ok(f) = File::open(&path) {
                    self.reader = Some(f);
                    self.inode = m.ino();
                    self.offset = 0;
                    self.pending.clear();
                    self.read_to_eof(&mut lines, false);
                }
            }
            None => {
                // Unlinked (recreation pending): drain what the old FD
                // can still see, then keep it for the next poll.
                if self.reader.is_some() {
                    self.read_to_eof(&mut lines, true);
                }
            }
        }
        Ok(lines)
    }

    /// Read from the current offset to EOF, appending lines.
    fn read_to_eof(&mut self, lines: &mut Vec<String>, flush_partial: bool) {
        let Some(reader) = self.reader.as_mut() else {
            return;
        };
        if reader.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if reader.read_to_end(&mut buf).is_err() {
            return;
        }
        self.offset += buf.len() as u64;
        self.pending.push_str(&String::from_utf8_lossy(&buf));
        while let Some(i) = self.pending.find('\n') {
            let line: String = self.pending.drain(..=i).collect();
            lines.push(line.trim_end_matches(['\r', '\n']).to_owned());
        }
        if flush_partial && !self.pending.is_empty() {
            lines.push(self.pending.trim_end_matches('\r').to_owned());
            self.pending.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// Watcher driver + tray regeneration signal
// ---------------------------------------------------------------------------

/// Primary watch driver: directory events wake the tail; the wait
/// timeout doubles as the poll fallback. Both paths carry inode-change
/// detection inside [`RotatingTail::poll`].
pub struct LogWatcher {
    pub tail: RotatingTail,
    dir: Option<DirWatch>,
}

impl LogWatcher {
    /// Open a watcher for `log_path`'s containing directory.
    pub fn open(log_path: &Path) -> Result<Self, String> {
        let dir = log_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| format!("watch: {} has no containing directory", log_path.display()))?;
        let name = log_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("watch: bad log file name: {}", log_path.display()))?;
        Ok(Self {
            tail: RotatingTail::new(dir, name),
            dir: DirWatch::new(dir),
        })
    }

    pub fn resume(&mut self, pos: LogPosition) -> Result<(), String> {
        self.tail.resume(pos)
    }

    pub fn position(&self) -> LogPosition {
        self.tail.position()
    }

    /// True when the directory watch (rather than the poll fallback) is
    /// driving wakeups.
    pub fn watching_dir(&self) -> bool {
        self.dir.is_some()
    }

    /// Wait up to `timeout_ms` for activity, then return any new lines.
    pub fn next_lines(&mut self, timeout_ms: i32) -> Result<Vec<String>, String> {
        match &self.dir {
            Some(w) => {
                let _ = w.wait(timeout_ms);
            }
            None => std::thread::sleep(std::time::Duration::from_millis(
                timeout_ms.max(0) as u64,
            )),
        }
        self.tail.poll()
    }
}

/// Coalesced regeneration signal. The tray's manual Regenerate verb
/// calls [`RegenSignal::request`]; the watch loop consumes it with
/// [`RegenSignal::take`] and forces one page regeneration. Mission-end
/// events request it automatically.
#[derive(Debug, Clone, Default)]
pub struct RegenSignal {
    flag: Arc<AtomicBool>,
}

impl RegenSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Consume a pending request (returns true at most once per request).
    pub fn take(&self) -> bool {
        self.flag.swap(false, Ordering::SeqCst)
    }

    pub fn is_pending(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fracker-watch-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_appended_lines_then_holds_partial() {
        let dir = tmpdir("append");
        let path = dir.join("EE.log");
        let mut tail = RotatingTail::new(&dir, "EE.log");
        assert!(tail.poll().unwrap().is_empty(), "file does not exist yet");

        {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"a\nb\npart").unwrap();
        }
        assert_eq!(tail.poll().unwrap(), vec!["a".to_owned(), "b".to_owned()]);
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"ial\nd\n").unwrap();
        }
        assert_eq!(tail.poll().unwrap(), vec!["partial".to_owned(), "d".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drains_old_inode_to_eof_on_rotation() {
        // Addendum #35b proof: the session's final writes land on the
        // OLD inode after unlink. Holding the FD lets the tail read them
        // before switching to the recreated file — no lost mission-end
        // picks. (The writer FD is the game's.)
        let dir = tmpdir("rotate");
        let path = dir.join("EE.log");
        let mut writer = {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"start\n").unwrap();
            f
        };
        let mut tail = RotatingTail::new(&dir, "EE.log");
        assert_eq!(tail.poll().unwrap(), vec!["start".to_owned()]);
        let pos_before = tail.position();
        assert!(pos_before.inode != 0);

        // The game writes its final lines, unlinks, writes once more
        // through the still-open FD, then creates the new session file.
        writer.write_all(b"mission-end picks\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        writer.write_all(b"last gasp\n").unwrap();
        let mut new = File::create(&path).unwrap();
        new.write_all(b"new session\n").unwrap();
        new.flush().unwrap();

        let lines = tail.poll().unwrap();
        assert_eq!(
            lines,
            vec![
                "mission-end picks".to_owned(),
                "last gasp".to_owned(),
                "new session".to_owned(),
            ],
            "old inode drained to EOF before the new file is read"
        );
        assert_ne!(tail.position().inode, pos_before.inode, "now on the new inode");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unlinked_file_drains_then_waits_for_recreation() {
        // If the file disappears before the new one lands, the drain
        // still yields the tail; the next poll picks up the recreated
        // file (inode change detected).
        let dir = tmpdir("unlink");
        let path = dir.join("EE.log");
        let mut writer = {
            let mut f = File::create(&path).unwrap();
            f.write_all(b"one\n").unwrap();
            f
        };
        let mut tail = RotatingTail::new(&dir, "EE.log");
        assert_eq!(tail.poll().unwrap(), vec!["one".to_owned()]);
        writer.write_all(b"two").unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            tail.poll().unwrap(),
            vec!["two".to_owned()],
            "partial final line flushed when the inode is unlinked"
        );
        assert!(tail.poll().unwrap().is_empty(), "nothing left on the old inode");
        std::fs::write(&path, b"three\n").unwrap();
        assert_eq!(tail.poll().unwrap(), vec!["three".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn truncation_in_place_restarts_at_zero() {
        let dir = tmpdir("truncate");
        let path = dir.join("EE.log");
        std::fs::write(&path, b"old-a\nold-b\n").unwrap();
        let mut tail = RotatingTail::new(&dir, "EE.log");
        assert_eq!(tail.poll().unwrap(), vec!["old-a".to_owned(), "old-b".to_owned()]);
        // Same inode, truncated and rewritten shorter.
        std::fs::write(&path, b"new\n").unwrap();
        assert_eq!(tail.poll().unwrap(), vec!["new".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_from_persisted_position_skips_prefix() {
        let dir = tmpdir("resume");
        let path = dir.join("EE.log");
        std::fs::write(&path, b"line-1\nline-2\n").unwrap();
        let mut first = RotatingTail::new(&dir, "EE.log");
        assert_eq!(first.poll().unwrap().len(), 2);
        let saved = first.position();
        // Restart: resume from the persisted (inode, offset).
        let mut second = RotatingTail::new(&dir, "EE.log");
        second.resume(saved).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"line-3\n")
            .unwrap();
        assert_eq!(second.poll().unwrap(), vec!["line-3".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_reconciles_an_unprocessed_tail() {
        // Persisted offset behind EOF (crash before the tail was read):
        // startup resumes and processes the unread tail.
        let dir = tmpdir("reconcile");
        let path = dir.join("EE.log");
        std::fs::write(&path, b"a\nb\nc\n").unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let mut tail = RotatingTail::new(&dir, "EE.log");
        tail.resume(LogPosition {
            inode: meta.ino(),
            offset: 2, // only "a\n" was processed
        })
        .unwrap();
        assert_eq!(tail.poll().unwrap(), vec!["b".to_owned(), "c".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_watch_sees_events_and_watcher_falls_back() {
        let dir = tmpdir("dirwatch");
        let path = dir.join("EE.log");
        // Either inotify is available (events name the file) or the
        // watcher runs in poll-fallback mode — both must surface lines.
        let mut watcher = LogWatcher::open(&path).unwrap();
        std::fs::write(&path, b"hello\n").unwrap();
        let mut lines = Vec::new();
        for _ in 0..20 {
            lines.extend(watcher.next_lines(50).unwrap());
            if !lines.is_empty() {
                break;
            }
        }
        assert_eq!(lines, vec!["hello".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn regen_signal_consumes_once() {
        let s = RegenSignal::new();
        assert!(!s.take());
        s.request();
        assert!(s.is_pending());
        assert!(s.take());
        assert!(!s.take());
    }
}
