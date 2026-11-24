//! Raw USB HID access through Linux's `/dev/hidraw*` nodes, in 100% safe Rust.
//!
//! hidraw needs no ioctls for what we do: device identity comes from sysfs text
//! files, output reports are plain `write()`s and replies are plain `read()`s.
//! That means no `unsafe`, no C library, and no kernel driver of our own.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// `O_NONBLOCK` on Linux (same value on x86, x86-64, ARM and AArch64).
const O_NONBLOCK: i32 = 0o4000;
/// Standard 64-byte report plus a report-id byte.
pub const MAX_REPORT: usize = 65;
/// Hard cap on any single write (Lian Li hubs use 353-byte reports).
pub const MAX_WRITE: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HidInfo {
    pub node: PathBuf,
    pub vid: u16,
    pub pid: u16,
    pub name: String,
    /// Physical USB path, stable across reboots for the same port.
    pub phys: String,
    /// The HID report descriptor (read from sysfs; describes the interface).
    pub descriptor: Vec<u8>,
}

impl HidInfo {
    /// Does this interface declare the given 16-bit (vendor) usage page?
    pub fn has_usage_page(&self, page: u16) -> bool {
        let [lo, hi] = page.to_le_bytes();
        self.descriptor.windows(3).any(|w| w == [0x06, lo, hi])
    }
}

/// List every hidraw device. Only *reads* sysfs; never touches the devices.
pub fn enumerate() -> Vec<HidInfo> {
    enumerate_in(Path::new("/sys/class/hidraw"), Path::new("/dev"))
}

pub fn enumerate_in(sys: &Path, dev: &Path) -> Vec<HidInfo> {
    let Ok(entries) = fs::read_dir(sys) else {
        return Vec::new();
    };
    let mut out: Vec<HidInfo> = entries
        .flatten()
        .filter_map(|e| {
            let node_name = e.file_name();
            let text = fs::read_to_string(e.path().join("device/uevent")).ok()?;
            let (vid, pid, name, phys) = parse_uevent(&text)?;
            let descriptor = fs::read(e.path().join("device/report_descriptor")).unwrap_or_default();
            Some(HidInfo { node: dev.join(node_name), vid, pid, name, phys, descriptor })
        })
        .collect();
    out.sort_by(|a, b| a.node.cmp(&b.node));
    out
}

/// Parse `HID_ID=0003:00000B05:00001867`, `HID_NAME=...`, `HID_PHYS=...`.
pub fn parse_uevent(text: &str) -> Option<(u16, u16, String, String)> {
    let mut ids = None;
    let mut name = String::new();
    let mut phys = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("HID_ID=") {
            let mut parts = v.split(':');
            let _bus = parts.next()?;
            let vid = u32::from_str_radix(parts.next()?, 16).ok()?;
            let pid = u32::from_str_radix(parts.next()?, 16).ok()?;
            ids = Some((u16::try_from(vid).ok()?, u16::try_from(pid).ok()?));
        } else if let Some(v) = line.strip_prefix("HID_NAME=") {
            name = v.chars().filter(|c| !c.is_control()).collect();
        } else if let Some(v) = line.strip_prefix("HID_PHYS=") {
            phys = v.chars().filter(|c| !c.is_control()).collect();
        }
    }
    let (vid, pid) = ids?;
    Some((vid, pid, name, phys))
}

/// An open hidraw device. Only drivers for allow-listed VID:PIDs create these.
pub struct HidDevice {
    file: File,
}

impl HidDevice {
    pub fn open(info: &HidInfo) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(&info.node)?;
        Ok(Self { file })
    }

    /// Send one output report. `report[0]` is the report id (0 if the device has none).
    pub fn write_report(&mut self, report: &[u8]) -> io::Result<()> {
        if report.is_empty() || report.len() > MAX_WRITE {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "bad report size"));
        }
        // A non-blocking write can briefly fail with WouldBlock; retry a few times.
        for _ in 0..20 {
            match self.file.write(report) {
                Ok(n) if n == report.len() => return Ok(()),
                Ok(_) => return Err(io::Error::new(io::ErrorKind::WriteZero, "short write")),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(1)),
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "device not accepting data"))
    }

    /// Wait up to `timeout` for one input report.
    pub fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.file.read(buf) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "no reply from device"));
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Throw away queued replies so the kernel buffer never fills up.
    pub fn drain(&mut self) {
        let mut buf = [0u8; 64];
        for _ in 0..64 {
            match self.file.read(&mut buf) {
                Ok(n) if n > 0 => continue,
                _ => break,
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_uevent() {
        let t = "DRIVER=hid-generic\nHID_ID=0003:00000B05:00001867\nHID_NAME=AsusTek Computer Inc. AURA LED Controller\nHID_PHYS=usb-0000:00:14.0-9/input2\n";
        let (vid, pid, name, phys) = parse_uevent(t).unwrap();
        assert_eq!((vid, pid), (0x0B05, 0x1867));
        assert!(name.contains("AURA"));
        assert_eq!(phys, "usb-0000:00:14.0-9/input2");
        assert_eq!(parse_uevent("HID_ID=0003:XYZ:1"), None);
        assert_eq!(parse_uevent("HID_ID=0003:100000000:1"), None);
        assert_eq!(parse_uevent(""), None);
    }

    #[test]
    fn usage_page_detection() {
        let mk = |d: Vec<u8>| HidInfo { node: "/dev/hidraw0".into(), vid: 0, pid: 0, name: String::new(), phys: String::new(), descriptor: d };
        // Usage Page (Vendor 0xFF72) = 06 72 FF
        assert!(mk(vec![0x06, 0x72, 0xFF, 0x09, 0xA1]).has_usage_page(0xFF72));
        assert!(!mk(vec![0x05, 0x01, 0x09, 0x02]).has_usage_page(0xFF72)); // a mouse
        assert!(!mk(Vec::new()).has_usage_page(0xFF72));
    }

    #[test]
    fn enumerates_fake_sysfs() {
        let root = std::env::temp_dir().join(format!("rgbforge-test-{}", std::process::id()));
        let dev = root.join("hidraw3/device");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("uevent"), "HID_ID=0003:00001B1C:00000C0B\nHID_NAME=Lighting Node Pro\n").unwrap();
        let found = enumerate_in(&root, Path::new("/dev"));
        fs::remove_dir_all(&root).ok();
        assert_eq!(found.len(), 1);
        assert_eq!(found.first().unwrap().node, PathBuf::from("/dev/hidraw3"));
        assert_eq!(found.first().unwrap().pid, 0x0C0B);
    }
}
