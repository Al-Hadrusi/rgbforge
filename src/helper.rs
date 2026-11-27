//! `rgbforge --helper`: the privileged hardware service (runs as a sandboxed systemd service).
//!
//! It is the only process with access to the lighting hardware. It owns every device
//! driver and accepts only the narrow requests in [`crate::proto`], so no other program
//! on the PC gets raw access to the SMBus, the GPU's RGB bus or USB lighting controllers.

use std::collections::{HashMap, HashSet};
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

/// Who is on the other end of a connection, as reported by the kernel (SO_PEERCRED).
/// This can't be forged by the client, unlike anything it sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Peer {
    uid: u32,
    pid: i32,
}

fn peer_of(stream: &UnixStream) -> io::Result<Peer> {
    let cred = rustix::net::sockopt::socket_peercred(stream)?;
    Ok(Peer { uid: cred.uid.as_raw(), pid: cred.pid.as_raw_nonzero().get() })
}

/// Accounts allowed to use the service: root and the `rgbforge` group. The socket's file
/// mode already enforces this; checking the kernel-reported identity as well means a
/// mis-set mode, a stale group membership or a future packaging mistake can't widen access.
fn trusted_now() -> Vec<u32> {
    let group = fs::read_to_string("/etc/group").unwrap_or_default();
    let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
    drivers::trusted_uids(&group, &passwd)
}

fn authorized(peer: Peer, trusted: &[u32]) -> bool {
    trusted.contains(&peer.uid)
}

/// Client-supplied text made safe for a log line: no control characters (no forged log
/// lines), only a conservative character set, and bounded length.
fn log_safe(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-' | '/'))
        .take(80)
        .collect()
}

/// One audit line on stderr (the systemd journal: `journalctl -u rgbforge-helper`).
/// Records who did a security-relevant thing. Never colours, never message bodies.
fn audit_line(peer: Peer, event: &str, detail: &str) -> String {
    format!("rgbforge-helper audit: event={event} uid={} pid={} {detail}", peer.uid, peer.pid)
}

fn audit(peer: Peer, event: &str, detail: &str) {
    eprintln!("{}", audit_line(peer, event, detail).trim_end());
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
        let Ok(peer) = peer_of(&conn) else { continue };
        let trusted = trusted_now();
        let (st, cl) = (Arc::clone(&state), Arc::clone(&clients));
        let spawned = thread::Builder::new().name("rgbforge-client".into()).spawn(move || {
            let _ = serve(conn, &st, peer, &trusted);
            cl.fetch_sub(1, Ordering::SeqCst);
        });
        if spawned.is_err() {
            // Couldn't start a thread for this client: give back the slot taken above.
            clients.fetch_sub(1, Ordering::SeqCst);
        }
    }
    ExitCode::SUCCESS
}

fn serve(stream: UnixStream, state: &Mutex<State>, peer: Peer, trusted: &[u32]) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut writer = stream.try_clone()?;
    if !authorized(peer, trusted) {
        // Refuse before reading anything: no request from this account is ever handled.
        audit(peer, "connection-refused", "reason=not-in-rgbforge-group");
        let _ = proto::write_msg(&mut writer, &Response::Error(crate::remote::NOT_IN_GROUP.into()));
        return Ok(());
    }
    let mut refused_logged = HashSet::new();
    let mut reader = BufReader::new(stream);
    let mut budget = Budget::new();
    loop {
        let response = match proto::read_msg::<Request, _>(&mut reader) {
            Ok(req) => {
                budget.take();
                handle(req, state, peer, &mut refused_logged)
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

fn handle(req: Request, state: &Mutex<State>, peer: Peer, refused_logged: &mut HashSet<String>) -> Response {
    match req {
        Request::Hello { .. } => Response::Hello { version: proto::VERSION },
        Request::Detect { gpu, ram } => {
            let mut s = lock(state);
            if !s.scan_allowed() {
                return Response::Error("Too many device scans; wait a moment and try again.".into());
            }
            // Enabling RAM or graphics-card control is the highest-risk action: record who did it.
            let allow = crate::policy::load();
            audit(peer, "detect", &format!("gpu={gpu} ram={ram} admin-allows={}", allow.text().replace(' ', ",")));
            s.drivers.clear(); // drop old drivers first (they hand devices back)
            s.last_write.clear();
            let found = drivers::detect_with(DemoMode::Never, gpu, ram, allow);
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
                    audit(peer, "resize", &format!("device={} zone={zone} leds={leds}", log_safe(&id)));
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
                Err(e) => {
                    // A write the driver's allow-list blocked: log it once per device per connection
                    // (frames arrive many times a second; don't flood the journal).
                    if e.kind() == io::ErrorKind::PermissionDenied && refused_logged.insert(id.clone()) {
                        audit(peer, "write-blocked", &format!("device={}", log_safe(&id)));
                    }
                    Response::Error(e.to_string())
                }
            }
        }
        Request::Check(what) => {
            // Don't probe the bus while drivers are mid-write, and not too often.
            let mut s = lock(state);
            if !s.scan_allowed() {
                return Response::Error("Too many checks; wait a moment and try again.".into());
            }
            audit(peer, "check", &format!("what={what:?}"));
            Response::Text(match what {
                Check::Gpu => drivers::sapphire::check_report(),
                Check::Ram => drivers::ene_ram::check_report(),
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn demo_state() -> Arc<Mutex<State>> {
        Arc::new(Mutex::new(State { drivers: drivers::demo::devices(), ..State::default() }))
    }

    fn fan_leds(state: &Mutex<State>) -> u16 {
        let s = lock(state);
        let d = s.drivers.iter().find(|d| d.info().id == "demo:fans").unwrap();
        d.info().zones[0].leds
    }

    /// Run the real `serve` on one end of a socket pair, send `req`, return the reply.
    fn exchange(state: &Arc<Mutex<State>>, trusted: Vec<u32>, req: &Request) -> Response {
        let (client, server) = UnixStream::pair().unwrap();
        let peer = peer_of(&server).unwrap();
        let st = Arc::clone(state);
        let t = thread::spawn(move || serve(server, &st, peer, &trusted));
        let mut w = client.try_clone().unwrap();
        // A refused peer may have closed already; the reply below is what matters.
        let _ = proto::write_msg(&mut w, req);
        let reply = proto::read_msg::<Response, _>(&mut BufReader::new(client)).unwrap();
        drop(w);
        let _ = t.join();
        reply
    }

    #[test]
    fn peer_identity_comes_from_the_kernel() {
        let (a, b) = UnixStream::pair().unwrap();
        let p = peer_of(&b).unwrap();
        assert_eq!(p.pid, i32::try_from(std::process::id()).unwrap());
        assert_eq!(peer_of(&a).unwrap().uid, p.uid);
    }

    #[test]
    fn an_account_outside_the_group_is_refused_and_changes_nothing() {
        let state = demo_state();
        let before = fan_leds(&state);
        let me = peer_of(&UnixStream::pair().unwrap().0).unwrap().uid;
        let not_me = vec![me.wrapping_add(1)];
        let reply = exchange(&state, not_me, &Request::Resize { id: "demo:fans".into(), zone: 0, leds: 7 });
        assert!(matches!(reply, Response::Error(ref m) if m == crate::remote::NOT_IN_GROUP), "{reply:?}");
        assert_eq!(fan_leds(&state), before, "a refused request must leave the device unchanged");
    }

    #[test]
    fn a_trusted_account_is_served() {
        let state = demo_state();
        let me = peer_of(&UnixStream::pair().unwrap().0).unwrap().uid;
        let reply = exchange(&state, vec![me], &Request::Resize { id: "demo:fans".into(), zone: 0, leds: 7 });
        assert_eq!(reply, Response::Ok);
        assert_eq!(fan_leds(&state), 7);
    }

    #[test]
    fn audit_lines_cannot_be_forged_or_leak_data() {
        let peer = Peer { uid: 1000, pid: 42 };
        let id = "demo:fans\nrgbforge-helper audit: event=detect uid=0 ram=true";
        let line = audit_line(peer, "resize", &format!("device={}", log_safe(id)));
        assert!(!line.contains('\n') && !line.contains(" uid=0"), "{line}");
        assert!(line.starts_with("rgbforge-helper audit: event=resize uid=1000 pid=42 device=demo:fans"), "{line}");
        assert_eq!(log_safe(&"x".repeat(500)).len(), 80);
    }

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
