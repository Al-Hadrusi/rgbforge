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
use std::time::Instant;

use crate::drivers::{self, DemoMode, Driver};
use crate::proto::{self, Check, RemoteDevice, Request, Response};

/// At most this many simultaneous client connections.
const MAX_CLIENTS: usize = 8;

#[derive(Default)]
struct State {
    drivers: Vec<Box<dyn Driver>>,
    last_write: HashMap<String, Instant>,
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
    // Any local program may connect: the API can only change colours (see proto.rs).
    if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o666)) {
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
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let response = match proto::read_msg::<Request, _>(&mut reader) {
            Ok(req) => handle(req, state),
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
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
            let State { drivers, last_write } = &mut *guard;
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
        Request::Check(Check::Gpu) => Response::Text(drivers::sapphire::check_report()),
        Request::Check(Check::Ram) => {
            // Don't probe the bus while drivers are mid-write.
            let _s = lock(state);
            Response::Text(drivers::ene_ram::check_report())
        }
    }
}
