//! `rgbforge --helper`: the privileged hardware service (runs as a sandboxed systemd service).
//!
//! It is the only process with access to the lighting hardware. It owns every device
//! driver and accepts only the narrow requests in [`crate::proto`], so no other program
//! on the PC gets raw access to the SMBus, the GPU's RGB bus or USB lighting controllers.

use std::collections::HashMap;
use std::fs;
use std::io::{self, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crate::drivers::{self, DemoMode, Driver};
use crate::proto::{self, Check, RemoteDevice, Request, Response};

/// At most this many simultaneous client connections.
const MAX_CLIENTS: usize = 8;
/// Idle connections are closed after this long, so idle or stuck clients can't hold the
/// connection slots forever (the app reconnects automatically when it needs to).
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A client that stops reading replies can't block its connection thread forever.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Per-connection request budget: sustained rate and burst.
const REQUESTS_PER_SEC: f32 = 240.0;
const REQUEST_BURST: f32 = 480.0;
/// Hardware scans and checks probe buses; allow at most one per this interval (all clients).
const SCAN_COOLDOWN: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    drivers: Vec<Box<dyn Driver>>,
    last_write: HashMap<String, Instant>,
    last_scan: Option<Instant>,
}

impl State {
    /// Global cooldown for requests that probe hardware (Detect, Check).
    fn scan_allowed(&mut self) -> bool {
        if self.last_scan.is_some_and(|t| t.elapsed() < SCAN_COOLDOWN) {
            return false;
        }
        self.last_scan = Some(Instant::now());
        true
    }
}

/// Token bucket limiting how fast one connection may send requests.
struct Budget {
    tokens: f32,
    last: Instant,
}

impl Budget {
    fn new() -> Self {
        Self { tokens: REQUEST_BURST, last: Instant::now() }
    }

    /// Wait (throttle) until a request may proceed.
    fn take(&mut self) {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.last).as_secs_f32() * REQUESTS_PER_SEC).min(REQUEST_BURST);
        self.last = now;
        if self.tokens < 1.0 {
            thread::sleep(Duration::from_secs_f32((1.0 - self.tokens) / REQUESTS_PER_SEC));
            self.tokens = 1.0;
            self.last = Instant::now();
        }
        self.tokens -= 1.0;
    }
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn run() -> ExitCode {
    let path = Path::new(proto::SOCKET);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::remove_file(path); // stale socket from a previous run
    let listener = match UnixListener::bind(path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("rgbforge-helper: can't listen on {}: {e}", proto::SOCKET);
            return ExitCode::FAILURE;
        }
    };
    // Only root and members of the service's group (`rgbforge`, set with Group= in the systemd
    // unit; the installer adds the desktop user to it) may connect. Other accounts on the PC
    // can't reach the service at all.
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o660)) {
        eprintln!("rgbforge-helper: can't set socket permissions: {e}");
        return ExitCode::FAILURE;
    }
    eprintln!("rgbforge-helper: listening on {}", proto::SOCKET);

    let state = Arc::new(Mutex::new(State::default()));
    let clients = Arc::new(AtomicUsize::new(0));
    for conn in listener.incoming().flatten() {
        if clients.load(Ordering::SeqCst) >= MAX_CLIENTS {
            continue; // dropping the connection closes it
        }
        clients.fetch_add(1, Ordering::SeqCst);
        let (st, cl) = (Arc::clone(&state), Arc::clone(&clients));
        let spawned = thread::Builder::new().name("rgbforge-client".into()).spawn(move || {
            let _ = serve(conn, &st);
            cl.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            // Couldn't start a thread for this client: give back the slot taken above.
            clients.fetch_sub(1, Ordering::SeqCst);
        }
    }
    ExitCode::SUCCESS
}

fn serve(stream: UnixStream, state: &Mutex<State>) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut budget = Budget::new();
    loop {
        let response = match proto::read_msg::<Request, _>(&mut reader) {
            Ok(req) => {
                budget.take();
                handle(req, state)
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            // Idle for too long: close quietly.
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(()),
            Err(e) => {
                // Malformed input: report once, then drop the connection.
                let _ = proto::write_msg(&mut writer, &Response::Error(format!("bad request: {e}")));
                return Ok(());
            }
        };
        proto::write_msg(&mut writer, &response)?;
    }
}

fn handle(req: Request, state: &Mutex<State>) -> Response {
    match req {
        Request::Hello { .. } => Response::Hello { version: proto::VERSION },
        Request::Detect { gpu, ram } => {
            let mut s = lock(state);
            if !s.scan_allowed() {
                return Response::Error("Too many device scans; wait a moment and try again.".into());
            }
            s.drivers.clear(); // drop old drivers first (they hand devices back)
            s.last_write.clear();
            let found = drivers::detect(DemoMode::Never, gpu, ram);
            let devices = found
                .drivers
                .iter()
                .map(|d| RemoteDevice { info: d.info().clone(), min_interval: d.min_interval_secs(), keepalive: d.keepalive_secs() })
                .collect();
            s.drivers = found.drivers;
            Response::Devices { devices, problems: found.problems }
        }
        Request::Resize { id, zone, leds } => {
            let mut s = lock(state);
            match s.drivers.iter_mut().find(|d| d.info().id == id) {
                Some(d) => {
                    d.resize_zone(zone, leds);
                    Response::Ok
                }
                None => Response::Error("unknown device".into()),
            }
        }
        Request::Apply { id, frame } => {
            let mut guard = lock(state);
            let State { drivers, last_write, .. } = &mut *guard;
            let Some(d) = drivers.iter_mut().find(|d| d.info().id == id) else {
                return Response::Error("unknown device".into());
            };
            // Enforce each device's rate limit here too, whatever the client does.
            if last_write.get(&id).is_some_and(|t| t.elapsed().as_secs_f32() < d.min_interval_secs()) {
                return Response::Ok;
            }
            let frame = proto::fit_frame(d.info(), frame);
            match d.apply(&frame) {
                Ok(()) => {
                    last_write.insert(id, Instant::now());
                    Response::Ok
                }
                Err(e) => Response::Error(e.to_string()),
            }
        }
        Request::Check(what) => {
            // Don't probe the bus while drivers are mid-write, and not too often.
            let mut s = lock(state);
            if !s.scan_allowed() {
                return Response::Error("Too many checks; wait a moment and try again.".into());
            }
            Response::Text(match what {
                Check::Gpu => drivers::sapphire::check_report(),
                Check::Ram => drivers::ene_ram::check_report(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_are_rate_limited() {
        let mut s = State::default();
        assert!(s.scan_allowed());
        assert!(!s.scan_allowed(), "a second scan right away must be refused");
    }

    #[test]
    fn request_budget_throttles_floods() {
        let mut b = Budget::new();
        let start = Instant::now();
        for _ in 0..(REQUEST_BURST as usize + 24) {
            b.take();
        }
        // The burst is free; the 24 extra requests must wait about 24/240 s = 100 ms.
        assert!(start.elapsed() >= Duration::from_millis(80), "{:?}", start.elapsed());
    }
}
