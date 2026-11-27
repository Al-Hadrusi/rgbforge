//! App side of the helper connection: devices behind `rgbforge-helper` look like ordinary
//! drivers to the lighting engine, but every operation is a request to the helper.

use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::drivers::{self, DemoMode, Detection, Driver};
use crate::model::{DeviceInfo, Rgb};
use crate::proto::{self, Check, Request, Response};

const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

pub type Shared = Arc<Mutex<Conn>>;

/// Open and greet a new connection to the hardware service.
fn open_conn() -> io::Result<Conn> {
    let stream = UnixStream::connect(proto::SOCKET)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let mut c = Conn { writer: stream.try_clone()?, reader: BufReader::new(stream) };
    proto::write_msg(&mut c.writer, &Request::Hello { version: proto::VERSION })?;
    match proto::read_msg(&mut c.reader)? {
        Response::Hello { version } if version == proto::VERSION => Ok(c),
        // The service checked who we are and refused (not in the `rgbforge` group yet).
        Response::Error(m) if m == NOT_IN_GROUP => Err(io::Error::new(io::ErrorKind::PermissionDenied, m)),
        _ => Err(io::Error::other("the RGB Forge hardware service is a different version; re-run ./install.sh")),
    }
}

pub fn connect() -> io::Result<Shared> {
    Ok(Arc::new(Mutex::new(open_conn()?)))
}

/// The connection itself broke (e.g. the service closed an idle connection or restarted).
fn is_disconnect(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::NotConnected
    )
}

pub fn request(conn: &Shared, req: &Request) -> io::Result<Response> {
    let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
    let first = proto::write_msg(&mut c.writer, req).and_then(|()| proto::read_msg(&mut c.reader));
    match first {
        // Reconnect once and retry: the service's device list is shared, so ids stay valid.
        Err(e) if is_disconnect(&e) => {
            *c = open_conn()?;
            proto::write_msg(&mut c.writer, req)?;
            proto::read_msg(&mut c.reader)
        }
        other => other,
    }
}

/// Run a read-only check inside the helper (which has the hardware access).
pub fn check(what: Check) -> io::Result<String> {
    match request(&connect()?, &Request::Check(what))? {
        Response::Text(t) => Ok(t),
        Response::Error(e) => Err(io::Error::other(e)),
        _ => Err(io::Error::other("unexpected reply from the hardware service")),
    }
}

pub struct RemoteDriver {
    conn: Shared,
    info: DeviceInfo,
    min_interval: f32,
    keepalive: f32,
}

impl Driver for RemoteDriver {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone).filter(|z| z.resizable) {
            z.leds = leds.min(z.max_leds);
            let _ = request(&self.conn, &Request::Resize { id: self.info.id.clone(), zone, leds });
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        match request(&self.conn, &Request::Apply { id: self.info.id.clone(), frame: frame.to_vec() })? {
            Response::Ok => Ok(()),
            Response::Error(e) => Err(io::Error::other(e)),
            _ => Err(io::Error::other("unexpected reply from the hardware service")),
        }
    }

    fn keepalive_secs(&self) -> f32 {
        self.keepalive
    }

    fn min_interval_secs(&self) -> f32 {
        self.min_interval
    }
}

/// The command that starts the hardware service now and at every boot, for this PC's init
/// system (systemd, or OpenRC as on Gentoo).
pub fn start_service_command() -> &'static str {
    start_command_for(std::path::Path::new("/run/systemd/system").is_dir())
}

pub fn start_command_for(systemd: bool) -> &'static str {
    if systemd {
        "sudo systemctl enable --now rgbforge-helper"
    } else {
        "sudo rc-update add rgbforge-helper default && sudo rc-service rgbforge-helper start"
    }
}

pub fn not_running() -> String {
    format!("RGB Forge's hardware service isn't running, so real devices can't be controlled. Re-run ./install.sh, or run: {}", start_service_command())
}
pub const NOT_IN_GROUP: &str = "You're not allowed to use RGB Forge's hardware service yet. Log out and back in (or restart) once after installing, so your account joins the 'rgbforge' group.";

/// Find devices through the helper; demo devices are added here in the app.
pub fn detect(demo: DemoMode, gpu: bool, ram: bool) -> Detection {
    let remote = connect().and_then(|conn| match request(&conn, &Request::Detect { gpu, ram })? {
        Response::Devices { devices, problems } => Ok((conn, devices, problems)),
        Response::Error(e) => Err(io::Error::other(e)),
        _ => Err(io::Error::other("unexpected reply from the hardware service")),
    });
    let (mut found, mut problems): (Vec<Box<dyn Driver>>, Vec<String>) = match remote {
        Ok((conn, devices, problems)) => (
            devices
                .into_iter()
                .map(|d| {
                    Box::new(RemoteDriver { conn: Arc::clone(&conn), info: d.info, min_interval: d.min_interval, keepalive: d.keepalive })
                        as Box<dyn Driver>
                })
                .collect(),
            problems,
        ),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => (Vec::new(), vec![NOT_IN_GROUP.to_string()]),
        Err(_) => (Vec::new(), vec![not_running()]),
    };
    let show_demo = match demo {
        DemoMode::Always => true,
        DemoMode::Never => false,
        DemoMode::Auto => found.is_empty(),
    };
    if show_demo {
        found.extend(drivers::demo_devices());
    }
    problems.dedup();
    Detection { drivers: found, problems }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_command_matches_the_init_system() {
        assert_eq!(start_command_for(true), "sudo systemctl enable --now rgbforge-helper");
        assert!(start_command_for(false).contains("rc-service rgbforge-helper start"));
        assert!(not_running().contains(start_service_command()));
    }
}
