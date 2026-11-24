//! The lighting engine: a background thread that owns the devices, renders
//! effects at a fixed frame rate and pushes colours out. The GUI talks to it
//! through a channel and reads back a status snapshot, so a slow device can
//! never freeze the window.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::drivers::{DemoMode, Driver};
use crate::effects::{self, Lighting};
use crate::model::{DeviceInfo, Rgb};

pub const FPS: u32 = 30;
/// After this many consecutive failed writes, stop talking to a device.
const MAX_ERRORS: u32 = 5;

/// What to show on which device.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    pub sync_all: bool,
    pub global: Lighting,
    pub per_device: HashMap<String, Lighting>,
    pub disabled: HashSet<String>,
}

impl Default for Plan {
    fn default() -> Self {
        Self { sync_all: true, global: Lighting::default(), per_device: HashMap::new(), disabled: HashSet::new() }
    }
}

impl Plan {
    pub fn lighting_for(&self, id: &str) -> Lighting {
        if self.sync_all {
            return self.global;
        }
        self.per_device.get(id).copied().unwrap_or(self.global)
    }
}

pub enum Cmd {
    Plan(Plan),
    Rescan { demo: DemoMode, gpu: bool, ram: bool, zone_sizes: HashMap<String, Vec<u16>> },
    Resize { id: String, zone: usize, leds: u16 },
}

#[derive(Clone, Debug)]
pub struct DeviceStatus {
    pub info: DeviceInfo,
    /// Last colours per zone, for the on-screen preview.
    pub preview: Vec<Vec<Rgb>>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub devices: Vec<DeviceStatus>,
    pub problems: Vec<String>,
    pub scanning: bool,
    /// Smoothed music level 0..=1 (for previews).
    pub audio_level: f32,
    /// Set when Music is in use but no audio could be captured.
    pub audio_problem: Option<String>,
}

pub struct Engine {
    tx: Sender<Cmd>,
    status: Arc<Mutex<Status>>,
}

fn lock(m: &Mutex<Status>) -> MutexGuard<'_, Status> {
    // A panic elsewhere can't leave Status half-written in a harmful way, so recover.
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Smooth the raw music level: quick to rise, but it can fall at most ~3.3 full-scale steps
/// per second (≥300 ms), so beats pulse instead of strobing.
pub fn smooth_audio(env: f32, target: f32, dt: f32) -> f32 {
    let target = if target.is_finite() { target.clamp(0.0, 1.0) } else { 0.0 };
    if target > env {
        env + (target - env) * (dt * 25.0).min(1.0)
    } else {
        (env - dt / 0.3).max(target)
    }
}

impl Engine {
    /// A handle for sending commands from other threads (the tray icon).
    pub fn sender(&self) -> Sender<Cmd> {
        self.tx.clone()
    }

    pub fn start(plan: Plan, demo: DemoMode, gpu: bool, ram: bool, zone_sizes: HashMap<String, Vec<u16>>) -> Self {
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status { scanning: true, ..Default::default() }));
        let shared = Arc::clone(&status);
        // If the thread can't be spawned the UI simply shows no devices.
        let _ = thread::Builder::new()
            .name("rgb-engine".into())
            .spawn(move || run(rx, shared, plan, demo, gpu, ram, zone_sizes));
        Self { tx, status }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    pub fn status(&self) -> Status {
        lock(&self.status).clone()
    }
}

struct Slot {
    driver: Box<dyn Driver>,
    last_sent: Option<Vec<Vec<Rgb>>>,
    last_time: Option<Instant>,
    errors: u32,
    error: Option<String>,
}

fn scan(demo: DemoMode, gpu: bool, ram: bool, sizes: &HashMap<String, Vec<u16>>, status: &Mutex<Status>) -> Vec<Slot> {
    lock(status).scanning = true;
    let found = crate::remote::detect(demo, gpu, ram);
    let slots: Vec<Slot> = found
        .drivers
        .into_iter()
        .map(|mut driver| {
            if let Some(saved) = sizes.get(&driver.info().id) {
                for (zone, leds) in saved.iter().enumerate() {
                    driver.resize_zone(zone, *leds);
                }
            }
            Slot { driver, last_sent: None, last_time: None, errors: 0, error: None }
        })
        .collect();
    let mut s = lock(status);
    s.problems = found.problems;
    s.scanning = false;
    slots
}

fn run(
    rx: Receiver<Cmd>,
    status: Arc<Mutex<Status>>,
    mut plan: Plan,
    demo: DemoMode,
    gpu: bool,
    ram: bool,
    sizes: HashMap<String, Vec<u16>>,
) {
    let mut slots = scan(demo, gpu, ram, &sizes, &status);
    let start = Instant::now();
    let frame_time = Duration::from_secs(1) / FPS;
    let mut buf = Vec::new();
    let mut audio: Option<crate::audio::Audio> = None;
    let mut audio_failed = false;
    let mut env = 0.0f32;

    loop {
        let frame_start = Instant::now();
        loop {
            match rx.try_recv() {
                Ok(Cmd::Plan(p)) => plan = p,
                Ok(Cmd::Rescan { demo, gpu, ram, zone_sizes }) => slots = scan(demo, gpu, ram, &zone_sizes, &status),
                Ok(Cmd::Resize { id, zone, leds }) => {
                    if let Some(s) = slots.iter_mut().find(|s| s.driver.info().id == id) {
                        s.driver.resize_zone(zone, leds);
                        s.last_sent = None;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return, // window closed
            }
        }

        // Music: listen only while some controlled device actually uses it.
        let wants_music = slots.iter().any(|s| {
            let id = &s.driver.info().id;
            plan.lighting_for(id).effect == effects::Effect::Music && !plan.disabled.contains(id)
        });
        if wants_music {
            if audio.as_ref().is_some_and(|a| a.is_dead()) {
                audio = None;
            }
            if audio.is_none() && !audio_failed {
                audio = crate::audio::Audio::start();
                audio_failed = audio.is_none();
            }
        } else {
            audio = None;
            audio_failed = false;
        }
        let raw = audio.as_ref().map_or(0.0, |a| a.level());
        env = smooth_audio(env, raw, frame_time.as_secs_f32());
        {
            let mut s = lock(&status);
            s.audio_level = env;
            s.audio_problem = (wants_music && audio_failed).then(|| {
                "Music effect: couldn't listen to your PC's sound. Install the 'libpulse' package (provides parec).".to_string()
            });
        }

        let t = start.elapsed().as_secs_f32();
        let total_leds: usize = slots.iter().map(|s| s.driver.info().led_count()).sum::<usize>().max(1);
        let mut offset = 0usize;
        let mut statuses = Vec::with_capacity(slots.len());

        for slot in &mut slots {
            let info = slot.driver.info().clone();
            let lighting = plan.lighting_for(&info.id);
            let enabled = !plan.disabled.contains(&info.id);
            let mut frame = Vec::with_capacity(info.zones.len());
            for z in &info.zones {
                let n = usize::from(z.leds);
                // Phase is the position across *all* devices, so waves flow PC-wide.
                let phase = offset as f32 / total_leds as f32;
                effects::render(&lighting, t, phase, env, n, &mut buf);
                frame.push(buf.clone());
                offset = offset.saturating_add(n);
            }

            if enabled && slot.errors < MAX_ERRORS {
                let changed = slot.last_sent.as_ref() != Some(&frame);
                let stale = slot
                    .last_time
                    .is_none_or(|lt| lt.elapsed().as_secs_f32() >= slot.driver.keepalive_secs());
                let too_soon = slot
                    .last_time
                    .is_some_and(|lt| lt.elapsed().as_secs_f32() < slot.driver.min_interval_secs());
                if (changed || stale) && !too_soon {
                    match slot.driver.apply(&frame) {
                        Ok(()) => {
                            slot.errors = 0;
                            slot.error = None;
                            slot.last_sent = Some(frame.clone());
                            slot.last_time = Some(Instant::now());
                        }
                        Err(e) => {
                            slot.errors = slot.errors.saturating_add(1);
                            slot.error = Some(if slot.errors >= MAX_ERRORS {
                                format!("Stopped after repeated errors ({e}). Click Rescan to retry.")
                            } else {
                                e.to_string()
                            });
                        }
                    }
                }
            }
            statuses.push(DeviceStatus { info, preview: frame, error: slot.error.clone() });
        }
        lock(&status).devices = statuses;

        thread::sleep(frame_time.saturating_sub(frame_start.elapsed()));
    }
}

#[cfg(test)]
mod tests {
    use super::smooth_audio;

    #[test]
    fn music_can_pulse_but_never_strobe() {
        let dt = 1.0 / 30.0;
        // Rises quickly...
        let mut env = 0.0;
        for _ in 0..3 {
            env = smooth_audio(env, 1.0, dt);
        }
        assert!(env > 0.9, "{env}");
        // ...but a sudden drop to silence takes at least 300 ms to go dark.
        let mut frames = 0;
        while env > 0.0 {
            env = smooth_audio(env, 0.0, dt);
            frames += 1;
        }
        assert!(frames as f32 * dt >= 0.27, "fell in {frames} frames");
        assert_eq!(smooth_audio(0.5, f32::NAN, dt), smooth_audio(0.5, 0.0, dt));
    }
}
