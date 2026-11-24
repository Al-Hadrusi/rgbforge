//! Only one RGB Forge may run at a time. Two copies would both drive the same
//! hardware with different settings and the lights would flicker between them.
//!
//! The first copy holds an exclusive lock file and listens on a Unix socket.
//! A second copy finds the lock taken, asks the first to show its window, and exits.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

const SHOW: &[u8] = b"show";

pub struct Guard {
    _lock: File,
    listener: Option<UnixListener>,
}

pub enum Instance {
    /// We're the only copy (or locking isn't possible; then we run anyway).
    First(Option<Guard>),
    /// Another copy is running and has been asked to show itself.
    AlreadyRunning,
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .or_else(|| crate::config::path().and_then(|p| p.parent().map(PathBuf::from)))
}

pub fn acquire() -> Instance {
    let Some(dir) = runtime_dir() else { return Instance::First(None) };
    if fs::create_dir_all(&dir).is_err() {
        return Instance::First(None);
    }
    let sock = dir.join("rgbforge.sock");
    let Ok(lock) = OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("rgbforge.lock")) else {
        return Instance::First(None);
    };
    match lock.try_lock() {
        Ok(()) => {
            // We own the lock, so any socket file left behind is stale.
            let _ = fs::remove_file(&sock);
            let listener = UnixListener::bind(&sock).ok();
            Instance::First(Some(Guard { _lock: lock, listener }))
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            if let Ok(mut s) = UnixStream::connect(&sock) {
                let _ = s.set_write_timeout(Some(Duration::from_secs(1)));
                let _ = s.write_all(SHOW);
            }
            Instance::AlreadyRunning
        }
        Err(std::fs::TryLockError::Error(_)) => Instance::First(None),
    }
}

impl Guard {
    /// Listen for "show yourself" requests from later launches.
    pub fn listen(&mut self, open: Sender<()>, window: crate::tray::WindowSlot) {
        if let Some(listener) = self.listener.take() {
            let _ = thread::Builder::new().name("rgbforge-single".into()).spawn(move || {
                for mut conn in listener.incoming().flatten() {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(1)));
                    let mut buf = [0u8; 8];
                    let n = conn.read(&mut buf).unwrap_or(0);
                    if buf.get(..n) == Some(SHOW) {
                        let _ = open.send(());
                        window.wake();
                    }
                }
            });
        }
    }
}
