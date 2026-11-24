//! ASRock Polychrome RGB (USB) on ASRock motherboards: 26ce:01a2, and the DeskMini LED strip 26ce:01a6.
//!
//! Protocol (from OpenRGB; 65-byte output reports with a leading 0x00, each answered by
//! one 64-byte input report):
//! * `14 _ cfg` → read-only config: cfg 1 = zones present (bitmask), 2 = LED counts,
//!   3 = red/green swap (bitmask). Reply data from byte 4.
//! * `10 _ zone 01 c1 c2 c3 speed FF` → zone shows a static colour.
//!
//! Each header shows one colour at a time: ASRock's per-LED "direct" mode has open bug
//! reports (only the first LED responds, random blinking), so it is not used.
//!
//! BLOCKED by [`allowed`]: `12` (OpenRGB: "saves all writes in device - will keep after
//! powerup") and `15` (rewrites the device's own configuration table), and every other
//! command. Only static-mode zone updates and the three read-only queries can be sent.

use std::io;
use std::time::Duration;

use super::Driver;
use crate::hidraw::{HidDevice, HidInfo};
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

const REPORT: usize = 65;
const ZONES: u8 = 8;
const NOT_PRESENT: u8 = 0x1E;
const MAX_ADDRESSABLE: u8 = 100;
const ZONE_NAMES: [&str; 8] = ["RGB header 1", "RGB header 2", "ARGB header 1", "ARGB header 2", "Chipset", "I/O cover", "Board edge", "Audio / ARGB header 3"];

pub(crate) fn allowed(pkt: &[u8]) -> bool {
    match pkt {
        [0x00, 0x10, _, zone, 0x01, ..] => *zone < ZONES,
        [0x00, 0x14, _, 0x01..=0x03, ..] => true,
        _ => false,
    }
}

fn packet(bytes: &[u8]) -> [u8; REPORT] {
    let mut p = [0u8; REPORT];
    for (dst, src) in p.iter_mut().zip(bytes) {
        *dst = *src;
    }
    p
}

/// Static-colour packet for `zone`. Default byte order is G,R,B; `swap` means R,G,B.
pub(crate) fn zone_packet(zone: u8, c: Rgb, swap: bool) -> [u8; REPORT] {
    let (a, b) = if swap { (c.r, c.g) } else { (c.g, c.r) };
    packet(&[0x00, 0x10, 0x00, zone, 0x01, a, b, c.b, 0xE0, 0xFF])
}

pub struct AsRock {
    dev: HidDevice,
    info: DeviceInfo,
    /// Device zone id for each of our zones, with its colour-order flag.
    map: Vec<(u8, bool)>,
}

impl AsRock {
    fn send(&mut self, pkt: &[u8]) -> io::Result<Vec<u8>> {
        if !allowed(pkt) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked non-lighting ASRock command"));
        }
        self.dev.write_report(pkt)?;
        // Every write is answered; read it so replies never get out of step.
        let mut buf = [0u8; 64];
        let n = self.dev.read_report(&mut buf, Duration::from_millis(500))?;
        Ok(buf.get(..n).unwrap_or_default().to_vec())
    }

    fn read_config(&mut self, cfg: u8) -> io::Result<Vec<u8>> {
        let reply = self.send(&packet(&[0x00, 0x14, 0x00, cfg]))?;
        Ok(reply.get(4..).unwrap_or_default().to_vec())
    }

    pub fn open(hid: &HidInfo, name: &str) -> io::Result<Self> {
        let dev = HidDevice::open(hid)?;
        let mut me = Self {
            dev,
            info: DeviceInfo {
                id: format!("asrock:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: Kind::Motherboard,
                zones: Vec::new(),
                demo: false,
                experimental: true,
                detail: "One colour per header (ASRock's per-LED mode is unreliable)".into(),
            },
            map: Vec::new(),
        };
        me.dev.drain();
        let present = me.read_config(0x01)?.first().copied().unwrap_or(0);
        let counts = me.read_config(0x02)?;
        let swap = me.read_config(0x03)?.first().copied().unwrap_or(0);
        for zone in 0..ZONES {
            let count = counts.get(usize::from(zone)).copied().unwrap_or(NOT_PRESENT);
            if count == NOT_PRESENT || present & (1 << zone) == 0 {
                continue;
            }
            let leds = u16::from(count.clamp(1, MAX_ADDRESSABLE));
            me.info.zones.push(ZoneInfo {
                name: ZONE_NAMES.get(usize::from(zone)).copied().unwrap_or("Zone").into(),
                leds,
                resizable: false,
                max_leds: leds,
            });
            me.map.push((zone, swap & (1 << zone) != 0));
        }
        Ok(me)
    }
}

impl Driver for AsRock {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, _zone: usize, _leds: u16) {}

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        for ((zone, swap), colors) in self.map.clone().into_iter().zip(frame) {
            let c = colors.first().copied().unwrap_or_default();
            self.send(&zone_packet(zone, c, swap))?;
        }
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.1
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn save_and_config_writes_are_blocked() {
        for bad in [&[0x00, 0x12][..], &[0x00, 0x15, 0x00, 0x02], &[0x00, 0x10, 0x00, 0x02, 0xE3], &[0x00, 0x10, 0x00, 0xFF, 0x01], &[0x00, 0x14, 0x00, 0x09], &[0x01, 0x10, 0, 0, 1]] {
            assert!(!allowed(bad), "{bad:02x?}");
        }
        assert!(allowed(&packet(&[0x00, 0x14, 0x00, 0x02])));
    }

    #[test]
    fn static_packet_and_colour_order() {
        let p = zone_packet(2, Rgb::new(10, 20, 30), false);
        assert_eq!(&p[..10], &[0x00, 0x10, 0x00, 0x02, 0x01, 20, 10, 30, 0xE0, 0xFF]); // G, R, B
        let p = zone_packet(2, Rgb::new(10, 20, 30), true);
        assert_eq!(&p[5..8], &[10, 20, 30]); // R, G, B when swapped
        assert!(allowed(&p) && p.len() == 65);
    }
}
