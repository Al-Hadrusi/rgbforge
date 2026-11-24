//! Razer Chroma peripherals (wired keyboards, mice, mousemats, headset stands, keypads)
//! using the "extended matrix" lighting commands.
//!
//! Protocol (from openrazer and OpenRGB): 90-byte HID *feature* reports (plus report id 0),
//! sent with the `hidapi` crate, each answered by a feature-report reply:
//!   status, transaction_id, remaining_packets(be16), protocol_type, data_size,
//!   command_class, command_id, arguments[80], crc (XOR of bytes 2..=87), reserved.
//!
//! Commands this driver can send (enforced by [`allowed`]):
//! * `00/81` read firmware version: read-only, used to identify the right USB interface
//! * `0F/03` custom frame row: `00 00 row start stop RGB…`
//! * `0F/02` show the custom frame: `00 00 08` (storage 0x00 = NOSTORE, nothing saved)
//!
//! Everything else is refused: DPI (class 04), polling rate / device mode / pairing
//! (class 00 writes), key behaviour (class 02), power (class 07), profiles, and any
//! lighting command with VARSTORE (which writes to the device's onboard profile).

use std::ffi::CString;
use std::fs::OpenOptions;
use std::io;
use std::thread;
use std::time::Duration;

use super::Driver;
use crate::hidraw::HidInfo;
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

const REPORT: usize = 91; // report id 0 + 90-byte Razer report
const STATUS_BUSY: u8 = 0x01;
const STATUS_OK: u8 = 0x02;

#[derive(Clone, Copy, Debug)]
pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    pub kind: Kind,
    pub txn: u8,
    pub rows: u8,
    pub cols: u8,
}

const fn m(pid: u16, name: &'static str, kind: Kind, txn: u8, rows: u8, cols: u8) -> Model {
    Model { pid, name, kind, txn, rows, cols }
}

/// Wired extended-matrix devices (from OpenRGB's device table).
pub const MODELS: &[Model] = &[
    m(0x0228, "Razer BlackWidow Elite", Kind::Other, 0x1F, 6, 22),
    m(0x0241, "Razer BlackWidow 2019", Kind::Other, 0x1F, 6, 22),
    m(0x024E, "Razer BlackWidow V3", Kind::Other, 0x3F, 6, 22),
    m(0x0A24, "Razer BlackWidow V3 TKL", Kind::Other, 0x3F, 6, 18),
    m(0x025A, "Razer BlackWidow V3 Pro", Kind::Other, 0x3F, 6, 22),
    m(0x0258, "Razer BlackWidow V3 Mini", Kind::Other, 0x1F, 5, 16),
    m(0x0287, "Razer BlackWidow V4", Kind::Other, 0x1F, 8, 23),
    m(0x028D, "Razer BlackWidow V4 Pro", Kind::Other, 0x1F, 8, 23),
    m(0x02A5, "Razer BlackWidow V4 75%", Kind::Other, 0x1F, 6, 18),
    m(0x0293, "Razer BlackWidow V4 X", Kind::Other, 0x1F, 6, 22),
    m(0x0227, "Razer Huntsman", Kind::Other, 0x3F, 6, 22),
    m(0x0226, "Razer Huntsman Elite", Kind::Other, 0x3F, 9, 22),
    m(0x0243, "Razer Huntsman Tournament Edition", Kind::Other, 0x3F, 6, 18),
    m(0x0257, "Razer Huntsman Mini", Kind::Other, 0x3F, 5, 15),
    m(0x026C, "Razer Huntsman V2", Kind::Other, 0x3F, 6, 22),
    m(0x026B, "Razer Huntsman V2 TKL", Kind::Other, 0x3F, 6, 17),
    m(0x0266, "Razer Huntsman V2 Analog", Kind::Other, 0x3F, 9, 22),
    m(0x02A6, "Razer Huntsman V3 Pro", Kind::Other, 0x3F, 6, 22),
    m(0x021E, "Razer Ornata Chroma", Kind::Other, 0x3F, 6, 22),
    m(0x025D, "Razer Ornata V2", Kind::Other, 0x1F, 6, 22),
    m(0x028F, "Razer Ornata V3", Kind::Other, 0x1F, 1, 10),
    m(0x02A1, "Razer Ornata V3", Kind::Other, 0x1F, 1, 10),
    m(0x0294, "Razer Ornata V3 X", Kind::Other, 0x1F, 1, 1),
    m(0x022A, "Razer Cynosa Chroma", Kind::Other, 0x3F, 6, 22),
    m(0x025E, "Razer Cynosa V2", Kind::Other, 0x1F, 6, 22),
    m(0x0244, "Razer Tartarus Pro", Kind::Other, 0x1F, 1, 21),
    m(0x022B, "Razer Tartarus V2", Kind::Other, 0x1F, 4, 6),
    m(0x0084, "Razer DeathAdder V2", Kind::Other, 0x1F, 1, 2),
    m(0x008C, "Razer DeathAdder V2 Mini", Kind::Other, 0x1F, 1, 1),
    m(0x005C, "Razer DeathAdder Elite", Kind::Other, 0x3F, 1, 2),
    m(0x0098, "Razer DeathAdder Essential V2", Kind::Other, 0x1F, 1, 2),
    m(0x0064, "Razer Basilisk", Kind::Other, 0x3F, 1, 2),
    m(0x0085, "Razer Basilisk V2", Kind::Other, 0x3F, 1, 2),
    m(0x0099, "Razer Basilisk V3", Kind::Other, 0x1F, 1, 11),
    m(0x00AA, "Razer Basilisk V3 Pro", Kind::Other, 0x1F, 1, 13),
    m(0x0086, "Razer Basilisk Ultimate", Kind::Other, 0x1F, 1, 14),
    m(0x007A, "Razer Viper Ultimate", Kind::Other, 0x3F, 1, 1),
    m(0x0078, "Razer Viper", Kind::Other, 0x3F, 1, 1),
    m(0x008A, "Razer Viper Mini", Kind::Other, 0x3F, 1, 1),
    m(0x0091, "Razer Viper 8KHz", Kind::Other, 0x1F, 1, 1),
    m(0x0067, "Razer Naga Trinity", Kind::Other, 0x3F, 1, 3),
    m(0x006C, "Razer Mamba Elite", Kind::Other, 0x1F, 1, 20),
    m(0x0C04, "Razer Firefly V2", Kind::Other, 0x3F, 1, 19),
    m(0x0C08, "Razer Firefly V2 Pro", Kind::Other, 0x1F, 1, 17),
    m(0x0C01, "Razer Goliathus", Kind::Other, 0x3F, 1, 1),
    m(0x0C02, "Razer Goliathus Extended", Kind::Other, 0x3F, 1, 1),
    m(0x0C06, "Razer Goliathus 3XL", Kind::Other, 0x1F, 1, 1),
    m(0x0C05, "Razer Strider Chroma", Kind::Other, 0x3F, 1, 19),
    m(0x0F08, "Razer Base Station Chroma", Kind::Other, 0x3F, 1, 15),
    m(0x0F20, "Razer Base Station V2 Chroma", Kind::Other, 0x1F, 1, 8),
];

pub fn model(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// The full write allow-list. `pkt` is the 91-byte buffer (report id first).
pub(crate) fn allowed(pkt: &[u8], model: &Model) -> bool {
    let (Some(class), Some(id), Some(args)) = (pkt.get(7), pkt.get(8), pkt.get(9..)) else { return false };
    match (class, id) {
        (0x00, 0x81) => true, // read firmware version (GET)
        (0x0F, 0x03) => match args {
            [0x00, 0x00, row, start, stop, ..] => *row < model.rows && start <= stop && *stop < model.cols,
            _ => false,
        },
        // Show the custom frame, NOSTORE only: storage 0, LED 0, effect 0x08.
        (0x0F, 0x02) => matches!(args, [0x00, 0x00, 0x08, ..]),
        _ => false,
    }
}

/// Build a report: id 0, status 0, transaction id, data size, class, command, arguments, CRC.
pub(crate) fn report(txn: u8, class: u8, id: u8, size: u8, args: &[u8]) -> [u8; REPORT] {
    let mut p = [0u8; REPORT];
    for (dst, src) in p.iter_mut().skip(1).zip([0x00, txn, 0x00, 0x00, 0x00, size, class, id]) {
        *dst = src;
    }
    for (dst, src) in p.iter_mut().skip(9).zip(args.iter().take(80)) {
        *dst = *src;
    }
    // CRC: XOR of Razer-report bytes 2..=87 (buffer bytes 3..=88, after the report id).
    let crc = p.get(3..89).unwrap_or_default().iter().fold(0u8, |a, b| a ^ b);
    if let Some(b) = p.get_mut(89) {
        *b = crc;
    }
    p
}

/// One custom-frame row packet.
pub(crate) fn row_packet(model: &Model, row: u8, colors: &[Rgb]) -> [u8; REPORT] {
    let cols = usize::from(model.cols).min(colors.len()).clamp(1, 25);
    let stop = u8::try_from(cols - 1).unwrap_or(0);
    let mut args = vec![0x00, 0x00, row, 0x00, stop];
    args.extend(colors.iter().take(cols).flat_map(|c| [c.r, c.g, c.b]));
    let size = u8::try_from(cols * 3 + 5).unwrap_or(0x47);
    report(model.txn, 0x0F, 0x03, size, &args)
}

pub(crate) fn show_frame_packet(model: &Model) -> [u8; REPORT] {
    report(model.txn, 0x0F, 0x02, 0x0C, &[0x00, 0x00, 0x08])
}

/// Process names of other Razer tools that would talk to the same device.
pub fn conflicting_tool_running() -> Option<&'static str> {
    let names: Vec<String> = std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|c| c.trim().to_ascii_lowercase())
        .collect();
    if names.iter().any(|n| n.starts_with("openrazer")) {
        Some("OpenRazer")
    } else if names.iter().any(|n| n == "openrgb") {
        Some("OpenRGB")
    } else {
        None
    }
}

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

pub struct Razer {
    dev: hidapi::HidDevice,
    model: Model,
    info: DeviceInfo,
}

impl Razer {
    /// Send one allow-listed packet and check the device's reply matches it.
    fn transact(&self, pkt: &[u8; REPORT]) -> io::Result<[u8; REPORT]> {
        if !allowed(pkt, &self.model) {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "blocked non-lighting Razer command"));
        }
        self.dev.send_feature_report(pkt).map_err(err)?;
        for _ in 0..10 {
            thread::sleep(Duration::from_millis(1));
            let mut reply = [0u8; REPORT];
            let n = self.dev.get_feature_report(&mut reply).map_err(err)?;
            if n < 10 {
                continue;
            }
            // Only accept the answer to *this* command (other software may share the device).
            if reply.get(7..9) != pkt.get(7..9) {
                continue;
            }
            match reply.get(1).copied() {
                Some(STATUS_OK | STATUS_BUSY) => return Ok(reply),
                Some(0x00) => continue, // still processing
                Some(s) => return Err(io::Error::other(format!("device refused the command (status {s:#04x})"))),
                None => continue,
            }
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "no reply from the device"))
    }

    /// Open `hid` if it is the device's lighting interface: it must answer a read-only
    /// firmware query correctly before anything else is sent.
    pub fn open(hid: &HidInfo, model: &Model) -> io::Result<Self> {
        OpenOptions::new().read(true).write(true).open(&hid.node)?; // explain permissions clearly
        let api = hidapi::HidApi::new().map_err(err)?;
        let path = CString::new(hid.node.to_string_lossy().as_bytes()).map_err(err)?;
        let dev = api.open_path(&path).map_err(err)?;
        let leds = u16::from(model.rows) * u16::from(model.cols);
        let me = Self {
            dev,
            model: *model,
            info: DeviceInfo {
                id: format!("razer:{:04x}:{}", model.pid, device_key(&hid.phys)),
                name: model.name.into(),
                kind: model.kind,
                zones: vec![ZoneInfo { name: if model.rows > 1 { "Keys" } else { "Lighting" }.into(), leds, resizable: false, max_leds: leds }],
                demo: false,
                experimental: true,
                detail: String::new(),
            },
        };
        let reply = me.transact(&report(model.txn, 0x00, 0x81, 0x02, &[]))?;
        let (major, minor) = (reply.get(9).copied().unwrap_or(0), reply.get(10).copied().unwrap_or(0));
        Ok(Self { info: DeviceInfo { detail: format!("Firmware {major}.{minor}"), ..me.info.clone() }, ..me })
    }
}

/// USB path without the interface suffix, so all interfaces of one device share a key.
pub fn device_key(phys: &str) -> String {
    phys.rsplit_once("/input").map_or(phys, |(dev, _)| dev).to_string()
}

impl Driver for Razer {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, _zone: usize, _leds: u16) {}

    fn apply(&mut self, frame: &[Vec<Rgb>]) -> io::Result<()> {
        let colors = frame.first().cloned().unwrap_or_default();
        let cols = usize::from(self.model.cols).max(1);
        for (row, chunk) in (0..self.model.rows).zip(colors.chunks(cols)) {
            self.transact(&row_packet(&self.model, row, chunk))?;
        }
        self.transact(&show_frame_packet(&self.model))?;
        Ok(())
    }

    fn keepalive_secs(&self) -> f32 {
        f32::MAX
    }

    fn min_interval_secs(&self) -> f32 {
        0.06
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used)]
mod tests {
    use super::*;

    fn kb() -> Model {
        *model(0x024E).unwrap() // BlackWidow V3, 6×22, txn 0x3F
    }

    #[test]
    fn report_layout_and_crc() {
        let p = show_frame_packet(&kb());
        assert_eq!(&p[..12], &[0x00, 0x00, 0x3F, 0x00, 0x00, 0x00, 0x0C, 0x0F, 0x02, 0x00, 0x00, 0x08]);
        // CRC = XOR of Razer bytes 2..=87 = 0x0C ^ 0x0F ^ 0x02 ^ 0x08
        assert_eq!(p[89], 0x0C ^ 0x0F ^ 0x02 ^ 0x08);
        assert_eq!(p.len(), 91);
    }

    #[test]
    fn row_packets() {
        let colors: Vec<Rgb> = (0..22u8).map(|i| Rgb::new(i, 1, 2)).collect();
        let p = row_packet(&kb(), 3, &colors);
        assert_eq!(&p[6..9], &[22 * 3 + 5, 0x0F, 0x03]);
        assert_eq!(&p[9..17], &[0x00, 0x00, 3, 0, 21, 0, 1, 2]);
        assert!(allowed(&p, &kb()));
    }

    #[test]
    fn only_lighting_with_nostore_is_allowed() {
        let k = kb();
        assert!(allowed(&report(0x3F, 0x00, 0x81, 0x02, &[]), &k)); // firmware read
        for (class, id, args) in [
            (0x04u8, 0x05u8, vec![0x01, 0x03, 0x20]), // DPI
            (0x00, 0x05, vec![0x01]),                 // polling rate
            (0x00, 0x04, vec![0x03, 0x00]),           // device mode
            (0x02, 0x06, vec![0x01]),                 // fn toggle
            (0x07, 0x03, vec![0x00]),                 // idle time
            (0x0F, 0x02, vec![0x01, 0x00, 0x08]),     // custom frame with VARSTORE
            (0x0F, 0x02, vec![0x00, 0x05, 0x01]),     // other effect
            (0x0F, 0x04, vec![0x01, 0x05, 0xFF]),     // brightness VARSTORE
            (0x03, 0x0A, vec![0x05, 0x01]),           // legacy apply with storage
        ] {
            assert!(!allowed(&report(0x3F, class, id, 0x09, &args), &k), "{class:02x}/{id:02x}");
        }
        // Rows/columns outside the device's matrix are refused.
        assert!(!allowed(&report(0x3F, 0x0F, 0x03, 0x08, &[0, 0, 6, 0, 0]), &k));
        assert!(!allowed(&report(0x3F, 0x0F, 0x03, 0x08, &[0, 0, 0, 0, 22]), &k));
        assert!(!allowed(&report(0x3F, 0x0F, 0x03, 0x08, &[1, 0, 0, 0, 0]), &k));
    }

    #[test]
    fn device_keys_group_interfaces() {
        assert_eq!(device_key("usb-0000:00:14.0-3/input2"), "usb-0000:00:14.0-3");
        assert_eq!(device_key("usb-0000:00:14.0-3/input0"), device_key("usb-0000:00:14.0-3/input3"));
    }

    #[test]
    fn table_is_sane() {
        for (i, a) in MODELS.iter().enumerate() {
            assert!(a.rows >= 1 && a.cols >= 1 && u16::from(a.cols) <= 25, "{}", a.name);
            assert!(MODELS.iter().skip(i + 1).all(|b| b.pid != a.pid), "duplicate {:04x}", a.pid);
        }
    }
}
