//! ASUS Aura USB controllers: stand-alone ARGB controllers and the Aura
//! controller built into ASUS motherboards (e.g. ROG Strix Z790 boards, 0b05:19af).
//!
//! Protocol (65-byte output reports, report id 0xEC):
//! * `EC 82`                        → firmware string reply `EC 02 <ascii…>`
//! * `EC B0`                        → config table reply `EC 30 00 00 <60 bytes>` (motherboards)
//! * `EC 35 ch 00 00 FF`            → put effect channel `ch` into direct (software) mode
//! * `EC 40 ch|apply start n rgb…`  → up to 20 LEDs per packet; bit 0x80 latches the frame
//!
//! Motherboard channel map: the board's own LEDs (and 12 V RGB header) use effect
//! channel 0 / direct channel 4; ARGB header `i` uses effect channel `i+1` / direct channel `i`.
//!
//! Only these commands exist in this file. Direct mode lives in the controller's RAM;
//! the "save to flash" command is deliberately never implemented, so unplugging power
//! restores whatever the board did before.

use std::io;
use std::time::Duration;

use super::{Driver, chunks};
use crate::hidraw::{HidDevice, HidInfo, MAX_REPORT};
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

const REPORT_ID: u8 = 0xEC;
const CMD_FIRMWARE: u8 = 0x82;
const CMD_CONFIG: u8 = 0xB0;
const CMD_EFFECT: u8 = 0x35;
const CMD_DIRECT: u8 = 0x40;
const MODE_DIRECT: u8 = 0xFF;
const APPLY: u8 = 0x80;
const LEDS_PER_PACKET: usize = 20;
const MAX_LEDS_PER_HEADER: u16 = 120;
/// Used when a controller doesn't tell us its layout.
const DEFAULT_ARGB_HEADERS: u8 = 3;
/// Sanity limits for values read from the config table.
const MAX_ARGB_HEADERS: u8 = 8;
const MAX_FIXED_LEDS: u8 = 60;
const MAINBOARD_DIRECT_CHANNEL: u8 = 0x04;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Channel {
    effect: u8,
    direct: u8,
}

pub struct Aura {
    dev: HidDevice,
    info: DeviceInfo,
    channels: Vec<Channel>,
    direct_set: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    pub fixed_leds: u8,
    pub argb_headers: u8,
}

/// Parse the `EC 30 …` config-table reply. Returns `None` for anything unexpected,
/// in which case the driver falls back to a conservative layout.
pub(crate) fn parse_config(reply: &[u8]) -> Option<Layout> {
    if reply.first() != Some(&REPORT_ID) || reply.get(1) != Some(&0x30) {
        return None;
    }
    let table = reply.get(4..)?;
    let argb_headers = *table.get(0x02)?;
    let fixed_leds = *table.get(0x1B)?;
    if argb_headers > MAX_ARGB_HEADERS || fixed_leds > MAX_FIXED_LEDS {
        return None;
    }
    Some(Layout { fixed_leds, argb_headers })
}

fn request(dev: &mut HidDevice, cmd: u8) -> io::Result<Vec<u8>> {
    dev.drain();
    let mut pkt = [0u8; MAX_REPORT];
    for (dst, src) in pkt.iter_mut().zip([REPORT_ID, cmd]) {
        *dst = src;
    }
    dev.write_report(&pkt)?;
    let mut buf = [0u8; MAX_REPORT];
    let n = dev.read_report(&mut buf, Duration::from_millis(300))?;
    Ok(buf.get(..n).unwrap_or_default().to_vec())
}

fn read_firmware(dev: &mut HidDevice) -> io::Result<String> {
    let reply = request(dev, CMD_FIRMWARE)?;
    Ok(reply
        .get(2..)
        .unwrap_or_default()
        .iter()
        .take_while(|b| **b != 0)
        .filter(|b| b.is_ascii_graphic())
        .map(|b| char::from(*b))
        .collect())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

impl Aura {
    pub fn open(hid: &HidInfo, name: &str, mainboard: bool) -> io::Result<Self> {
        let mut dev = HidDevice::open(hid)?;
        let firmware = read_firmware(&mut dev).unwrap_or_default();

        let mut zones = Vec::new();
        let mut channels = Vec::new();
        let mut debug = String::new();
        let argb_headers = if mainboard {
            let reply = request(&mut dev, CMD_CONFIG).unwrap_or_default();
            debug = format!(" · config: {}", hex(reply.get(..34).unwrap_or(&reply)));
            match parse_config(&reply) {
                Some(layout) => {
                    if layout.fixed_leds > 0 {
                        zones.push(ZoneInfo {
                            name: "Motherboard lights".into(),
                            leds: u16::from(layout.fixed_leds),
                            resizable: false,
                            max_leds: u16::from(layout.fixed_leds),
                        });
                        channels.push(Channel { effect: 0, direct: MAINBOARD_DIRECT_CHANNEL });
                    }
                    layout.argb_headers
                }
                // Unknown layout: only drive the ARGB headers, never guess the board's own LEDs.
                None => DEFAULT_ARGB_HEADERS,
            }
        } else {
            DEFAULT_ARGB_HEADERS
        };
        for i in 0..argb_headers {
            zones.push(ZoneInfo {
                name: format!("ARGB header {}", u16::from(i) + 1),
                leds: super::DEFAULT_HEADER_LEDS,
                resizable: true,
                max_leds: MAX_LEDS_PER_HEADER,
            });
            channels.push(Channel { effect: if mainboard { i.saturating_add(1) } else { i }, direct: i });
        }

        let fw = if firmware.is_empty() { "unknown".to_string() } else { firmware };
        Ok(Self {
            dev,
            info: DeviceInfo {
                id: format!("aura:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: if mainboard { Kind::Motherboard } else { Kind::Fans },
                zones,
                demo: false,
                experimental: true,
                detail: format!("Firmware {fw}{debug}"),
            },
            channels,
            direct_set: false,
        })
    }
}

pub(crate) fn effect_packet(channel: u8, mode: u8) -> [u8; MAX_REPORT] {
    let mut p = [0u8; MAX_REPORT];
    for (dst, src) in p.iter_mut().zip([REPORT_ID, CMD_EFFECT, channel, 0, 0, mode]) {
        *dst = src;
    }
    p
}

/// Direct-colour packets for one channel. The last packet carries the apply bit.
pub(crate) fn direct_packets(channel: u8, colors: &[Rgb]) -> Vec<[u8; MAX_REPORT]> {
    let total = colors.len().div_ceil(LEDS_PER_PACKET);
    chunks(colors, LEDS_PER_PACKET)
        .enumerate()
        .map(|(i, (start, chunk))| {
            let mut p = [0u8; MAX_REPORT];
            let apply = if i.saturating_add(1) == total { APPLY } else { 0 };
            let head = [
                REPORT_ID,
                CMD_DIRECT,
                channel | apply,
                u8::try_from(start).unwrap_or(u8::MAX),
                u8::try_from(chunk.len()).unwrap_or(0),
            ];
            let rgb = chunk.iter().flat_map(|c| [c.r, c.g, c.b]);
            for (dst, src) in p.iter_mut().zip(head.into_iter().chain(rgb)) {
                *dst = src;
            }
            p
        })
        .collect()
}

impl Driver for Aura {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone).filter(|z| z.resizable) {
            z.leds = leds.min(z.max_leds);
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        if !self.direct_set {
            for ch in &self.channels {
                self.dev.write_report(&effect_packet(ch.effect, MODE_DIRECT))?;
            }
            self.direct_set = true;
        }
        for (ch, colors) in self.channels.iter().zip(frame) {
            for pkt in direct_packets(ch.direct, colors) {
                self.dev.write_report(&pkt)?;
            }
        }
        self.dev.drain();
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn packets_split_at_20_and_apply_last() {
        let colors: Vec<Rgb> = (0..45u8).map(|i| Rgb::new(i, 1, 2)).collect();
        let p = direct_packets(1, &colors);
        assert_eq!(p.len(), 3);
        assert_eq!(&p[0][..5], &[0xEC, 0x40, 0x01, 0, 20]);
        assert_eq!(&p[1][..5], &[0xEC, 0x40, 0x01, 20, 20]);
        assert_eq!(&p[2][..5], &[0xEC, 0x40, 0x81, 40, 5]);
        assert_eq!(&p[2][5..8], &[40, 1, 2]);
        assert!(direct_packets(0, &[]).is_empty());
        assert_eq!(&effect_packet(2, MODE_DIRECT)[..6], &[0xEC, 0x35, 2, 0, 0, 0xFF]);
    }

    #[test]
    fn config_table() {
        let mut reply = vec![0u8; 64];
        reply[0] = 0xEC;
        reply[1] = 0x30;
        reply[4 + 0x02] = 3; // ARGB headers
        reply[4 + 0x1B] = 6; // board LEDs
        assert_eq!(parse_config(&reply), Some(Layout { fixed_leds: 6, argb_headers: 3 }));
        // Garbage or implausible values are rejected, not trusted.
        reply[4 + 0x02] = 200;
        assert_eq!(parse_config(&reply), None);
        assert_eq!(parse_config(&[0xEC, 0x02]), None);
        assert_eq!(parse_config(&[]), None);
    }
}
