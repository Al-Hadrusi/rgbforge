//! G.Skill Trident Z5 RGB (ENE "DIMM_LED" RGB controller on each DDR5 stick), over the
//! Intel chipset SMBus. THIS IS THE MOST DANGEROUS DRIVER IN RGB FORGE, so it is built as
//! a set of hard gates; if any gate is not clearly passed, it stops before writing anything.
//!
//! What has bricked RAM in the past (OpenRGB issues #4934 et al., vendor tools):
//!  * writes to the SPD memory-settings chip at 0x50-0x57 (incl. MR11 page-select writes),
//!  * blind writes to move RGB controllers to other addresses ("remapping"),
//!  * saving to the RGB controller's own memory, probing other chips (PMICs at 0x48-0x4F),
//!  * several programs using the bus at once.
//!
//! What this driver does instead:
//!  1. Refuses unless the chipset's SPD Write Disable lock is confirmed ON (hardware then blocks
//!     every write to 0x50-0x57 until reboot, whatever any software does).
//!  2. Refuses if OpenRGB is running.
//!  3. Only ever addresses 0x70-0x77 (checked at compile time to exclude 0x48-0x5F).
//!  4. Identifies a chip with plain reads first (its 0xA0-0xAF signature), then by its name string,
//!     which must be one OpenRGB also knows. Unknown → stop.
//!  5. Never remaps addresses. Identical sticks sharing an address receive identical writes.
//!  6. Every write goes through one guard that allows only: direct-mode flag (0/1), apply = 0x01,
//!     and the direct colour registers. The save command (apply = 0xAA) and the address/slot
//!     registers can never be written.
//!  7. Off unless the user switches it on. At most 4 updates per second.

use std::fs;
use std::io;
use std::path::Path;

use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;

use super::Driver;
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

pub const BUS_NAME_PREFIX: &str = "SMBus I801 adapter";
const ADDRESSES: std::ops::RangeInclusive<u16> = 0x70..=0x77;
// Compile-time guarantee: the address range can never include PMICs (0x48-0x4F) or SPD (0x50-0x5F).
const _: () = assert!(*ADDRESSES.start() > 0x5F);

const REG_NAME: u16 = 0x1000;
const REG_CONFIG_LED_COUNT: u16 = 0x1C02;
const REG_DIRECT_V1: u16 = 0x8000;
const REG_DIRECT_V2: u16 = 0x8100;
const REG_DIRECT_MODE: u16 = 0x8020;
const REG_APPLY: u16 = 0x80A0;
const APPLY_VAL: u8 = 0x01;
const MAX_LEDS: u8 = 20;

/// Name strings OpenRGB drives, and their colour-register version.
const KNOWN: &[(&str, u16)] = &[("DIMM_LED-0102", REG_DIRECT_V1), ("DIMM_LED-0103", REG_DIRECT_V1), ("AUDA0-E6K5-0101", REG_DIRECT_V2)];

fn err(e: impl std::error::Error + Send + Sync + 'static) -> io::Error {
    io::Error::other(e)
}

fn refuse(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, msg.into())
}

// ---------------------------------------------------------------------------
// Gates (read-only)
// ---------------------------------------------------------------------------

/// Bit 4 of the SMBus controller's HOSTC config register (PCI offset 0x40): SPD Write Disable.
/// Once firmware sets it, it can't be cleared until reset.
const HOSTC_SPD_WD: u8 = 0x10;
/// Where the root-run udev rule (see `rgbforge --print-udev-rules`) copies the HOSTC byte at boot.
pub const HOSTC_DIR: &str = "/run/rgbforge";

/// Parse the `od` output written by the udev rule, e.g. " 11\n".
pub fn parse_hostc(text: &str) -> Option<u8> {
    u8::from_str_radix(text.trim(), 16).ok()
}

/// Is the chipset's SPD Write Disable lock on? `None` = couldn't find out.
pub fn spd_write_disabled() -> Option<bool> {
    // 1. Authoritative: the hardware register itself, captured by root at boot.
    // There is deliberately no fallback: kernel log text can be influenced (e.g. by USB
    // device names), so only the register value itself counts as evidence.
    let captured = bus_pci_name()
        .and_then(|pci| root_owned_read(&Path::new(HOSTC_DIR).join(format!("smbus-hostc-{pci}"))))
        .and_then(|t| parse_hostc(&t))?;
    Some(captured & HOSTC_SPD_WD != 0)
}

/// Read a small file only if it and its folder are root-owned, not links, and writable by
/// nobody else, so no other account can plant or alter the safety evidence.
fn root_owned_read(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let trusted = |m: &fs::Metadata| m.uid() == 0 && m.mode() & 0o022 == 0;
    let dir = fs::symlink_metadata(path.parent()?).ok()?;
    let file = fs::symlink_metadata(path).ok()?;
    (dir.is_dir() && trusted(&dir) && file.file_type().is_file() && trusted(&file) && file.len() <= 64)
        .then(|| fs::read_to_string(path).ok())
        .flatten()
}

/// PCI name (e.g. "0000:00:1f.4") of the chipset SMBus controller.
fn bus_pci_name() -> Option<String> {
    let bus = find_bus(Path::new("/sys/bus/i2c/devices"))?;
    let target = fs::read_link(format!("/sys/bus/i2c/devices/i2c-{bus}/device")).ok()?;
    let name = target.file_name()?.to_str()?.to_string();
    // Only accept something that looks like a PCI address; it becomes part of a file name.
    name.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.').then_some(name)
}

pub fn openrgb_running() -> bool {
    super::trusted_process_names().iter().any(|n| n == "openrgb")
}

/// The single chipset SMBus adapter number. Ambiguity → None.
pub fn find_bus(sys: &Path) -> Option<u32> {
    let mut found = fs::read_dir(sys).ok()?.flatten().filter_map(|e| {
        let n: u32 = e.file_name().to_str()?.strip_prefix("i2c-")?.parse().ok()?;
        let name = fs::read_to_string(e.path().join("name")).ok()?;
        name.starts_with(BUS_NAME_PREFIX).then_some(n)
    });
    let first = found.next()?;
    if found.next().is_some() { None } else { Some(first) }
}

/// Plain SMBus reads of registers 0xA0-0xAF; ENE controllers answer 0x00..0x0F.
fn has_signature(dev: &mut LinuxI2CDevice) -> bool {
    (0u8..16).all(|i| dev.smbus_read_byte_data(0xA0 + i).ok() == Some(i))
}

fn open(bus: u32, addr: u16) -> io::Result<LinuxI2CDevice> {
    debug_assert!(ADDRESSES.contains(&addr));
    if !ADDRESSES.contains(&addr) {
        return Err(refuse("address outside the allowed RGB range"));
    }
    // I2C_SLAVE (never FORCE): the kernel refuses if a driver owns the address.
    LinuxI2CDevice::new(format!("/dev/i2c-{bus}"), addr).map_err(|e| match e {
        i2cdev::linux::LinuxI2CError::Io(io) => io,
        other => err(other),
    })
}

/// Text for `rgbforge --ram-check`. Bus traffic: only plain reads of 0xA0-0xAF at 0x70-0x77.
pub fn check_report() -> String {
    let mut r = String::from("RGB Forge RAM check (read-only: no writes are sent to any chip)\n\n");
    let wd = spd_write_disabled();
    r.push_str(match wd {
        Some(true) => "SPD Write Disable: ON ✔ (the chipset blocks all writes to the memory-settings chips)\n",
        Some(false) => "SPD Write Disable: OFF ✘ RAM lighting will stay disabled. This is a hard safety requirement.\n",
        None => "SPD Write Disable: couldn't check. Re-run ./install.sh (it adds the boot-time check), then reboot ✘\n",
    });
    if openrgb_running() {
        r.push_str("OpenRGB is running ✘ (close it: two programs must never use the bus at once)\n");
    }
    let Some(bus) = find_bus(Path::new("/sys/bus/i2c/devices")) else {
        r.push_str("Chipset SMBus (i2c-i801): not found, or more than one ✘\n");
        return r;
    };
    r.push_str(&format!("Chipset SMBus: i2c-{bus} ✔\n"));
    r.push_str(&ene_section(bus, wd));
    r.push_str(&super::ddr4_check::report(bus));
    r
}

/// The G.Skill/ENE part of the check (needs the SPD Write Disable lock).
fn ene_section(bus: u32, wd: Option<bool>) -> String {
    let mut r = String::from("\nG.Skill / ENE RGB check:\n");
    if wd != Some(true) {
        r.push_str("  Skipped: SPD Write Disable isn't confirmed.\n");
        return r;
    }
    let mut any = false;
    for addr in ADDRESSES {
        match open(bus, addr) {
            Ok(mut dev) => {
                if has_signature(&mut dev) {
                    any = true;
                    r.push_str(&format!("  {addr:#04x}: ENE RGB controller signature found ✔\n"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                r.push_str("  No permission for the SMBus yet (re-run ./install.sh, log out and back in).\n");
                return r;
            }
            Err(_) => {}
        }
    }
    if !any {
        r.push_str("  No RAM RGB controllers answered at 0x70-0x77.\n");
    }
    r
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

struct Ene {
    dev: LinuxI2CDevice,
    direct_base: u16,
    leds: u8,
}

impl Ene {
    /// Registers the pointer may ever be set to: the name and LED-count reads, direct mode,
    /// apply and the colour registers. Never the slot/address registers (0x80F8/0x80F9).
    fn pointer_allowed(&self, reg: u16) -> bool {
        pointer_ok(reg, self.direct_base, self.leds)
    }

    fn set_pointer(&mut self, reg: u16) -> io::Result<()> {
        if !self.pointer_allowed(reg) {
            return Err(refuse(format!("blocked register select {reg:#06x}")));
        }
        // ENE expects the register address high byte first.
        self.dev.smbus_write_word_data(0x00, reg.swap_bytes()).map_err(err)
    }

    fn read(&mut self, reg: u16) -> io::Result<u8> {
        self.set_pointer(reg)?;
        self.dev.smbus_read_byte_data(0x81).map_err(err)
    }

    fn allowed(&self, reg: u16, val: u8) -> bool {
        let colours = self.direct_base..self.direct_base.saturating_add(u16::from(self.leds) * 3);
        (reg == REG_DIRECT_MODE && val <= 1) || (reg == REG_APPLY && val == APPLY_VAL) || colours.contains(&reg)
    }

    /// The ONLY write path. Anything outside the allow-list is refused.
    fn write(&mut self, reg: u16, val: u8) -> io::Result<()> {
        if !self.allowed(reg, val) {
            return Err(refuse(format!("blocked unsafe write {reg:#06x}={val:#04x}")));
        }
        self.set_pointer(reg)?;
        self.dev.smbus_write_byte_data(0x01, val).map_err(err)
    }
}

/// Registers the ENE pointer may be set to (see `Ene::pointer_allowed`).
pub(crate) fn pointer_ok(reg: u16, direct_base: u16, leds: u8) -> bool {
    let colours = direct_base..direct_base.saturating_add(u16::from(leds) * 3);
    (REG_NAME..REG_NAME + 16).contains(&reg)
        || reg == REG_CONFIG_LED_COUNT
        || reg == REG_DIRECT_MODE
        || reg == REG_APPLY
        || (leds > 0 && colours.contains(&reg))
}

pub struct EneRam {
    chip: Ene,
    info: DeviceInfo,
    initialised: bool,
    original_direct: u8,
}

pub fn detect() -> (Vec<EneRam>, Vec<String>) {
    let mut problems = Vec::new();
    match spd_write_disabled() {
        Some(true) => {}
        Some(false) => {
            problems.push("RAM lighting is on, but your chipset's SPD Write Disable lock is off, so RGB Forge won't touch the RAM.".into());
            return (Vec::new(), problems);
        }
        None => {
            problems.push("RAM lighting is on, but RGB Forge couldn't confirm the SPD Write Disable lock, so it won't touch the RAM. Run `rgbforge --ram-check`.".into());
            return (Vec::new(), problems);
        }
    }
    if openrgb_running() {
        problems.push("RAM lighting paused: close OpenRGB first (two programs must never share the bus).".into());
        return (Vec::new(), problems);
    }
    let Some(bus) = find_bus(Path::new("/sys/bus/i2c/devices")) else {
        problems.push("RAM lighting is on, but the chipset SMBus wasn't found.".into());
        return (Vec::new(), problems);
    };
    let mut out = Vec::new();
    for addr in ADDRESSES {
        match EneRam::open(bus, addr) {
            Ok(Some(d)) => out.push(d),
            Ok(None) => {}
            Err(e) => problems.push(format!("RAM at {addr:#04x} not used: {e}")),
        }
    }
    (out, problems)
}

impl EneRam {
    fn open(bus: u32, addr: u16) -> io::Result<Option<Self>> {
        let Ok(mut dev) = open(bus, addr) else { return Ok(None) };
        if !has_signature(&mut dev) {
            return Ok(None); // nothing (or something else) here: never touched beyond plain reads
        }
        let mut chip = Ene { dev, direct_base: 0, leds: 0 };
        let name: String = (0..16u16)
            .map(|i| chip.read(REG_NAME + i))
            .collect::<io::Result<Vec<u8>>>()?
            .into_iter()
            .take_while(|b| *b != 0)
            .filter(|b| b.is_ascii_graphic())
            .map(char::from)
            .collect();
        let Some((_, base)) = KNOWN.iter().find(|(n, _)| *n == name) else {
            return Err(refuse(format!("unknown RGB chip \"{name}\"; not touching it")));
        };
        let leds = chip.read(REG_CONFIG_LED_COUNT)?;
        if leds == 0 || leds > MAX_LEDS {
            return Err(refuse(format!("implausible LED count {leds}; not touching it")));
        }
        let original_direct = chip.read(REG_DIRECT_MODE)?;
        chip.direct_base = *base;
        chip.leds = leds;
        let shared = if addr == 0x77 { " (all sticks together)" } else { "" };
        Ok(Some(Self {
            chip,
            info: DeviceInfo {
                id: format!("ene-ram:{bus}:{addr:02x}"),
                name: format!("G.Skill RAM{shared}"),
                kind: Kind::Ram,
                zones: vec![ZoneInfo { name: "Light bar".into(), leds: u16::from(leds), resizable: false, max_leds: u16::from(leds) }],
                demo: false,
                experimental: true,
                detail: format!("RGB chip {name} at {addr:#04x} on i2c-{bus}"),
            },
            initialised: false,
            original_direct: original_direct.min(1),
        }))
    }
}

impl Drop for EneRam {
    /// Hand the lights back to the RAM's own effect if it wasn't in direct mode before.
    fn drop(&mut self) {
        if self.initialised && self.original_direct == 0 {
            let _ = self.chip.write(REG_DIRECT_MODE, 0);
            let _ = self.chip.write(REG_APPLY, APPLY_VAL);
        }
    }
}

impl Driver for EneRam {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, _zone: usize, _leds: u16) {}

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        if !self.initialised {
            self.chip.write(REG_DIRECT_MODE, 1)?;
            self.chip.write(REG_APPLY, APPLY_VAL)?;
            self.initialised = true;
        }
        let colors = frame.first().cloned().unwrap_or_default();
        let base = self.chip.direct_base;
        for (i, c) in colors.iter().take(usize::from(self.chip.leds)).enumerate() {
            let reg = base.saturating_add(u16::try_from(i * 3).unwrap_or(u16::MAX));
            // ENE colour order is red, blue, green.
            for (k, v) in [c.r, c.b, c.g].into_iter().enumerate() {
                self.chip.write(reg.saturating_add(u16::try_from(k).unwrap_or(0)), v)?;
            }
        }
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.25
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn address_range_excludes_spd_and_pmic() {
        for a in ADDRESSES {
            assert!(!(0x48..=0x5F).contains(&a));
        }
    }

    #[test]
    fn evidence_file_must_be_root_owned_and_unwritable_by_others() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("rgbforge-hostc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let good = dir.join("good");
        fs::write(&good, " 11\n").unwrap();
        fs::set_permissions(&good, fs::Permissions::from_mode(0o644)).unwrap();
        let writable = dir.join("writable");
        fs::write(&writable, " 11\n").unwrap();
        fs::set_permissions(&writable, fs::Permissions::from_mode(0o666)).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&good, &link).unwrap();
        assert_eq!(root_owned_read(&writable), None, "world-writable file must be refused");
        assert_eq!(root_owned_read(&link), None, "symlink must be refused");
        let running_as_root = fs::metadata(&good).map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0).unwrap_or(false);
        assert_eq!(root_owned_read(&good).is_some(), running_as_root, "accepted only when root-owned");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert_eq!(root_owned_read(&good), None, "writable folder must be refused");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn register_select_never_reaches_address_or_save_registers() {
        // Before identification (no LEDs known): only name and LED-count reads, mode, apply.
        assert!(pointer_ok(REG_NAME + 5, 0, 0) && pointer_ok(REG_CONFIG_LED_COUNT, 0, 0));
        assert!(!pointer_ok(REG_DIRECT_V1, 0, 0));
        // After identification: the stick's own colour registers too, nothing beyond them.
        assert!(pointer_ok(REG_DIRECT_V1 + 29, REG_DIRECT_V1, 10));
        assert!(!pointer_ok(REG_DIRECT_V1 + 30, REG_DIRECT_V1, 10));
        // Slot index / I2C address registers (address remapping) can never be selected.
        for reg in [0x80F8, 0x80F9, 0x1030, 0x0000, 0xFFFF] {
            assert!(!pointer_ok(reg, REG_DIRECT_V1, 20), "{reg:#06x}");
        }
    }

    #[test]
    fn spd_write_disable_bit() {
        // The user's real register value: 0x11 = SMBus enabled + SPD Write Disable.
        assert_eq!(parse_hostc(" 11\n").map(|b| b & HOSTC_SPD_WD != 0), Some(true));
        assert_eq!(parse_hostc(" 01\n").map(|b| b & HOSTC_SPD_WD != 0), Some(false));
        assert_eq!(parse_hostc("garbage"), None);
    }

    #[test]
    fn known_names() {
        assert!(KNOWN.iter().any(|(n, _)| *n == "DIMM_LED-0103"));
        assert!(!KNOWN.iter().any(|(n, _)| n.contains("Micron")));
    }

    #[test]
    fn finds_exactly_one_chipset_bus() {
        let root = std::env::temp_dir().join(format!("rgbforge-i2c-{}", std::process::id()));
        for (n, name) in [(0, "SMBus I801 adapter at 0000:00:1f.4"), (5, "AMDGPU DM i2c OEM bus"), (9, "AMDGPU SMU 0")] {
            let d = root.join(format!("i2c-{n}"));
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("name"), format!("{name}\n")).unwrap();
        }
        assert_eq!(find_bus(&root), Some(0));
        let d = root.join("i2c-12");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("name"), "SMBus I801 adapter at 0000:00:1f.5\n").unwrap();
        assert_eq!(find_bus(&root), None, "two candidate buses must be refused");
        fs::remove_dir_all(&root).ok();
    }
}
