//! Device drivers.
//!
//! Safety rules every driver follows:
//! * A driver is only ever attached to a USB device whose exact vendor:product id
//!   is in [`SUPPORTED`]. Nothing else is opened, let alone written to.
//! * Drivers only send *lighting* commands that change RAM-only state on the device.
//!   Nothing is ever written to device flash/EEPROM, and no firmware commands exist here.
//! * After repeated write errors, the engine stops talking to that device until a rescan.

mod aura;
mod corsair;
mod asrock;
mod demo;
mod gigabyte;
mod lianli;
mod nzxt;
pub mod razer;
pub mod sapphire;
pub mod ene_ram;
pub mod ddr4_check;

use std::io;

use crate::hidraw::{self, HidInfo};
use crate::model::{DeviceInfo, Rgb};

/// LEDs assumed on an addressable header until the user says otherwise. Sending data
/// for LEDs that aren't there is harmless (it just runs off the end of the chain),
/// while 0 would leave connected fans dark, which is far more confusing.
pub const DEFAULT_HEADER_LEDS: u16 = 40;

pub trait Driver: Send {
    fn info(&self) -> &DeviceInfo;
    /// Change the LED count of a user-configurable zone (clamped to its maximum).
    fn resize_zone(&mut self, zone: usize, leds: u16);
    /// Show `frame` (one colour list per zone, each exactly that zone's length).
    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()>;
    /// Seconds after which the device must be refreshed even if nothing changed.
    fn keepalive_secs(&self) -> f32 {
        1.0
    }
    /// Minimum seconds between updates (rate limit for slow buses).
    fn min_interval_secs(&self) -> f32 {
        0.0
    }
}

pub struct Supported {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub kind: Family,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    AuraArgb,
    AuraMainboard,
    CorsairNode { channels: u8 },
    Nzxt(nzxt::Model),
    LianLi,
    AsRock,
    Gigabyte,
}

/// The complete allow-list of real hardware this program will ever talk to.
pub const SUPPORTED: &[Supported] = &[
    Supported { vid: 0x0B05, pid: 0x1867, name: "ASUS Aura ARGB controller", kind: Family::AuraArgb },
    Supported { vid: 0x0B05, pid: 0x1872, name: "ASUS Aura ARGB controller", kind: Family::AuraArgb },
    Supported { vid: 0x0B05, pid: 0x18A3, name: "ASUS Aura ARGB controller", kind: Family::AuraArgb },
    Supported { vid: 0x0B05, pid: 0x18A5, name: "ASUS Aura ARGB controller", kind: Family::AuraArgb },
    Supported { vid: 0x0B05, pid: 0x18F3, name: "ASUS motherboard (Aura)", kind: Family::AuraMainboard },
    Supported { vid: 0x0B05, pid: 0x1939, name: "ASUS motherboard (Aura)", kind: Family::AuraMainboard },
    Supported { vid: 0x0B05, pid: 0x19AF, name: "ASUS motherboard (Aura)", kind: Family::AuraMainboard },
    Supported { vid: 0x0B05, pid: 0x1AA6, name: "ASUS motherboard (Aura)", kind: Family::AuraMainboard },
    Supported { vid: 0x1E71, pid: 0x2007, name: "NZXT Kraken X53/X63/X73", kind: Family::Nzxt(nzxt::Model::KrakenX3) },
    Supported { vid: 0x1E71, pid: 0x2014, name: "NZXT Kraken X53/X63/X73", kind: Family::Nzxt(nzxt::Model::KrakenX3) },
    Supported { vid: 0x1E71, pid: 0x3008, name: "NZXT Kraken Z53/Z63/Z73", kind: Family::Nzxt(nzxt::Model::KrakenZ3) },
    Supported { vid: 0x1E71, pid: 0x3012, name: "NZXT Kraken Elite RGB (2024)", kind: Family::Nzxt(nzxt::Model::KrakenElite2024) },
    Supported { vid: 0x1E71, pid: 0x2006, name: "NZXT Smart Device V2", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x200D, name: "NZXT Smart Device V2", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x200F, name: "NZXT Smart Device V2", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x2001, name: "NZXT HUE 2", kind: Family::Nzxt(nzxt::Model::Hub { channels: 4 }) },
    Supported { vid: 0x1E71, pid: 0x2002, name: "NZXT HUE 2 Ambient", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x2009, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x200E, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x2010, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 2 }) },
    Supported { vid: 0x1E71, pid: 0x2011, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 6 }) },
    Supported { vid: 0x1E71, pid: 0x2019, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 6 }) },
    Supported { vid: 0x1E71, pid: 0x201F, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 6 }) },
    Supported { vid: 0x1E71, pid: 0x2020, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 6 }) },
    Supported { vid: 0x1E71, pid: 0x2022, name: "NZXT RGB & Fan Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 6 }) },
    Supported { vid: 0x1E71, pid: 0x2012, name: "NZXT RGB Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 3 }) },
    Supported { vid: 0x1E71, pid: 0x2021, name: "NZXT RGB Controller", kind: Family::Nzxt(nzxt::Model::Hub { channels: 3 }) },
    Supported { vid: 0x0CF2, pid: 0xA103, name: "Lian Li UNI HUB SL V2", kind: Family::LianLi },
    Supported { vid: 0x0CF2, pid: 0xA104, name: "Lian Li UNI HUB AL V2", kind: Family::LianLi },
    Supported { vid: 0x0CF2, pid: 0xA105, name: "Lian Li UNI HUB SL V2", kind: Family::LianLi },
    Supported { vid: 0x26CE, pid: 0x01A2, name: "ASRock Polychrome RGB", kind: Family::AsRock },
    Supported { vid: 0x26CE, pid: 0x01A6, name: "ASRock DeskMini LED strip", kind: Family::AsRock },
    Supported { vid: 0x048D, pid: 0x5702, name: "Gigabyte RGB Fusion 2", kind: Family::Gigabyte },
    Supported { vid: 0x048D, pid: 0x8297, name: "Gigabyte RGB Fusion 2", kind: Family::Gigabyte },
    Supported { vid: 0x048D, pid: 0x5711, name: "Gigabyte RGB Fusion 2", kind: Family::Gigabyte },
    Supported { vid: 0x048D, pid: 0x8950, name: "Gigabyte RGB Fusion 2", kind: Family::Gigabyte },
    Supported { vid: 0x1B1C, pid: 0x0C0B, name: "Corsair Lighting Node Pro", kind: Family::CorsairNode { channels: 2 } },
    Supported { vid: 0x1B1C, pid: 0x0C1A, name: "Corsair Lighting Node Core", kind: Family::CorsairNode { channels: 1 } },
    Supported { vid: 0x1B1C, pid: 0x0C10, name: "Corsair Commander Pro", kind: Family::CorsairNode { channels: 2 } },
];

/// USB HID devices present but without a driver, as "vvvv:pppp Name" lines (deduplicated).
/// Read-only: comes from sysfs, nothing is opened. Used for driver requests.
pub fn unsupported_usb() -> Vec<String> {
    unsupported_from(hidraw::enumerate())
}

pub fn unsupported_from(hids: Vec<HidInfo>) -> Vec<String> {
    let mut out: Vec<String> = hids
        .into_iter()
        .filter(|h| lookup(h.vid, h.pid).is_none() && !is_razer(h))
        .map(|h| format!("{:04x}:{:04x} {}", h.vid, h.pid, h.name))
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn lookup(vid: u16, pid: u16) -> Option<&'static Supported> {
    SUPPORTED.iter().find(|s| s.vid == vid && s.pid == pid)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum DemoMode {
    /// Show demo devices only when no real hardware is found.
    #[default]
    Auto,
    Always,
    Never,
}

/// Lower-cased names of processes run by root or by members of the `rgbforge` group.
/// Used to avoid fighting other RGB tools over a device; processes of other accounts are
/// ignored, so they can't pause RGB Forge just by naming a process "openrgb".
pub fn trusted_process_names() -> Vec<String> {
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let uids = trusted_uids(&group, &passwd);
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let status = std::fs::read_to_string(e.path().join("status")).ok()?;
            uids.contains(&real_uid(&status)?).then_some(())?;
            Some(std::fs::read_to_string(e.path().join("comm")).ok()?.trim().to_ascii_lowercase())
        })
        .collect()
}

/// root, plus every account in the `rgbforge` group (listed members or primary group).
pub(crate) fn trusted_uids(group: &str, passwd: &str) -> Vec<u32> {
    let mut uids = vec![0];
    let Some((gid, members)) = group.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.first() == Some(&"rgbforge")).then(|| (f.get(2).copied().unwrap_or(""), f.get(3).copied().unwrap_or("")))
    }) else {
        return uids;
    };
    let members: Vec<&str> = members.split(',').filter(|m| !m.is_empty()).collect();
    for l in passwd.lines() {
        let f: Vec<&str> = l.split(':').collect();
        let (Some(name), Some(uid), Some(pgid)) = (f.first(), f.get(2), f.get(3)) else { continue };
        if (members.contains(name) || *pgid == gid)
            && let Ok(u) = uid.parse()
        {
            uids.push(u);
        }
    }
    uids
}

/// The real uid from a /proc/<pid>/status text.
pub(crate) fn real_uid(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok())
}

/// The simulated devices (added by the app, never by the hardware service).
pub fn demo_devices() -> Vec<Box<dyn Driver>> {
    demo::devices()
}

pub struct Detection {
    pub drivers: Vec<Box<dyn Driver>>,
    /// Human-readable problems (e.g. "found X but no permission").
    pub problems: Vec<String>,
}

pub fn detect(demo: DemoMode, gpu: bool, ram: bool) -> Detection {
    let mut d = detect_from(hidraw::enumerate(), demo);
    if gpu {
        match sapphire::find_card(std::path::Path::new("/sys/bus/pci/devices")) {
            None => d.problems.push(format!("Graphics card lighting is on, but no {} was found.", sapphire::NAME)),
            Some(card) => match sapphire::SapphireNitro::open(&card) {
                Ok(drv) => {
                    // Real hardware found: drop demo devices shown in Auto mode.
                    if demo == DemoMode::Auto {
                        d.drivers.retain(|x| !x.info().demo);
                    }
                    d.drivers.push(Box::new(drv));
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => d.problems.push(
                    "Graphics card found, but no permission for its RGB bus yet. Re-run ./install.sh, then log out and back in.".into(),
                ),
                Err(e) => d.problems.push(format!("Graphics card lighting not started: {e}")),
            },
        }
    }
    if ram {
        let (rams, problems) = ene_ram::detect();
        if !rams.is_empty() && demo == DemoMode::Auto {
            d.drivers.retain(|x| !x.info().demo);
        }
        d.drivers.extend(rams.into_iter().map(|r| Box::new(r) as Box<dyn Driver>));
        d.problems.extend(problems);
    }
    d
}

fn is_razer(h: &HidInfo) -> bool {
    h.vid == RAZER_VID && razer::model(h.pid).is_some()
}

const RAZER_VID: u16 = 0x1532;

pub fn detect_from(hids: Vec<HidInfo>, demo: DemoMode) -> Detection {
    let mut drivers: Vec<Box<dyn Driver>> = Vec::new();
    let mut problems = Vec::new();
    // Razer devices have several interfaces; only the one that answers a read-only
    // firmware query is used, once per physical device.
    let mut razer_done: Vec<String> = Vec::new();
    let mut razer_denied: Vec<(String, &'static str)> = Vec::new();
    let razer_conflict = if hids.iter().any(is_razer) { razer::conflicting_tool_running() } else { None };
    for info in hids {
        if let Some(model) = razer::model(info.pid).filter(|_| info.vid == RAZER_VID) {
            let key = razer::device_key(&info.phys);
            if razer_done.contains(&key) || razer_conflict.is_some() {
                continue;
            }
            match razer::Razer::open(&info, model) {
                Ok(d) => {
                    razer_done.push(key);
                    drivers.push(Box::new(d));
                }
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => razer_denied.push((key, model.name)),
                Err(_) => {} // not the lighting interface
            }
            continue;
        }
        let Some(sup) = lookup(info.vid, info.pid) else { continue };
        let result: io::Result<Box<dyn Driver>> = match sup.kind {
            Family::AuraArgb => aura::Aura::open(&info, sup.name, false).map(|d| Box::new(d) as Box<dyn Driver>),
            Family::AuraMainboard => aura::Aura::open(&info, sup.name, true).map(|d| Box::new(d) as Box<dyn Driver>),
            Family::Nzxt(model) => nzxt::Nzxt::open(&info, sup.name, model).map(|d| Box::new(d) as Box<dyn Driver>),
            Family::Gigabyte => {
                // Several interfaces share this chip; only the lighting one is ever opened.
                if !info.has_usage_page(gigabyte::USAGE_PAGE) {
                    continue;
                }
                gigabyte::Gigabyte::open(&info, sup.name).map(|d| Box::new(d) as Box<dyn Driver>)
            }
            Family::AsRock => asrock::AsRock::open(&info, sup.name).map(|d| Box::new(d) as Box<dyn Driver>),
            Family::LianLi => {
                // The hub has several USB interfaces; only the lighting one is ever opened.
                if !info.has_usage_page(lianli::USAGE_PAGE) {
                    continue;
                }
                lianli::LianLi::open(&info, sup.name).map(|d| Box::new(d) as Box<dyn Driver>)
            }
            Family::CorsairNode { channels } => {
                corsair::CorsairNode::open(&info, sup.name, channels).map(|d| Box::new(d) as Box<dyn Driver>)
            }
        };
        match result {
            Ok(d) => drivers.push(d),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => problems.push(format!(
                "Found {} but couldn't open it (no permission). Make sure the rgbforge-helper service is running (see Help).",
                sup.name
            )),
            Err(e) => problems.push(format!("Found {} but couldn't open it: {e}", sup.name)),
        }
    }
    if let Some(tool) = razer_conflict {
        problems.push(format!("Razer devices skipped: {tool} is running and would talk to them at the same time. Close it, then click Find devices."));
    }
    razer_denied.retain(|(k, _)| !razer_done.contains(k));
    razer_denied.dedup_by(|a, b| a.0 == b.0);
    for (_, name) in razer_denied {
        problems.push(format!("Found {name} but couldn't open it (no permission). Make sure the rgbforge-helper service is running (see Help)."));
    }
    let show_demo = match demo {
        DemoMode::Always => true,
        DemoMode::Never => false,
        DemoMode::Auto => drivers.is_empty(),
    };
    if show_demo {
        drivers.extend(demo::devices());
    }
    Detection { drivers, problems }
}

/// Split `colors` into packets of at most `max` items, yielding `(start, chunk)`.
pub(crate) fn chunks(colors: &[Rgb], max: usize) -> impl Iterator<Item = (usize, &[Rgb])> {
    colors.chunks(max.max(1)).enumerate().map(move |(i, c)| (i.saturating_mul(max.max(1)), c))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unknown_devices_are_ignored() {
        let keyboard = HidInfo {
            node: "/dev/hidraw0".into(),
            vid: 0x046D,
            pid: 0xC31C,
            name: "Some keyboard".into(),
            phys: String::new(),
            descriptor: Vec::new(),
        };
        let d = detect_from(vec![keyboard], DemoMode::Never);
        assert!(d.drivers.is_empty());
        assert!(d.problems.is_empty());
    }

    #[test]
    fn lists_only_unsupported_devices_once() {
        let h = |vid, pid, name: &str| HidInfo { node: "/dev/hidraw0".into(), vid, pid, name: name.into(), phys: String::new(), descriptor: Vec::new() };
        let list = unsupported_from(vec![h(0x1532, 0x00a6, "Razer Viper V2 Pro"), h(0x1532, 0x00a6, "Razer Viper V2 Pro"), h(0x0b05, 0x19af, "AURA")]);
        assert_eq!(list, vec!["1532:00a6 Razer Viper V2 Pro".to_string()]);
    }

    #[test]
    fn demo_mode() {
        assert!(!detect_from(vec![], DemoMode::Auto).drivers.is_empty());
        assert!(detect_from(vec![], DemoMode::Never).drivers.is_empty());
    }

    #[test]
    fn only_root_and_rgbforge_members_are_trusted() {
        let group = "root:x:0:\nrgbforge:x:977:alice,bob\nwheel:x:10:carol\n";
        let passwd = "root:x:0:0::/root:/bin/sh\nalice:x:1000:1000::/h:/bin/sh\nbob:x:1001:1001::/h:/bin/sh\ncarol:x:1002:1002::/h:/bin/sh\nsvc:x:990:977::/:/bin/false\n";
        let mut u = trusted_uids(group, passwd);
        u.sort_unstable();
        assert_eq!(u, vec![0, 990, 1000, 1001]); // carol (other account) is not trusted
        assert_eq!(trusted_uids("", ""), vec![0]);
        assert_eq!(real_uid("Name:\topenrgb\nUid:\t1002\t1002\t1002\t1002\n"), Some(1002));
        assert_eq!(real_uid("garbage"), None);
    }

    #[test]
    fn firmware_upgrade_mode_device_is_never_supported() {
        // 048d:57db is the ITE chip in firmware-upgrade mode: never touch it.
        assert!(lookup(0x048D, 0x57DB).is_none());
    }

    #[test]
    fn allow_list_has_no_duplicates() {
        for (i, a) in SUPPORTED.iter().enumerate() {
            for b in SUPPORTED.iter().skip(i + 1) {
                assert!((a.vid, a.pid) != (b.vid, b.pid));
            }
        }
    }
}
