//! Gigabyte RGB Fusion 2 (USB, ITE IT5701/IT5702/IT8297/IT5711 chips) on Gigabyte boards.
//!
//! Protocol (from OpenRGB and liquidctl). Every command is a 64-byte HID *feature* report
//! with report id 0xCC, sent through the `hidapi` crate (pure-Rust Linux backend):
//! * `CC 60`                                 → identify (reply: product string "IT57xx…", LED counts, calibration)
//! * `CC 20 FF 00 00 00 …  01 FF 00 B G R 00` board lighting: all onboard zones, static colour
//! * `CC 28 FF 00`                           commit (never with a zero mask: that cancels)
//! * `CC 32 03`                              switch ARGB headers D_LED1/2 to per-LED mode
//! * `CC 34 n1n0 …`                          ARGB LED count (in steps of 32/64/256 LEDs)
//! * `CC 58|59 off_lo off_hi len <19 LEDs>`  per-LED colours for D_LED1 / D_LED2
//!
//! BLOCKED by [`allowed`] (anything not on the list above is refused), notably:
//! `CC 5E` save to flash, `CC 47` persistent lighting, `CC 33` calibration write, the strip
//! scan commands, and anything else. Only the SMBus version of Fusion 2 has bricked boards;
//! this USB version has no documented bricks. The 048d:57db "upgrade mode" device is never touched.

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io;
use std::thread;
use std::time::Duration;

use super::Driver;
use crate::hidraw::HidInfo;
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

pub const USAGE_PAGE: u16 = 0xFF89;
const REPORT_ID: u8 = 0xCC;
const REPORT: usize = 64;
const LEDS_PER_PACKET: usize = 19;
/// IT57xx chips support 256 LEDs per string.
const MAX_LEDS: u16 = 256;
const DIRECT_HEADERS: [u8; 2] = [0x58, 0x59];

/// The full write allow-list. `pkt` includes the report id.
pub(crate) fn allowed(pkt: &[u8]) -> bool {
    match pkt {
        [REPORT_ID, 0x60, ..] => true,
        // Onboard effect header with effect_type (byte 11) = 1, static only.
        [REPORT_ID, 0x20..=0x27, rest @ ..] => rest.get(9) == Some(&0x01),
        // Commit, never with an all-zero mask (that cancels pending changes).
        [REPORT_ID, 0x28, mask @ ..] => mask.iter().take(8).any(|b| *b != 0),
        // Per-LED mode for D_LED1 + D_LED2 only (the two headers this driver drives).
        [REPORT_ID, 0x32, 0x03, rest @ ..] => rest.iter().all(|b| *b == 0),
        // LED-count codes for the six headers, two per byte; valid codes are 0-4
        // (32/64/256/512/1024 LEDs, OpenRGB's LedCount).
        [REPORT_ID, 0x34, a, b, c, rest @ ..] => {
            [a, b, c].iter().all(|v| (**v & 0x0F) <= 4 && (**v >> 4) <= 4) && rest.iter().all(|b| *b == 0)
        }
        // Direct colours: whole LEDs, at most 19 per packet, inside the 256-LED string.
        [REPORT_ID, 0x58 | 0x59, lo, hi, len, ..] => {
            let end = usize::from(u16::from_le_bytes([*lo, *hi])) + usize::from(*len);
            usize::from(*len) <= LEDS_PER_PACKET * 3 && *len % 3 == 0 && end <= usize::from(MAX_LEDS) * 3
        }
        // Turn off the board's audio-reactive mode / LampArray mode: value 0 only.
        [REPORT_ID, 0x31 | 0x48, 0x00, ..] => true,
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

/// Static colour on all onboard zones (`B, G, R, 0` colour layout).
pub(crate) fn board_packet(c: Rgb) -> [u8; REPORT] {
    let mut p = packet(&[REPORT_ID, 0x20, 0xFF, 0x00, 0x00, 0x00]);
    let tail = [0x01, 0xFF, 0x00, c.b, c.g, c.r, 0x00];
    for (dst, src) in p.iter_mut().skip(11).zip(tail) {
        *dst = src;
    }
    p
}

/// Smallest LED-count step (0 = 32, 1 = 64, 2 = 256) that covers `leds`.
pub(crate) fn count_code(leds: u16) -> u8 {
    match leds {
        0..=32 => 0,
        33..=64 => 1,
        _ => 2,
    }
}

/// Per-LED packets for one ARGB header. `order` maps R,G,B to byte positions 0-2.
pub(crate) fn direct_packets(header: u8, colors: &[Rgb], order: [usize; 3]) -> Vec<[u8; REPORT]> {
    colors
        .chunks(LEDS_PER_PACKET)
        .enumerate()
        .map(|(i, chunk)| {
            let offset = u16::try_from(i * LEDS_PER_PACKET * 3).unwrap_or(u16::MAX);
            let [lo, hi] = offset.to_le_bytes();
            let mut p = packet(&[REPORT_ID, header, lo, hi, u8::try_from(chunk.len() * 3).unwrap_or(0)]);
            for (j, c) in chunk.iter().enumerate() {
                for (value, pos) in [c.r, c.g, c.b].into_iter().zip(order) {
                    if let Some(b) = p.get_mut(5 + j * 3 + pos) {
                        *b = value;
                    }
                }
            }
            p
        })
        .collect()
}

/// Byte positions from a strip calibration value (`B` in bits 0-7, `G` 8-15, `R` 16-23).
/// Unset or invalid values fall back to G,R,B, the usual order for ARGB strips.
pub(crate) fn order_from_calibration(v: u32) -> [usize; 3] {
    let [b, g, r, _] = v.to_le_bytes();
    let order = [usize::from(r), usize::from(g), usize::from(b)];
    let mut seen = [false; 3];
    for p in order {
        match seen.get_mut(p) {
            Some(s) if !*s => *s = true,
            _ => return [1, 0, 2], // GRB
        }
    }
    order
}

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

pub struct Gigabyte {
    dev: hidapi::HidDevice,
    info: DeviceInfo,
    orders: [[usize; 3]; 2],
    direct_ready: bool,
    sizes_sent: Option<(u8, u8)>,
    /// LED-count codes of headers 3-6 as the board reported them; sent back unchanged.
    other_counts: (u8, u8),
}

impl Gigabyte {
    fn send(&self, pkt: &[u8]) -> io::Result<()> {
        if !allowed(pkt) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked non-lighting Gigabyte command"));
        }
        self.dev.send_feature_report(pkt).map_err(err)
    }

    pub fn open(hid: &HidInfo, name: &str) -> io::Result<Self> {
        // Check permissions with a plain open first, so the app can explain a missing udev rule.
        // Plain open first (clear permission errors), then confirm it is the identified device.
        // Holding this handle while hidapi opens the same node, and re-checking afterwards,
        // means a swapped device can't slip in between the checks.
        let probe = OpenOptions::new().read(true).write(true).open(&hid.node)?;
        crate::hidraw::verify_opened(hid, &probe)?;
        let api = hidapi::HidApi::new().map_err(err)?; // discovery is disabled in main()
        let path = CString::new(hid.node.to_string_lossy().as_bytes()).map_err(err)?;
        let dev = api.open_path(&path).map_err(err)?;
        crate::hidraw::verify_opened(hid, &probe)?;
        let now = std::fs::metadata(&hid.node)?;
        if std::os::unix::fs::MetadataExt::rdev(&now) != std::os::unix::fs::MetadataExt::rdev(&probe.metadata()?) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "device changed while opening"));
        }
        drop(probe);
        let mut me = Self {
            dev,
            info: DeviceInfo {
                id: format!("gigabyte:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: Kind::Motherboard,
                zones: Vec::new(),
                demo: false,
                experimental: true,
                detail: String::new(),
            },
            orders: [[1, 0, 2]; 2],
            direct_ready: false,
            sizes_sent: None,
            other_counts: (0, 0),
        };

        // Identify. The reply must look like an ITE RGB controller, or we stop here.
        me.send(&packet(&[REPORT_ID, 0x60]))?;
        let mut reply = [0u8; REPORT];
        if let Some(b) = reply.first_mut() {
            *b = REPORT_ID;
        }
        let n = me.dev.get_feature_report(&mut reply).map_err(err)?;
        let product: String = reply
            .get(12..40)
            .unwrap_or_default()
            .iter()
            .take_while(|b| **b != 0)
            .filter(|b| b.is_ascii_graphic() || **b == b' ')
            .map(|b| char::from(*b))
            .collect();
        if n < 40 || !product.starts_with("IT") {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("controller didn't identify as expected (\"{product}\"); not touching it")));
        }
        let cal = |at: usize| reply.get(at..at + 4).and_then(|s| <[u8; 4]>::try_from(s).ok()).map_or(0, u32::from_le_bytes);
        me.orders = [order_from_calibration(cal(44)), order_from_calibration(cal(48))];
        // IT8297Report bytes 9-10: LED-count codes of headers 3/4 and 5/6. Keep them, so
        // setting our two headers' counts doesn't change headers this driver doesn't drive.
        let valid = |v: u8| (v & 0x0F) <= 4 && (v >> 4) <= 4;
        let keep = |at: usize| reply.get(at).copied().filter(|v| valid(*v)).unwrap_or(0);
        me.other_counts = (keep(9), keep(10));
        let flags = reply.get(11).copied().unwrap_or(0);
        if flags & 0x02 != 0 {
            me.send(&packet(&[REPORT_ID, 0x48, 0x00]))?; // LampArray mode off
        }
        me.send(&packet(&[REPORT_ID, 0x31, 0x00]))?; // audio-reactive mode off

        me.info.detail = format!("{product} · colour order from board calibration");
        me.info.zones = vec![
            ZoneInfo { name: "Board lighting".into(), leds: 1, resizable: false, max_leds: 1 },
            ZoneInfo { name: "ARGB header D_LED1".into(), leds: super::DEFAULT_HEADER_LEDS, resizable: true, max_leds: MAX_LEDS },
            ZoneInfo { name: "ARGB header D_LED2".into(), leds: super::DEFAULT_HEADER_LEDS, resizable: true, max_leds: MAX_LEDS },
        ];
        Ok(me)
    }
}

impl Driver for Gigabyte {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone).filter(|z| z.resizable) {
            z.leds = leds.min(z.max_leds);
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        if !self.direct_ready {
            self.send(&packet(&[REPORT_ID, 0x32, 0x03]))?; // D_LED1 + D_LED2 per-LED mode
            thread::sleep(Duration::from_millis(50));
            self.direct_ready = true;
        }
        let size = |i: usize| count_code(self.info.zones.get(i).map_or(0, |z| z.leds));
        let sizes = (size(1), size(2));
        if self.sizes_sent != Some(sizes) {
            self.send(&packet(&[REPORT_ID, 0x34, (sizes.1 << 4) | sizes.0, self.other_counts.0, self.other_counts.1]))?;
            self.sizes_sent = Some(sizes);
        }
        if let Some(c) = frame.first().and_then(|z| z.first()) {
            self.send(&board_packet(*c))?;
            self.send(&packet(&[REPORT_ID, 0x28, 0xFF, 0x00]))?;
        }
        for (i, header) in DIRECT_HEADERS.iter().enumerate() {
            if let (Some(colors), Some(order)) = (frame.get(i + 1), self.orders.get(i)) {
                for p in direct_packets(*header, colors, *order) {
                    self.send(&p)?;
                }
            }
        }
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.03
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn payloads_of_mode_count_and_colour_packets_are_bounded() {
        assert!(allowed(&packet(&[REPORT_ID, 0x32, 0x03])));
        assert!(!allowed(&packet(&[REPORT_ID, 0x32, 0xFF])), "only headers 1+2");
        assert!(!allowed(&packet(&[REPORT_ID, 0x32, 0x03, 0x01])));
        assert!(allowed(&packet(&[REPORT_ID, 0x34, 0x21, 0x44, 0x00])));
        assert!(!allowed(&packet(&[REPORT_ID, 0x34, 0x05, 0x00, 0x00])), "no such LED-count code");
        assert!(!allowed(&packet(&[REPORT_ID, 0x34, 0x00, 0x00, 0x00, 0x09])));
        assert!(allowed(&packet(&[REPORT_ID, 0x58, 0xC7, 0x02, 57])), "711 + 57 = 768: the last LED");
        assert!(!allowed(&packet(&[REPORT_ID, 0x58, 0xCA, 0x02, 57])), "714 + 57 = 771: past the end");
        assert!(!allowed(&packet(&[REPORT_ID, 0x58, 0x00, 0x03, 3])), "past the 256-LED string");
        assert!(!allowed(&packet(&[REPORT_ID, 0x59, 0, 0, 60])), "more than 19 LEDs");
        assert!(!allowed(&packet(&[REPORT_ID, 0x59, 0, 0, 4])), "partial LED");
        let colors = vec![Rgb::new(1, 2, 3); 256];
        assert!(direct_packets(0x59, &colors, [0, 1, 2]).iter().all(|p| allowed(p)));
    }

    #[test]
    fn flash_and_calibration_writes_are_blocked() {
        for bad in [
            &[0xCC, 0x5E, 0x00][..], // save to flash
            &[0xCC, 0x47, 0x01],     // persistent lighting
            &[0xCC, 0x47, 0x00],
            &[0xCC, 0x33, 0x00],     // calibration write
            &[0xCC, 0x3C, 0x00],     // strip scan
            &[0xCC, 0x28, 0x00, 0x00], // zero-mask commit
            &[0xCC, 0x31, 0x01],     // audio mode on
            &[0x00, 0x60],           // wrong report id
        ] {
            assert!(!allowed(bad), "{bad:02x?}");
        }
        // A non-static onboard effect is refused too.
        let mut flash = board_packet(Rgb::WHITE);
        flash[11] = 0x03;
        assert!(!allowed(&flash));
    }

    #[test]
    fn board_packet_layout() {
        let p = board_packet(Rgb::new(10, 20, 30));
        assert_eq!(&p[..6], &[0xCC, 0x20, 0xFF, 0, 0, 0]);
        assert_eq!(&p[11..18], &[0x01, 0xFF, 0x00, 30, 20, 10, 0]); // static, B G R
        assert!(allowed(&p));
    }

    #[test]
    fn direct_packets_and_order() {
        let colors: Vec<Rgb> = (0..21u8).map(|i| Rgb::new(i, 100, 200)).collect();
        let p = direct_packets(0x58, &colors, [1, 0, 2]); // GRB
        assert_eq!(p.len(), 2);
        assert_eq!(&p[0][..8], &[0xCC, 0x58, 0, 0, 57, 100, 0, 200]);
        assert_eq!(&p[1][..8], &[0xCC, 0x58, 57, 0, 6, 100, 19, 200]);
        assert!(p.iter().all(|x| allowed(x)));
    }

    #[test]
    fn calibration_and_counts() {
        assert_eq!(order_from_calibration(0x0000_0102), [0, 1, 2]); // "RGB": R→0, G→1, B→2
        assert_eq!(order_from_calibration(0), [1, 0, 2]); // unset → GRB
        assert_eq!(order_from_calibration(0x0505), [1, 0, 2]); // invalid → GRB
        assert_eq!((count_code(24), count_code(48), count_code(200)), (0, 1, 2));
    }
}
