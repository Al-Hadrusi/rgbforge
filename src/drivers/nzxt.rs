//! NZXT HUE 2 family lighting: Kraken X3/Z3 and 2024 Elite RGB pumps, Smart Device V2,
//! HUE 2 / HUE 2 Ambient, RGB & Fan Controllers and the 2023 RGB Controller.
//!
//! Protocol (from liquidctl and OpenRGB; 64-byte output reports, first byte = command):
//! * `20 03`                     → read-only accessory query, reply `21 03 …`
//! * `22 10 cid 00 <20×GRB>`     LEDs 0-19 of channel bitmask `cid`
//! * `22 11 cid 00 <20×GRB>`     LEDs 20-39
//! * `22 A0 cid 00 01 00 00 28 00 00 80 00 32 00 00 01`  apply ("super-fixed" mode)
//!
//! COOLING SAFETY: the same USB connection controls pump and fan speed (`72 …`, `62 …`,
//! `60 …`, `70 …`). Every write goes through [`allowed`], which only permits the lighting
//! commands above, so pump/fan/LCD/firmware commands cannot be sent by construction.
//! Colours live in device RAM until power is cut; nothing is saved.

use std::io;
use std::time::{Duration, Instant};

use super::Driver;
use crate::hidraw::{HidDevice, HidInfo};
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

const REPORT: usize = 64;
const LEDS_PER_CHANNEL: usize = 40;
const LEDS_PER_PACKET: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    KrakenX3,
    KrakenZ3,
    KrakenElite2024,
    /// Smart Device V2 / HUE 2 / RGB & Fan Controllers / 2023 RGB Controller.
    Hub { channels: u8 },
}

/// The only command prefixes this driver may ever send.
pub(crate) fn allowed(pkt: &[u8]) -> bool {
    matches!(pkt, [0x20, 0x03, ..] | [0x22, 0x10, ..] | [0x22, 0x11, ..] | [0x22, 0xA0, ..])
}

/// LEDs on an NZXT accessory, by the id the device reports (from OpenRGB's table).
pub(crate) fn accessory_leds(id: u8) -> usize {
    match id {
        0x01 | 0x04 | 0x0A => 10,
        0x02 | 0x05 | 0x0B | 0x0C | 0x10 | 0x17 | 0x18 | 0x19 => 8,
        0x06 => 6,
        0x08 => 14,
        0x09 => 15,
        0x11 => 1,
        0x13 | 0x14 => 18,
        0x15 | 0x16 => 20,
        0x1C => 16,
        0x1D | 0x1E | 0x1F | 0x2A => 24,
        _ => 0,
    }
}

/// Parse a `21 03` reply into LED counts per channel.
pub(crate) fn parse_accessories(reply: &[u8]) -> Option<Vec<usize>> {
    if reply.get(..2) != Some(&[0x21, 0x03]) {
        return None;
    }
    let channels = usize::from(*reply.get(14)?).min(8);
    Some(
        (0..channels)
            .map(|c| {
                (0..6)
                    .map_while(|a| reply.get(15 + 6 * c + a).copied().filter(|id| *id != 0))
                    .map(accessory_leds)
                    .sum::<usize>()
                    .min(LEDS_PER_CHANNEL)
            })
            .collect(),
    )
}

fn packet(head: &[u8], body: &[u8]) -> [u8; REPORT] {
    let mut p = [0u8; REPORT];
    for (dst, src) in p.iter_mut().zip(head.iter().chain(body)) {
        *dst = *src;
    }
    p
}

/// The three packets that show `colors` on channel bitmask `cid`.
pub(crate) fn frame_packets(cid: u8, colors: &[Rgb]) -> [[u8; REPORT]; 3] {
    let grb: Vec<u8> = colors.iter().take(LEDS_PER_CHANNEL).flat_map(|c| [c.g, c.r, c.b]).collect();
    let (first, second) = grb.split_at(grb.len().min(LEDS_PER_PACKET * 3));
    [
        packet(&[0x22, 0x10, cid, 0x00], first),
        packet(&[0x22, 0x11, cid, 0x00], second),
        packet(&[0x22, 0xA0, cid, 0x00, 0x01, 0x00, 0x00, 0x28, 0x00, 0x00, 0x80, 0x00, 0x32, 0x00, 0x00, 0x01], &[]),
    ]
}

pub struct Nzxt {
    dev: HidDevice,
    info: DeviceInfo,
}

impl Nzxt {
    fn send(&mut self, pkt: &[u8]) -> io::Result<()> {
        if !allowed(pkt) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked non-lighting NZXT command"));
        }
        self.dev.write_report(pkt)
    }

    fn query_accessories(&mut self) -> Option<Vec<usize>> {
        self.dev.drain();
        self.send(&packet(&[0x20, 0x03], &[])).ok()?;
        // Krakens also send unsolicited status reports (`75 …`); skip anything else.
        let deadline = Instant::now() + Duration::from_millis(1000);
        let mut buf = [0u8; REPORT];
        while Instant::now() < deadline {
            let n = self.dev.read_report(&mut buf, Duration::from_millis(200)).ok()?;
            if let Some(counts) = parse_accessories(buf.get(..n).unwrap_or_default()) {
                return Some(counts);
            }
        }
        None
    }

    pub fn open(hid: &HidInfo, name: &str, model: Model) -> io::Result<Self> {
        let dev = HidDevice::open(hid)?;
        let mut me = Self {
            dev,
            info: DeviceInfo {
                id: format!("nzxt:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: match model {
                    Model::Hub { .. } => Kind::Fans,
                    _ => Kind::Cooler,
                },
                zones: Vec::new(),
                demo: false,
                experimental: true,
                detail: String::new(),
            },
        };
        let detected = me.query_accessories();
        let names: Vec<String> = match model {
            Model::KrakenX3 => vec!["Accessory channel".into(), "Pump ring".into(), "Pump logo".into()],
            Model::KrakenZ3 => vec!["Accessory channel".into()],
            Model::KrakenElite2024 => vec!["Channel 1".into(), "Channel 2".into()],
            Model::Hub { channels } => (1..=channels).map(|i| format!("LED channel {i}")).collect(),
        };
        me.info.zones = names
            .into_iter()
            .enumerate()
            .map(|(i, name)| {
                let fixed = match (model, i) {
                    (Model::KrakenX3, 1) => Some(8),
                    (Model::KrakenX3, 2) => Some(1),
                    _ => None,
                };
                let found = detected.as_ref().and_then(|d| d.get(i).copied());
                let leds = fixed.or(found).unwrap_or(super::DEFAULT_HEADER_LEDS.into()).min(LEDS_PER_CHANNEL);
                ZoneInfo {
                    name,
                    leds: u16::try_from(leds).unwrap_or(0),
                    resizable: fixed.is_none(),
                    max_leds: u16::try_from(fixed.unwrap_or(LEDS_PER_CHANNEL)).unwrap_or(40),
                }
            })
            .collect();
        me.info.detail = if detected.is_some() {
            "USB lighting controller · accessories detected".into()
        } else {
            "USB lighting controller · accessories not detected, set LED counts below".into()
        };
        Ok(me)
    }
}

impl Driver for Nzxt {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone).filter(|z| z.resizable) {
            z.leds = leds.min(z.max_leds);
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        for (i, colors) in frame.iter().enumerate().take(8) {
            let cid = 1u8 << i;
            for p in frame_packets(cid, colors) {
                self.send(&p)?;
            }
        }
        self.dev.drain(); // discard status reports so the buffer never fills
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX // colours persist until power is cut
    }

    fn min_interval_secs(&self) -> f32 {
        0.04 // "the device can get confused when hammered" (liquidctl)
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn only_lighting_commands_pass() {
        for ok in [&[0x20, 0x03][..], &[0x22, 0x10, 1], &[0x22, 0x11, 1], &[0x22, 0xA0, 1]] {
            assert!(allowed(ok));
        }
        // Pump curve, fan duty, fan init, Kraken init, LCD, firmware query: all blocked.
        for bad in [&[0x72, 0x01][..], &[0x62, 0x01], &[0x60, 0x03], &[0x70, 0x02], &[0x30, 0x02], &[0x36, 0x01], &[0x22, 0x40], &[0x2A, 0x04], &[]] {
            assert!(!allowed(bad), "{bad:02x?}");
        }
    }

    #[test]
    fn packets_are_grb_and_split_at_20() {
        let colors: Vec<Rgb> = (0..25u8).map(|i| Rgb::new(i, 100, 200)).collect();
        let [a, b, c] = frame_packets(0x02, &colors);
        assert_eq!(&a[..7], &[0x22, 0x10, 0x02, 0x00, 100, 0, 200]); // G, R, B
        assert_eq!(&b[..7], &[0x22, 0x11, 0x02, 0x00, 100, 20, 200]); // LED 20
        assert_eq!(&c[..16], &[0x22, 0xA0, 0x02, 0, 1, 0, 0, 0x28, 0, 0, 0x80, 0, 0x32, 0, 0, 1]);
        assert!(frame_packets(1, &vec![Rgb::new(1, 1, 1); 99]).iter().all(|p| p.len() == 64));
    }

    #[test]
    fn accessory_parsing() {
        let mut r = vec![0u8; 64];
        r[0] = 0x21;
        r[1] = 0x03;
        r[14] = 2;
        r[15] = 0x13; // channel 1: 18-LED fan ...
        r[16] = 0x13; // ... and another
        r[21] = 0x08; // channel 2: 14 LEDs
        assert_eq!(parse_accessories(&r), Some(vec![36, 14]));
        r[17] = 0x13; // a third fan → capped at 40
        assert_eq!(parse_accessories(&r), Some(vec![40, 14]));
        assert_eq!(parse_accessories(&[0x75, 0x01]), None);
        assert_eq!(parse_accessories(&[]), None);
    }
}
