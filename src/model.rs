//! Core data types shared by the drivers, the lighting engine and the GUI.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb::new(0, 0, 0);
    #[cfg(test)]
    pub const WHITE: Rgb = Rgb::new(255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Parse `#RRGGBB`, `RRGGBB` or `#RGB`.
    pub fn from_hex(s: &str) -> Option<Self> {
        let h = s.trim().trim_start_matches('#');
        if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        match h.len() {
            6 => {
                let v = u32::from_str_radix(h, 16).ok()?;
                let [_, r, g, b] = v.to_be_bytes();
                Some(Self::new(r, g, b))
            }
            3 => {
                let d = |i: usize| h.get(i..i.saturating_add(1)).and_then(|c| u8::from_str_radix(c, 16).ok());
                Some(Self::new(d(0)? * 17, d(1)? * 17, d(2)? * 17))
            }
            _ => None,
        }
    }
}

/// What kind of component a device is (drives icons and grouping in the UI).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Kind {
    Ram,
    Gpu,
    Cooler,
    Fans,
    Motherboard,
    Strip,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Ram => "Memory",
            Kind::Gpu => "Graphics card",
            Kind::Cooler => "Liquid cooler",
            Kind::Fans => "Fans",
            Kind::Motherboard => "Motherboard",
            Kind::Strip => "LED strip",
            Kind::Other => "RGB device",
        }
    }

}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneInfo {
    pub name: String,
    pub leds: u16,
    /// Addressable headers can't detect how many LEDs are plugged in,
    /// so the user tells us in the UI.
    pub resizable: bool,
    pub max_leds: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable id used to remember settings (survives restarts).
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub zones: Vec<ZoneInfo>,
    /// Simulated device, so the app is usable without hardware.
    pub demo: bool,
    /// Driver not yet verified on real hardware.
    pub experimental: bool,
    pub detail: String,
}

impl DeviceInfo {
    pub fn led_count(&self) -> usize {
        self.zones.iter().map(|z| usize::from(z.leds)).sum()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::from_hex("#FF8000"), Some(Rgb::new(255, 128, 0)));
        assert_eq!(Rgb::from_hex("f80"), Some(Rgb::new(255, 136, 0)));
        assert_eq!(Rgb::from_hex(&Rgb::new(1, 2, 3).hex()), Some(Rgb::new(1, 2, 3)));
        for bad in ["", "#12", "#zzzzzz", "+12345", "ééé", "1234567"] {
            assert_eq!(Rgb::from_hex(bad), None, "{bad}");
        }
    }
}
