//! Lighting effects, computed on the PC and streamed to every device in sync.
//!
//! No strobe or flash effects exist on purpose: rapid flashing can trigger
//! photosensitive seizures. Everything here changes smoothly.

use serde::{Deserialize, Serialize};

use crate::model::Rgb;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Effect {
    Static,
    Gradient,
    Breathe,
    Rainbow,
    Wave,
    Comet,
    Music,
    Temperature,
    Off,
}

/// Which temperature the Temperature effect follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TempSource {
    #[default]
    Cpu,
    Gpu,
}

/// Live inputs some effects react to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    /// Smoothed music level 0..=1.
    pub audio: f32,
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
}

impl Effect {
    pub const ALL: [Effect; 9] = [
        Effect::Static,
        Effect::Gradient,
        Effect::Breathe,
        Effect::Rainbow,
        Effect::Wave,
        Effect::Comet,
        Effect::Music,
        Effect::Temperature,
        Effect::Off,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Effect::Static => "Solid",
            Effect::Gradient => "Gradient",
            Effect::Breathe => "Breathe",
            Effect::Rainbow => "Rainbow",
            Effect::Wave => "Rainbow Wave",
            Effect::Comet => "Comet",
            Effect::Music => "Music",
            Effect::Temperature => "Temperature",
            Effect::Off => "Off",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Effect::Static => "One colour, always on",
            Effect::Gradient => "Blend between two colours",
            Effect::Breathe => "Fades gently in and out",
            Effect::Rainbow => "Every LED cycles through all colours",
            Effect::Wave => "A rainbow flowing across your LEDs",
            Effect::Comet => "A bright spot gliding around",
            Effect::Music => "Pulses to whatever your PC is playing (first colour when quiet, second on the beat)",
            Effect::Temperature => "Cool colour when your CPU or GPU is cool, hot colour when it heats up",
            Effect::Off => "All LEDs off",
        }
    }

    pub fn uses_color(self) -> bool {
        matches!(self, Effect::Static | Effect::Gradient | Effect::Breathe | Effect::Comet | Effect::Music | Effect::Temperature)
    }

    pub fn uses_second_color(self) -> bool {
        matches!(self, Effect::Gradient | Effect::Music | Effect::Temperature)
    }

    /// Whether the Speed slider means anything for this effect.
    pub fn uses_speed(self) -> bool {
        self.animated() && self != Effect::Music
    }

    pub fn animated(self) -> bool {
        matches!(self, Effect::Breathe | Effect::Rainbow | Effect::Wave | Effect::Comet | Effect::Music)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub effect: Effect,
    pub color: Rgb,
    pub color2: Rgb,
    /// 1 (slow) ..= 10 (fast)
    pub speed: f32,
    /// 0 ..= 1
    pub brightness: f32,
    pub temp_source: TempSource,
    /// °C at which the first ("cool") colour shows fully.
    pub temp_low: f32,
    /// °C at which the second ("hot") colour shows fully.
    pub temp_high: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            effect: Effect::Wave,
            color: Rgb::new(140, 60, 255),
            color2: Rgb::new(0, 200, 255),
            speed: 3.0,
            brightness: 0.8,
            temp_source: TempSource::Cpu,
            temp_low: 40.0,
            temp_high: 80.0,
        }
    }
}

/// Fill `out` with `n` colours at time `t` seconds. `phase` (0..1) shifts
/// the pattern so neighbouring zones/devices continue the same wave.
/// `inputs` carries the music level and temperatures some effects react to.
pub fn render(l: &Lighting, t: f32, phase: f32, inputs: &Inputs, n: usize, out: &mut Vec<Rgb>) {
    let audio = inputs.audio;
    out.clear();
    let speed = finite_or(l.speed, 3.0).clamp(1.0, 10.0);
    let n_f = n.max(1) as f32;
    let pos = |i: usize| i as f32 / n_f;
    match l.effect {
        Effect::Off => out.resize(n, Rgb::BLACK),
        Effect::Static => out.resize(n, l.color),
        Effect::Gradient => {
            let span = (n.max(2) - 1) as f32;
            out.extend((0..n).map(|i| lerp(l.color, l.color2, i as f32 / span)));
        }
        Effect::Breathe => {
            // Period 8 s at speed 1 down to 0.8 s at speed 10: never a flash.
            let period = 8.0 / speed;
            let level = 0.06 + 0.94 * (0.5 - 0.5 * (t / period * std::f32::consts::TAU).cos());
            out.resize(n, scale(l.color, level));
        }
        Effect::Rainbow => out.resize(n, hsv(t * 12.0 * speed, 1.0, 1.0)),
        Effect::Temperature => {
            let temp = match l.temp_source {
                TempSource::Cpu => inputs.cpu_temp,
                TempSource::Gpu => inputs.gpu_temp,
            };
            out.resize(n, lerp(l.color, l.color2, temp_fraction(temp, l.temp_low, l.temp_high)));
        }
        Effect::Music => {
            // Never fully dark (floor 0.12): the engine also limits how fast this can fall,
            // so beats pulse rather than strobe.
            let a = finite_or(audio, 0.0).clamp(0.0, 1.0);
            out.resize(n, scale(lerp(l.color, l.color2, a), 0.12 + 0.88 * a));
        }
        Effect::Wave => {
            out.extend((0..n).map(|i| hsv(t * 12.0 * speed + (pos(i) + phase) * 360.0, 1.0, 1.0)));
        }
        Effect::Comet => {
            let head = (t * 0.25 * speed + phase).rem_euclid(1.0);
            out.extend((0..n).map(|i| {
                let d = (pos(i) - head).rem_euclid(1.0); // 0 at head, trailing behind
                let tail = (1.0 - d * 3.0).max(0.0).powi(2);
                scale(l.color, 0.08 + 0.92 * tail)
            }));
        }
    }
    let b = finite_or(l.brightness, 1.0).clamp(0.0, 1.0);
    if b < 1.0 {
        for c in out.iter_mut() {
            *c = scale(*c, b);
        }
    }
}

/// 0 at/below `low`, 1 at/above `high` (no sensor → 0, the cool colour).
pub fn temp_fraction(temp: Option<f32>, low: f32, high: f32) -> f32 {
    let (low, high) = (finite_or(low, 40.0), finite_or(high, 80.0));
    let Some(t) = temp.filter(|t| t.is_finite()) else { return 0.0 };
    if high <= low {
        return if t >= high { 1.0 } else { 0.0 };
    }
    ((t - low) / (high - low)).clamp(0.0, 1.0)
}

fn finite_or(v: f32, default: f32) -> f32 {
    if v.is_finite() { v } else { default }
}

/// HSV → RGB. `h` in degrees (wraps), `s`/`v` 0..=1.
pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = finite_or(h, 0.0).rem_euclid(360.0) / 60.0;
    let (s, v) = (s.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    Rgb::new(to_u8(r + m), to_u8(g + m), to_u8(b + m))
}

/// RGB → (hue degrees, saturation 0..1, value 0..1).
pub fn to_hsv(c: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(c.r) / 255.0, f32::from(c.g) / 255.0, f32::from(c.b) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s, max)
}

pub fn scale(c: Rgb, f: f32) -> Rgb {
    let f = finite_or(f, 0.0).clamp(0.0, 1.0);
    let s = |x: u8| to_u8(f32::from(x) / 255.0 * f);
    Rgb::new(s(c.r), s(c.g), s(c.b))
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = finite_or(t, 0.0).clamp(0.0, 1.0);
    let m = |x: u8, y: u8| to_u8((f32::from(x) + (f32::from(y) - f32::from(x)) * t) / 255.0);
    Rgb::new(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

fn to_u8(x: f32) -> u8 {
    // Float→int `as` saturates and maps NaN to 0 in Rust, so this can't misbehave.
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_effect_renders_exact_length_and_never_panics() {
        let mut out = Vec::new();
        for effect in Effect::ALL {
            for n in [0, 1, 2, 7, 204] {
                for t in [0.0, 1.5, 1e9, f32::NAN] {
                    let l = Lighting { effect, speed: f32::NAN, ..Default::default() };
                    render(&l, t, 0.3, &Inputs { audio: 0.5, ..Default::default() }, n, &mut out);
                    assert_eq!(out.len(), n);
                }
            }
        }
    }

    #[test]
    fn brightness_and_off() {
        let mut out = Vec::new();
        let l = Lighting { effect: Effect::Static, color: Rgb::WHITE, brightness: 0.0, ..Default::default() };
        render(&l, 0.0, 0.0, &Inputs { audio: 0.0, ..Default::default() }, 3, &mut out);
        assert!(out.iter().all(|c| *c == Rgb::BLACK));
        let l = Lighting { effect: Effect::Off, brightness: 1.0, ..Default::default() };
        render(&l, 0.0, 0.0, &Inputs { audio: 0.0, ..Default::default() }, 3, &mut out);
        assert!(out.iter().all(|c| *c == Rgb::BLACK));
        // Music: quiet shows a dim first colour, loud shows the full second colour.
        let l = Lighting { effect: Effect::Music, color: Rgb::new(255, 0, 0), color2: Rgb::new(0, 0, 255), brightness: 1.0, ..Default::default() };
        render(&l, 0.0, 0.0, &Inputs { audio: 1.0, ..Default::default() }, 1, &mut out);
        assert_eq!(out, vec![Rgb::new(0, 0, 255)]);
        render(&l, 0.0, 0.0, &Inputs { audio: 0.0, ..Default::default() }, 1, &mut out);
        assert!(out.first().is_some_and(|c| c.r > 0 && c.r < 60 && c.b == 0));
    }

    #[test]
    fn temperature_blends_cool_to_hot() {
        let mut out = Vec::new();
        let l = Lighting { effect: Effect::Temperature, color: Rgb::new(0, 0, 255), color2: Rgb::new(255, 0, 0), brightness: 1.0, ..Default::default() };
        let at = |cpu: Option<f32>, out: &mut Vec<Rgb>| {
            render(&l, 0.0, 0.0, &Inputs { cpu_temp: cpu, ..Default::default() }, 1, out);
            out.first().copied()
        };
        assert_eq!(at(Some(30.0), &mut out), Some(Rgb::new(0, 0, 255))); // below 40 °C: cool
        assert_eq!(at(Some(90.0), &mut out), Some(Rgb::new(255, 0, 0))); // above 80 °C: hot
        assert_eq!(at(Some(60.0), &mut out), Some(lerp(Rgb::new(0, 0, 255), Rgb::new(255, 0, 0), 0.5)));
        assert_eq!(at(None, &mut out), Some(Rgb::new(0, 0, 255))); // no sensor: cool colour
        assert_eq!(temp_fraction(Some(f32::NAN), 40.0, 80.0), 0.0);
        assert_eq!(temp_fraction(Some(50.0), 60.0, 60.0), 0.0); // equal thresholds don't divide by zero
    }

    #[test]
    fn colour_math() {
        assert_eq!(hsv(0.0, 1.0, 1.0), Rgb::new(255, 0, 0));
        assert_eq!(hsv(240.0, 1.0, 1.0), Rgb::new(0, 0, 255));
        assert_eq!(lerp(Rgb::BLACK, Rgb::WHITE, 1.0), Rgb::WHITE);
        assert_eq!(scale(Rgb::WHITE, 0.0), Rgb::BLACK);
        for c in [Rgb::new(255, 40, 120), Rgb::new(0, 200, 255), Rgb::new(10, 10, 10), Rgb::BLACK] {
            let (h, s, v) = to_hsv(c);
            assert_eq!(hsv(h, s, v), c);
        }
    }
}
