//! Corsair Lighting Node Pro / Node Core / Commander Pro (lighting part only).
//!
//! Protocol (64-byte output reports, no report id → a leading 0x00 byte):
//! * `37 ch`                  reset channel
//! * `38 ch 02`               channel → software control
//! * `32 ch start n c vals…`  up to 50 values of colour component `c` (0=R, 1=G, 2=B)
//! * `33 FF`                  commit (show) the frame
//!
//! The device falls back to its own effect after a few seconds without data,
//! which is why we resend every second. No fan/pump/firmware commands are ever sent.

use std::io;

use super::{Driver, chunks};
use crate::hidraw::{HidDevice, HidInfo, MAX_REPORT};
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

const CMD_DIRECT: u8 = 0x32;
const CMD_COMMIT: u8 = 0x33;
const CMD_RESET: u8 = 0x37;
const CMD_PORT_STATE: u8 = 0x38;
const STATE_SOFTWARE: u8 = 0x02;
const VALUES_PER_PACKET: usize = 50;
const MAX_LEDS_PER_CHANNEL: u16 = 204;

pub struct CorsairNode {
    dev: HidDevice,
    info: DeviceInfo,
    initialised: bool,
}

impl CorsairNode {
    pub fn open(hid: &HidInfo, name: &str, channels: u8) -> io::Result<Self> {
        let dev = HidDevice::open(hid)?;
        let zones = (1..=channels)
            .map(|i| ZoneInfo {
                name: format!("LED channel {i}"),
                leds: super::DEFAULT_HEADER_LEDS,
                resizable: true,
                max_leds: MAX_LEDS_PER_CHANNEL,
            })
            .collect();
        Ok(Self {
            dev,
            info: DeviceInfo {
                id: format!("corsair:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: Kind::Fans,
                zones,
                demo: false,
                experimental: true,
                detail: "USB lighting controller".into(),
            },
            initialised: false,
        })
    }
}

fn packet(bytes: &[u8]) -> [u8; MAX_REPORT] {
    let mut p = [0u8; MAX_REPORT];
    // p[0] stays 0: the "no report id" byte hidraw expects.
    for (dst, src) in p.iter_mut().skip(1).zip(bytes) {
        *dst = *src;
    }
    p
}

pub(crate) fn direct_packets(channel: u8, colors: &[Rgb]) -> Vec<[u8; MAX_REPORT]> {
    let mut out = Vec::new();
    for component in 0..3u8 {
        let values: Vec<Rgb> = colors.to_vec();
        for (start, chunk) in chunks(&values, VALUES_PER_PACKET) {
            let mut bytes = vec![
                CMD_DIRECT,
                channel,
                u8::try_from(start).unwrap_or(u8::MAX),
                u8::try_from(chunk.len()).unwrap_or(0),
                component,
            ];
            bytes.extend(chunk.iter().map(|c| match component {
                0 => c.r,
                1 => c.g,
                _ => c.b,
            }));
            out.push(packet(&bytes));
        }
    }
    out
}

impl Driver for CorsairNode {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone) {
            z.leds = leds.min(z.max_leds);
            self.initialised = false;
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        let channels = u8::try_from(self.info.zones.len()).unwrap_or(0);
        if !self.initialised {
            for ch in 0..channels {
                self.dev.write_report(&packet(&[CMD_RESET, ch]))?;
                self.dev.write_report(&packet(&[CMD_PORT_STATE, ch, STATE_SOFTWARE]))?;
            }
            self.initialised = true;
        }
        for (ch, colors) in (0..channels).zip(frame) {
            for p in direct_packets(ch, colors) {
                self.dev.write_report(&p)?;
            }
        }
        self.dev.write_report(&packet(&[CMD_COMMIT, 0xFF]))?;
        self.dev.drain();
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn packets_per_component() {
        let colors: Vec<Rgb> = (0..60u8).map(|i| Rgb::new(i, 100, 200)).collect();
        let p = direct_packets(1, &colors);
        assert_eq!(p.len(), 6); // 2 packets × R,G,B
        assert_eq!(&p[0][..7], &[0, 0x32, 1, 0, 50, 0, 0]);
        assert_eq!(&p[1][..7], &[0, 0x32, 1, 50, 10, 0, 50]);
        assert_eq!(&p[2][..7], &[0, 0x32, 1, 0, 50, 1, 100]);
        assert_eq!(&p[5][..7], &[0, 0x32, 1, 50, 10, 2, 200]);
    }
}
