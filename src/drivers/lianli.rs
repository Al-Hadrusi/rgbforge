//! Lian Li UNI HUB lighting: SL V2, AL V2 and SL V2 (v0.5) hubs for UNI FAN fans.
//!
//! Protocol (from OpenRGB and uni-sync; 353-byte output reports, report id 0xE0, interface
//! with vendor usage page 0xFF72):
//! * `E0 10 60 (ch<<4 | fans)`        start channel `ch` (0-3) with `fans` fans (1-6)
//! * `E0 (30+ch) <LEDs × R,B,G>`      colours (16 LEDs per fan)
//! * `E0 (10+ch) 01 02 00 00`         commit: static, normal speed, brightness 100%
//!
//! FAN SAFETY: fan speed (`E0 2x …`), fan PWM-sync (`E0 10 62 …`) and motherboard ARGB
//! sync (`E0 10 61 …`) share the same prefix as the lighting commands (only the third
//! byte differs). Every packet goes through [`allowed`], which checks every command byte,
//! so those can never be sent. Nothing is saved to the hub.

use std::io;

use super::Driver;
use crate::hidraw::{HidDevice, HidInfo};
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

pub const USAGE_PAGE: u16 = 0xFF72;
const REPORT: usize = 353;
const CHANNELS: u8 = 4;
const LEDS_PER_FAN: u16 = 16;
const MAX_FANS: u16 = 6;
/// OpenRGB caps each LED's R+G+B at 460 (the hub's power budget).
const MAX_SUM: u32 = 460;

/// Byte-level allow-list: only the three lighting packets above, with exactly these values.
pub(crate) fn allowed(pkt: &[u8]) -> bool {
    match pkt {
        [0xE0, 0x10, 0x60, n, ..] => (n >> 4) < CHANNELS && (1..=6).contains(&(n & 0x0F)),
        [0xE0, 0x10..=0x13, 0x01, 0x02, 0x00, 0x00, ..] => true,
        [0xE0, 0x30..=0x33, data @ ..] => colour_data_ok(data),
        _ => false,
    }
}

/// Colour data: at most 96 LEDs (6 fans x 16), each within the hub's power budget, and
/// nothing but zero padding after them. This is the protocol's only hardware-protection
/// limit, so the guard enforces it, not just the packet builder.
fn colour_data_ok(data: &[u8]) -> bool {
    let max = usize::from(LEDS_PER_FAN * MAX_FANS) * 3;
    let (leds, rest) = data.split_at(data.len().min(max));
    data.len() + 2 <= REPORT
        && leds.chunks(3).all(|t| t.iter().map(|v| u32::from(*v)).sum::<u32>() <= MAX_SUM)
        && rest.iter().all(|b| *b == 0)
}

fn limit(c: Rgb) -> Rgb {
    let sum = u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
    if sum <= MAX_SUM {
        return c;
    }
    let s = |v: u8| u8::try_from(u32::from(v) * MAX_SUM / sum).unwrap_or(u8::MAX);
    Rgb::new(s(c.r), s(c.g), s(c.b))
}

fn packet(bytes: impl IntoIterator<Item = u8>) -> Vec<u8> {
    let mut p = vec![0u8; REPORT];
    for (dst, src) in p.iter_mut().zip(bytes) {
        *dst = src;
    }
    p
}

/// The three packets that show `colors` on channel `ch`.
pub(crate) fn frame_packets(ch: u8, colors: &[Rgb]) -> [Vec<u8>; 3] {
    let max = usize::from(LEDS_PER_FAN * MAX_FANS);
    let leds = colors.len().min(max);
    let fans = u8::try_from(leds.div_ceil(usize::from(LEDS_PER_FAN)).clamp(1, usize::from(MAX_FANS))).unwrap_or(1);
    let rbg = colors.iter().take(leds).flat_map(|c| {
        let c = limit(*c);
        [c.r, c.b, c.g]
    });
    [
        packet([0xE0, 0x10, 0x60, (ch << 4) | fans]),
        packet([0xE0, 0x30 + ch].into_iter().chain(rbg)),
        packet([0xE0, 0x10 + ch, 0x01, 0x02, 0x00, 0x00]),
    ]
}

pub struct LianLi {
    dev: HidDevice,
    info: DeviceInfo,
}

impl LianLi {
    pub fn open(hid: &HidInfo, name: &str) -> io::Result<Self> {
        let dev = HidDevice::open(hid)?;
        let zones = (1..=CHANNELS)
            .map(|i| ZoneInfo {
                name: format!("Fan group {i}"),
                leds: 3 * LEDS_PER_FAN,
                resizable: true,
                max_leds: LEDS_PER_FAN * MAX_FANS,
            })
            .collect();
        Ok(Self {
            dev,
            info: DeviceInfo {
                id: format!("lianli:{:04x}:{:04x}:{}", hid.vid, hid.pid, hid.phys),
                name: name.into(),
                kind: Kind::Fans,
                zones,
                demo: false,
                experimental: true,
                detail: "16 LEDs per fan; set each group to fans × 16. If nothing lights, the hub may be set to follow the motherboard (change that in Lian Li's software once)."
                    .into(),
            },
        })
    }
}

impl Driver for LianLi {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone) {
            z.leds = leds.min(z.max_leds);
        }
    }

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        for (ch, colors) in (0..CHANNELS).zip(frame) {
            if colors.is_empty() {
                continue;
            }
            for p in frame_packets(ch, colors) {
                if !allowed(&p) {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked non-lighting Lian Li command"));
                }
                self.dev.write_report(&p)?;
            }
        }
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.05
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn guard_enforces_the_led_power_budget() {
        let mut p = vec![0u8; REPORT];
        p[0] = 0xE0;
        p[1] = 0x31;
        p[2..5].copy_from_slice(&[200, 200, 60]); // sum 460: at the limit
        assert!(allowed(&p));
        p[4] = 61; // sum 461: over budget
        assert!(!allowed(&p));
        p[4] = 60;
        p[2 + 96 * 3] = 1; // a 97th LED
        assert!(!allowed(&p));
        assert!(!allowed(&vec![0xE0; REPORT + 1]), "oversized report");
        // Full-white frames from the builder are scaled down and pass.
        let white = vec![Rgb::new(255, 255, 255); 96];
        assert!(frame_packets(3, &white).iter().all(|p| allowed(p)));
    }

    #[test]
    fn fan_and_sync_commands_are_blocked() {
        // Fan speed (E0 2x), fan PWM sync (E0 10 62), motherboard ARGB sync (E0 10 61), other effects.
        for bad in [&[0xE0, 0x20, 0x00, 0x10][..], &[0xE0, 0x10, 0x62, 0x11], &[0xE0, 0x10, 0x61, 0x01], &[0xE0, 0x10, 0x05, 0x02, 0, 0], &[0xE0, 0x10, 0x60, 0x47], &[0xE0, 0x10, 0x60, 0x07], &[0x00]] {
            assert!(!allowed(bad), "{bad:02x?}");
        }
    }

    #[test]
    fn packets_match_protocol() {
        let colors: Vec<Rgb> = (0..40u8).map(|i| Rgb::new(i, 50, 60)).collect();
        let [start, data, commit] = frame_packets(2, &colors);
        assert_eq!(&start[..4], &[0xE0, 0x10, 0x60, 0x23]); // channel 2, 3 fans
        assert_eq!(&data[..5], &[0xE0, 0x32, 0, 60, 50]); // R, B, G
        assert_eq!(&commit[..6], &[0xE0, 0x12, 0x01, 0x02, 0x00, 0x00]);
        assert!([&start, &data, &commit].iter().all(|p| p.len() == 353 && allowed(p)));
    }

    #[test]
    fn power_limit() {
        let c = limit(Rgb::new(255, 255, 255));
        assert!(u32::from(c.r) + u32::from(c.g) + u32::from(c.b) <= MAX_SUM);
        assert_eq!(limit(Rgb::new(255, 0, 0)), Rgb::new(255, 0, 0));
    }
}
