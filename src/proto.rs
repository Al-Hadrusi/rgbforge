//! The message format between the RGB Forge app and `rgbforge-helper`, the privileged
//! service that is the only thing allowed to touch lighting hardware.
//!
//! One JSON message per line over a Unix socket. The API is deliberately narrow: list
//! devices, show colours, change an LED count, run a read-only check. There is no way to
//! send raw bytes to a device; every write still goes through each driver's allow-list.
//! So even though any local program may connect, the worst it can do is change colours.

use std::io::{self, BufRead, BufReader, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::model::{DeviceInfo, Rgb};

pub const SOCKET: &str = "/run/rgbforge-helper/socket";
pub const VERSION: u32 = 1;
/// Largest accepted message (a full frame for every device fits easily).
pub const MAX_LINE: usize = 1 << 20;
/// Largest frame accepted per zone, whatever a client claims.
pub const MAX_ZONE_LEDS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Check {
    Gpu,
    Ram,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Hello { version: u32 },
    Detect { gpu: bool, ram: bool },
    Resize { id: String, zone: usize, leds: u16 },
    Apply { id: String, frame: Vec<Vec<Rgb>> },
    Check(Check),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteDevice {
    pub info: DeviceInfo,
    pub min_interval: f32,
    pub keepalive: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Hello { version: u32 },
    Devices { devices: Vec<RemoteDevice>, problems: Vec<String> },
    Ok,
    Text(String),
    Error(String),
}

pub fn write_msg<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
    if line.len() > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "message too large"));
    }
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

/// Read one message, refusing anything over [`MAX_LINE`] bytes.
pub fn read_msg<T: DeserializeOwned, R: Read>(r: &mut BufReader<R>) -> io::Result<T> {
    let mut buf = Vec::new();
    let limit = u64::try_from(MAX_LINE).unwrap_or(u64::MAX).saturating_add(1);
    let n = r.by_ref().take(limit).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
    }
    if buf.last() != Some(&b'\n') {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large or truncated"));
    }
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Make a client-supplied frame exactly fit the device: one list per zone, each exactly
/// that zone's LED count (extra data dropped, missing LEDs black). Never trust the client.
pub fn fit_frame(info: &DeviceInfo, frame: Vec<Vec<Rgb>>) -> Vec<Vec<Rgb>> {
    let mut zones = frame.into_iter();
    info.zones
        .iter()
        .map(|z| {
            let n = usize::from(z.leds).min(MAX_ZONE_LEDS);
            let mut c = zones.next().unwrap_or_default();
            c.resize(n, Rgb::BLACK);
            c
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::{Kind, ZoneInfo};

    #[test]
    fn roundtrip_and_limits() {
        let req = Request::Apply { id: "x".into(), frame: vec![vec![Rgb::new(1, 2, 3)]] };
        let mut buf = Vec::new();
        write_msg(&mut buf, &req).unwrap();
        let back: Request = read_msg(&mut BufReader::new(&buf[..])).unwrap();
        assert_eq!(back, req);

        // Oversized, unterminated and garbage input are all rejected without panicking.
        let huge = vec![b'a'; MAX_LINE + 10];
        assert!(read_msg::<Request, _>(&mut BufReader::new(&huge[..])).is_err());
        assert!(read_msg::<Request, _>(&mut BufReader::new(&b"{\"Hello\""[..])).is_err());
        assert!(read_msg::<Request, _>(&mut BufReader::new(&b"not json\n"[..])).is_err());
        assert!(read_msg::<Request, _>(&mut BufReader::new(&b""[..])).is_err());
    }

    #[test]
    fn frames_are_fitted_to_the_device() {
        let info = DeviceInfo {
            id: "d".into(),
            name: "d".into(),
            kind: Kind::Other,
            zones: vec![
                ZoneInfo { name: "a".into(), leds: 2, resizable: false, max_leds: 2 },
                ZoneInfo { name: "b".into(), leds: 3, resizable: false, max_leds: 3 },
            ],
            demo: false,
            experimental: false,
            detail: String::new(),
        };
        let fitted = fit_frame(&info, vec![vec![Rgb::WHITE; 99]]);
        assert_eq!(fitted, vec![vec![Rgb::WHITE; 2], vec![Rgb::BLACK; 3]]);
    }
}
