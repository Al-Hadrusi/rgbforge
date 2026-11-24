//! Sapphire Nitro+ Radeon RX 9070 XT ("Nitro Glow V3") lighting, over the GPU's I2C bus.
//!
//! Facts used (from OpenRGB's published support for this exact card):
//! * Card: PCI 1002:7550, subsystem 1DA2:E489.
//! * Lighting chip at I2C address 0x28.
//! * Registers: 0x0F external-sync control, 0x10 mode (0x06 = custom colour),
//!   0x1A/0x1B/0x1C red/green/blue. Plain SMBus byte writes.
//!
//! Safety rules, stricter than the USB drivers because a GPU's I2C buses also carry
//! things like its voltage controller:
//! * Off unless the user explicitly switches it on in the app.
//! * Only for the exact PCI vendor/device/subsystem above; any other card is never touched.
//! * Only the GPU's dedicated RGB bus (the amdgpu "OEM" I2C adapter) is opened. The
//!   card's other buses are never opened, scanned or probed.
//! * Exactly one address (0x28), opened with the normal I2C_SLAVE request, which the
//!   kernel refuses if a driver already owns that address (never the FORCE variant).
//! * One identity read: the mode register must hold a known mode value, or we stop.
//! * Only the registers listed above are ever written. No save/flash commands exist.
//! * At most 10 updates per second.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;

use super::Driver;
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

pub const VENDOR: u16 = 0x1002;
pub const DEVICE: u16 = 0x7550;
pub const SUB_VENDOR: u16 = 0x1DA2;
pub const SUB_DEVICE: u16 = 0xE489;
pub const NAME: &str = "Sapphire Nitro+ RX 9070 XT";

pub const RGB_BUS_NAME: &str = "AMDGPU DM i2c OEM bus";
/// Kernel that first exposes the RGB bus, and the first with the fix for a crash when the
/// GPU driver unloads while the bus is in use (kernel commit "remove oem i2c adapter on finish").
const MIN_KERNEL: (u32, u32) = (6, 15);
const GOOD_KERNEL: (u32, u32) = (6, 17);
const ADDR: u16 = 0x28;
const REG_EXTERNAL_CONTROL: u8 = 0x0F;
const REG_MODE: u8 = 0x10;
const REG_RED: u8 = 0x1A;
const REG_GREEN: u8 = 0x1B;
const REG_BLUE: u8 = 0x1C;
const MODE_CUSTOM: u8 = 0x06;
/// Modes documented for this chip: 0x00..=0x07 and 0xFF (external sync).
fn known_mode(v: u8) -> bool {
    v <= 0x07 || v == 0xFF
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub number: u32,
    pub name: String,
}

impl Adapter {
    /// The dedicated RGB bus amdgpu exposes since Linux 6.15. This exact name is the only
    /// bus with the RGB chip on it. Touching the card's other buses (notably "AMDGPU SMU 0/1")
    /// is what froze systems running older OpenRGB; OpenRGB fixed it the same way, by
    /// accepting only this name (OpenRGB MR !2950).
    pub fn is_rgb_bus(&self) -> bool {
        self.name.starts_with(RGB_BUS_NAME) && !self.name.to_ascii_uppercase().contains("SMU")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMatch {
    pub pci: String,
    pub adapters: Vec<Adapter>,
}

impl CardMatch {
    pub fn rgb_bus(&self) -> Option<&Adapter> {
        let mut buses = self.adapters.iter().filter(|a| a.is_rgb_bus());
        let first = buses.next()?;
        // Ambiguity is a reason to stop, not to guess.
        if buses.next().is_some() { None } else { Some(first) }
    }
}

fn read_hex(p: &Path) -> Option<u16> {
    let t = fs::read_to_string(p).ok()?;
    u16::from_str_radix(t.trim().trim_start_matches("0x"), 16).ok()
}

/// Find the card by reading sysfs only. Never opens any device.
pub fn find_card(pci_root: &Path) -> Option<CardMatch> {
    let entries = fs::read_dir(pci_root).ok()?;
    for e in entries.flatten() {
        let dir = e.path();
        let ids = (
            read_hex(&dir.join("vendor")),
            read_hex(&dir.join("device")),
            read_hex(&dir.join("subsystem_vendor")),
            read_hex(&dir.join("subsystem_device")),
        );
        if ids != (Some(VENDOR), Some(DEVICE), Some(SUB_VENDOR), Some(SUB_DEVICE)) {
            continue;
        }
        let mut adapters: Vec<Adapter> = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|a| {
                let file = a.file_name();
                let number = file.to_str()?.strip_prefix("i2c-")?.parse().ok()?;
                let name = fs::read_to_string(a.path().join("name")).ok()?;
                Some(Adapter { number, name: name.trim().chars().filter(|c| !c.is_control()).collect() })
            })
            .collect();
        adapters.sort_by_key(|a| a.number);
        return Some(CardMatch { pci: e.file_name().to_string_lossy().into_owned(), adapters });
    }
    None
}

/// "6.17.2-arch1-1" → (6, 17)
pub fn kernel_version(release: &str) -> Option<(u32, u32)> {
    let mut parts = release.trim().split(|c: char| !c.is_ascii_digit());
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// Text for `rgbforge --gpu-check`. Reads sysfs only.
pub fn check_report() -> String {
    let mut r = String::from("RGB Forge GPU check (read-only: nothing is sent to the card)\n\n");
    let release = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    match kernel_version(&release) {
        Some(v) if v < MIN_KERNEL => r.push_str(&format!(
            "Kernel {} is too old: the GPU's RGB bus needs Linux {}.{} or newer. ✘\n",
            release.trim(), MIN_KERNEL.0, MIN_KERNEL.1
        )),
        Some(v) if v < GOOD_KERNEL => r.push_str(&format!(
            "Kernel {}: has the RGB bus, but lacks a fix for a related kernel crash. Update to {}.{}+ first. ⚠\n",
            release.trim(), GOOD_KERNEL.0, GOOD_KERNEL.1
        )),
        Some(_) => r.push_str(&format!("Kernel {} ✔\n", release.trim())),
        None => r.push_str("Couldn't read the kernel version.\n"),
    }
    let Some(card) = find_card(Path::new("/sys/bus/pci/devices")) else {
        r.push_str(&format!(
            "No {NAME} found (looking for PCI {VENDOR:04x}:{DEVICE:04x} subsystem {SUB_VENDOR:04x}:{SUB_DEVICE:04x}).\n"
        ));
        return r;
    };
    r.push_str(&format!("Card found at PCI {} ✔\n", card.pci));
    r.push_str("I2C buses on this card:\n");
    if card.adapters.is_empty() {
        r.push_str("  (none listed)\n");
    }
    for a in &card.adapters {
        let mark = if a.is_rgb_bus() { "  ← RGB bus" } else { "" };
        r.push_str(&format!("  i2c-{}: {}{mark}\n", a.number, a.name));
    }
    match card.rgb_bus() {
        Some(a) => {
            let node = PathBuf::from(format!("/dev/i2c-{}", a.number));
            r.push_str(&format!("\nRGB bus: i2c-{} ✔\n", a.number));
            if !node.exists() {
                r.push_str("/dev node missing: the i2c-dev module isn't loaded (re-run ./install.sh).\n");
            } else if fs::OpenOptions::new().read(true).write(true).open(&node).is_err() {
                r.push_str("No permission to use it yet (re-run ./install.sh, then log out and back in).\n");
            } else {
                r.push_str("Permission OK. Ready to enable in the app (Help → Graphics card).\n");
            }
        }
        None => r.push_str(&format!(
            "\nNo \"{RGB_BUS_NAME}\" found. Linux only creates it when the card's firmware says the\n\
             card has one (and on kernel 6.15+). RGB Forge will not touch the card's other buses,\n\
             so use the card's 3-pin ARGB sync cable with a motherboard ARGB header instead.\n"
        )),
    }
    r
}

pub struct SapphireNitro {
    dev: LinuxI2CDevice,
    info: DeviceInfo,
    initialised: bool,
    /// The card's own settings before we took over, restored when control is switched off.
    original: (u8, u8),
}

fn err(e: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::other(e)
}

/// Register write as one plain I2C message `[reg, value]`. On the wire this is identical
/// to an SMBus "write byte data", but it also works on buses that reject SMBus-style
/// requests (reported on another RX 9070 XT's RGB bus).
fn write_reg(dev: &mut LinuxI2CDevice, reg: u8, value: u8) -> io::Result<()> {
    dev.write(&[reg, value]).map_err(err)
}

/// Register read: SMBus "read byte data" first, then the plain-I2C equivalent.
fn read_reg(dev: &mut LinuxI2CDevice, reg: u8) -> io::Result<u8> {
    if let Ok(v) = dev.smbus_read_byte_data(reg) {
        return Ok(v);
    }
    dev.write(&[reg]).map_err(err)?;
    let mut buf = [0u8; 1];
    dev.read(&mut buf).map_err(err)?;
    Ok(buf[0])
}

impl SapphireNitro {
    pub fn open(card: &CardMatch) -> io::Result<Self> {
        let bus = card
            .rgb_bus()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the card's dedicated RGB bus wasn't found"))?;
        let path = format!("/dev/i2c-{}", bus.number);
        let mut dev = LinuxI2CDevice::new(&path, ADDR).map_err(|e| match e {
            i2cdev::linux::LinuxI2CError::Io(io) => io,
            other => err(other),
        })?;
        let mode = read_reg(&mut dev, REG_MODE)?;
        let external = read_reg(&mut dev, REG_EXTERNAL_CONTROL)?;
        if !known_mode(mode) || external > 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("lighting chip didn't identify as expected (mode {mode:#04x}, sync {external:#04x}); not touching it"),
            ));
        }
        Ok(Self {
            dev,
            info: DeviceInfo {
                id: format!("sapphire:{}", card.pci),
                name: NAME.into(),
                kind: Kind::Gpu,
                zones: vec![ZoneInfo { name: "Card lighting".into(), leds: 1, resizable: false, max_leds: 1 }],
                demo: false,
                experimental: true,
                detail: format!("I2C bus {} ({}), previous mode {mode:#04x}", bus.number, bus.name),
            },
            initialised: false,
            original: (external, mode),
        })
    }
}

impl Drop for SapphireNitro {
    /// Hand the card back exactly as we found it (sync setting and effect mode).
    fn drop(&mut self) {
        if self.initialised {
            let (external, mode) = self.original;
            let _ = write_reg(&mut self.dev, REG_MODE, mode);
            let _ = write_reg(&mut self.dev, REG_EXTERNAL_CONTROL, external);
        }
    }
}

impl Driver for SapphireNitro {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, _zone: usize, _leds: u16) {}

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        let c = frame.first().and_then(|z| z.first()).copied().unwrap_or_default();
        if !self.initialised {
            // Ignore the sync cable input and show our own colour.
            write_reg(&mut self.dev, REG_EXTERNAL_CONTROL, 0)?;
            write_reg(&mut self.dev, REG_MODE, MODE_CUSTOM)?;
            self.initialised = true;
        }
        write_reg(&mut self.dev, REG_RED, c.r)?;
        write_reg(&mut self.dev, REG_GREEN, c.g)?;
        write_reg(&mut self.dev, REG_BLUE, c.b)?;
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        // The chip holds its colour; only write when something changes.
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.1
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fake_pci(root: &Path, slot: &str, ids: [&str; 4], adapters: &[(u32, &str)]) {
        let d = root.join(slot);
        fs::create_dir_all(&d).unwrap();
        for (f, v) in ["vendor", "device", "subsystem_vendor", "subsystem_device"].iter().zip(ids) {
            fs::write(d.join(f), format!("{v}\n")).unwrap();
        }
        for (n, name) in adapters {
            let a = d.join(format!("i2c-{n}"));
            fs::create_dir_all(&a).unwrap();
            fs::write(a.join("name"), format!("{name}\n")).unwrap();
        }
    }

    #[test]
    fn matches_only_the_exact_card_and_only_the_rgb_bus() {
        let root = std::env::temp_dir().join(format!("rgbforge-pci-{}", std::process::id()));
        // A different Sapphire card (Pure): must be ignored.
        fake_pci(&root, "0000:01:00.0", ["0x1002", "0x7550", "0x1da2", "0x3490"], &[(3, "AMDGPU DM i2c OEM bus")]);
        fake_pci(
            &root,
            "0000:03:00.0",
            ["0x1002", "0x7550", "0x1da2", "0xe489"],
            &[(7, "AMDGPU SMU 0"), (5, "AMDGPU DM i2c hw bus 0"), (9, "AMDGPU DM i2c OEM bus")],
        );
        let card = find_card(&root).unwrap();
        fs::remove_dir_all(&root).ok();
        assert_eq!(card.pci, "0000:03:00.0");
        assert_eq!(card.adapters.len(), 3);
        assert_eq!(card.rgb_bus().unwrap().number, 9);
    }

    #[test]
    fn ambiguous_or_missing_rgb_bus_means_no_bus() {
        let two = CardMatch {
            pci: "x".into(),
            adapters: vec![
                Adapter { number: 1, name: "AMDGPU DM i2c OEM bus".into() },
                Adapter { number: 2, name: "AMDGPU DM i2c OEM bus 2".into() },
            ],
        };
        assert_eq!(two.rgb_bus(), None);
        let none = CardMatch {
            pci: "x".into(),
            adapters: vec![
                Adapter { number: 1, name: "AMDGPU SMU 0".into() },
                Adapter { number: 2, name: "AMDGPU DM i2c hw bus 0".into() },
                Adapter { number: 3, name: "AMDGPU i2c bit bus OEM 0x97".into() },
            ],
        };
        assert_eq!(none.rgb_bus(), None);
        assert!(!Adapter { number: 3, name: "AMDGPU SMU OEM".into() }.is_rgb_bus());
    }

    #[test]
    fn kernel_versions() {
        assert_eq!(kernel_version("6.17.2-arch1-1\n"), Some((6, 17)));
        assert_eq!(kernel_version("6.15.0"), Some((6, 15)));
        assert!(kernel_version("6.9.1").unwrap() < MIN_KERNEL);
        assert_eq!(kernel_version("garbage"), None);
    }

    #[test]
    fn identity_check() {
        assert!(known_mode(0x00) && known_mode(0x06) && known_mode(0xFF));
        assert!(!known_mode(0x08) && !known_mode(0x80));
    }
}
