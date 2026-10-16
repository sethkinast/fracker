//! Single-instance guard (seventh pass §1).
//!
//! A resident Fracker must not double-watch the EE.log when launched
//! twice (KDE autostart + a manual launch, say). This takes an exclusive
//! `flock` on a lock file in the app data dir; a second launch sees the
//! lock held and exits cleanly instead of starting a second watcher.
//!
//! `flock` (not a PID file) is the mechanism: the kernel releases it
//! automatically when the process dies for any reason, so a crash never
//! leaves a stale "already running" state.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

/// Why acquiring the single-instance lock failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstanceError {
    /// Another Fracker already owns the lock.
    AlreadyRunning,
    /// The lock file could not be created/opened/locked.
    Io(String),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceError::AlreadyRunning => write!(f, "another Fracker is already running"),
            InstanceError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Holds the exclusive lock for the process lifetime; dropping it (or
/// process exit) releases the lock.
#[derive(Debug)]
pub struct InstanceGuard {
    file: File,
    path: PathBuf,
}

impl InstanceGuard {
    /// Take the exclusive lock at `path`. Returns
    /// [`InstanceError::AlreadyRunning`] when another process holds it.
    pub fn acquire(path: &Path) -> Result<Self, InstanceError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    InstanceError::Io(format!("mkdir {}: {e}", parent.display()))
                })?;
            }
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| InstanceError::Io(format!("open {}: {e}", path.display())))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            let would_block = err.raw_os_error() == Some(libc::EWOULDBLOCK)
                || err.raw_os_error() == Some(libc::EAGAIN);
            if would_block {
                return Err(InstanceError::AlreadyRunning);
            }
            return Err(InstanceError::Io(format!("flock {}: {err}", path.display())));
        }
        // Record the owning pid for diagnostics (best-effort).
        let _ = file.set_len(0);
        let _ = writeln!(file, "{}", std::process::id());
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Best-effort explicit unlock; closing the FD would also release.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_path(tag: &str) -> PathBuf {
        std::env::temp_dir()
            .join("fracker-instance-tests")
            .join(format!("{tag}-{}.lock", std::process::id()))
    }

    #[test]
    fn second_acquire_reports_already_running_then_recovers() {
        let path = lock_path("dup");
        let _ = std::fs::remove_file(&path);
        let first = InstanceGuard::acquire(&path).expect("first lock");
        assert_eq!(
            InstanceGuard::acquire(&path).unwrap_err(),
            InstanceError::AlreadyRunning
        );
        // Releasing the first frees the lock for a later launch.
        drop(first);
        let third = InstanceGuard::acquire(&path).expect("relock after drop");
        drop(third);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lock_file_records_owning_pid() {
        let path = lock_path("pid");
        let _ = std::fs::remove_file(&path);
        let guard = InstanceGuard::acquire(&path).expect("lock");
        let text = std::fs::read_to_string(guard.path()).unwrap();
        assert_eq!(text.trim(), std::process::id().to_string());
        drop(guard);
        let _ = std::fs::remove_file(&path);
    }
}
