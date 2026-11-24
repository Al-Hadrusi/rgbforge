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

pub fn connect() -> io::Result<Shared> {
    let stream = UnixStream::connect(proto::SOCKET)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let conn: Shared = Arc::new(Mutex::new(Conn { writer: stream.try_clone()?, reader: BufReader::new(stream) }));
    match request(&conn, &Request::Hello { version: proto::VERSION })? {
        Response::Hello { version } if version == proto::VERSION => Ok(conn),
        _ => Err(io::Error::other("the RGB Forge hardware service is a different version; re-run ./install.sh")),
    }
}

pub fn request(conn: &Shared, req: &Request) -> io::Result<Response> {
    let mut c = conn.lock().unwrap_or_else(|p| p.into_inner());
    proto::write_msg(&mut c.writer, req)?;
    proto::read_msg(&mut c.reader)
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

pub const NOT_RUNNING: &str = "RGB Forge's hardware service isn't running, so real devices can't be controlled. Re-run ./install.sh, or run: sudo systemctl enable --now rgbforge-helper";

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
        Err(_) => (Vec::new(), vec![NOT_RUNNING.to_string()]),
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
