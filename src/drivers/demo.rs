//! Simulated devices so the app can be tried (and tested) without hardware.

use std::io;

use super::Driver;
use crate::model::{DeviceInfo, Kind, Rgb, ZoneInfo};

pub struct Demo {
    info: DeviceInfo,
}

fn zone(name: &str, leds: u16, resizable: bool) -> ZoneInfo {
    ZoneInfo { name: name.into(), leds, resizable, max_leds: if resizable { 120 } else { leds } }
}

fn demo(id: &str, name: &str, kind: Kind, zones: Vec<ZoneInfo>) -> Box<dyn Driver> {
    Box::new(Demo {
        info: DeviceInfo {
            id: format!("demo:{id}"),
            name: name.into(),
            kind,
            zones,
            demo: true,
            experimental: false,
            detail: "Demo device: shows what your lighting would look like.".into(),
        },
    })
}

pub fn devices() -> Vec<Box<dyn Driver>> {
    vec![
        demo("ram", "Demo Memory (2 sticks)", Kind::Ram, vec![zone("Stick 1", 10, false), zone("Stick 2", 10, false)]),
        demo("gpu", "Demo Graphics Card", Kind::Gpu, vec![zone("Logo", 4, false), zone("Light bar", 18, false)]),
        demo("aio", "Demo Liquid Cooler", Kind::Cooler, vec![zone("Pump", 16, false), zone("Radiator fans", 24, true)]),
        demo("fans", "Demo Case Fans", Kind::Fans, vec![zone("Front fans", 24, true), zone("Rear fan", 8, true)]),
    ]
}

impl Driver for Demo {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn resize_zone(&mut self, zone: usize, leds: u16) {
        if let Some(z) = self.info.zones.get_mut(zone).filter(|z| z.resizable) {
            z.leds = leds.min(z.max_leds);
        }
    }

    fn apply(&mut self, _frame: &[Vec<Rgb>]) -> io::Result<()> {
        Ok(())
    }
}
