//! Read-only identification of Corsair and Kingston FURY DDR4 RGB controllers, for
//! `rgbforge --ram-check`. No driver uses this yet: it exists so owners can show whether
//! these chips identify themselves with reads alone (both vendors' known detection
//! sequences write before identifying, which RGB Forge doesn't allow).
//!
//! Rules: no writes of any kind; only SMBus "read byte/word data" requests; only
//! addresses 0x58-0x5F (no JEDEC device lives there on DDR4); only on systems where the
//! kernel shows DDR4 memory; the memory-settings (SPD) chips are never read or written.

use std::fs;
use std::io;
use std::path::Path;

use i2cdev::core::I2CDevice;
use i2cdev::linux::LinuxI2CDevice;

const ADDRESSES: std::ops::RangeInclusive<u16> = 0x58..=0x5F;
// Compile-time guarantee: never SPD (0x50-0x57), PMIC (0x48-0x4F) or thermal sensors (0x18-0x1F).
const _: () = assert!(*ADDRESSES.start() > 0x57);

/// DDR4 confirmed by the kernel's DDR4 SPD driver having devices (a directory listing only).
pub fn ddr4_present(sys: &Path) -> bool {
    fs::read_dir(sys.join("bus/i2c/drivers/ee1004"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().contains("-005"))
}

/// Corsair's signature registers: 0x43 ∈ {1A,1B,1C} and 0x44 ∈ {01,03,04} (from OpenRGB).
pub(crate) fn corsair_signature(r43: u8, r44: u8) -> bool {
    matches!(r43, 0x1A..=0x1C) && matches!(r44, 0x01 | 0x03 | 0x04)
}

/// Kingston's signature: high bytes of words 1-4 spell "FURY" (from OpenRGB).
pub(crate) fn kingston_signature(words: &[u16]) -> bool {
    let hi: Vec<u8> = words.iter().map(|w| w.to_be_bytes()[0]).collect();
    hi == b"FURY"
}

pub(crate) fn kingston_model(word6: u16) -> &'static str {
    match word6.to_be_bytes()[0] {
        0x21 => "FURY Beast RGB DDR4 (white)",
        0x23 => "FURY Beast RGB DDR4",
        _ => "unknown FURY model",
    }
}

fn open(bus: u32, addr: u16) -> io::Result<LinuxI2CDevice> {
    if !ADDRESSES.contains(&addr) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "address outside the read-only check range"));
    }
    LinuxI2CDevice::new(format!("/dev/i2c-{bus}"), addr).map_err(|e| match e {
        i2cdev::linux::LinuxI2CError::Io(io) => io,
        other => io::Error::other(other),
    })
}

/// Report section for `--ram-check`.
pub fn report(bus: u32) -> String {
    let mut r = String::from("\nDDR4 RGB check (Corsair / Kingston FURY; reads only, 0x58-0x5F only):\n");
    if !ddr4_present(Path::new("/sys")) {
        r.push_str("  Skipped: the kernel doesn't show DDR4 memory on this PC.\n");
        return r;
    }
    let mut answered = false;
    for addr in ADDRESSES {
        let Ok(mut dev) = open(bus, addr) else { continue };
        let b43 = dev.smbus_read_byte_data(0x43).ok();
        let b44 = dev.smbus_read_byte_data(0x44).ok();
        let words: Vec<Option<u16>> = (1u8..=4).chain([6]).map(|reg| dev.smbus_read_word_data(reg).ok()).collect();
        if b43.is_none() && b44.is_none() && words.iter().all(Option::is_none) {
            continue; // nothing at this address
        }
        answered = true;
        let fmt_b = |v: Option<u8>| v.map_or("--".to_string(), |v| format!("{v:02x}"));
        let fmt_w = |v: &Option<u16>| v.map_or("----".to_string(), |v| format!("{v:04x}"));
        r.push_str(&format!(
            "  {addr:#04x}: reg43={} reg44={} words1-4,6={}\n",
            fmt_b(b43),
            fmt_b(b44),
            words.iter().map(fmt_w).collect::<Vec<_>>().join(" ")
        ));
        if let (Some(a), Some(b)) = (b43, b44)
            && corsair_signature(a, b)
        {
            r.push_str("        → Corsair RGB controller signature found with reads only ✔\n");
        }
        let first4: Vec<u16> = words.iter().take(4).flatten().copied().collect();
        if first4.len() == 4 && kingston_signature(&first4) {
            let model = words.get(4).copied().flatten().map_or("unknown FURY model", kingston_model);
            r.push_str(&format!("        → Kingston FURY signature found with reads only ✔ ({model})\n"));
        }
    }
    if !answered {
        r.push_str("  Nothing answered at 0x58-0x5F.\n");
    }
    r.push_str("  Please share this output: it shows whether these sticks can be identified safely.\n");
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        assert!(corsair_signature(0x1B, 0x03));
        assert!(!corsair_signature(0x1D, 0x03) && !corsair_signature(0x1A, 0x02) && !corsair_signature(0xFF, 0xFF));
        assert!(kingston_signature(&[0x4600, 0x5512, 0x52FF, 0x5900]));
        assert!(!kingston_signature(&[0x4600, 0x5500, 0x5200]));
        assert!(!kingston_signature(&[0xFFFF; 4]));
        assert_eq!(kingston_model(0x2300), "FURY Beast RGB DDR4");
    }

    #[test]
    fn ddr4_detection_is_a_directory_listing() {
        let root = std::env::temp_dir().join(format!("rgbforge-ddr4-{}", std::process::id()));
        let d = root.join("bus/i2c/drivers/ee1004");
        fs::create_dir_all(&d).unwrap_or_default();
        assert!(!ddr4_present(&root));
        fs::create_dir_all(d.join("12-0050")).unwrap_or_default();
        assert!(ddr4_present(&root));
        fs::remove_dir_all(&root).ok();
    }
}
