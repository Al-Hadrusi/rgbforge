//! Music-reactive lighting: how loud (and how bassy) your PC's sound output is right now.
//!
//! We read what's *playing* (the default output's monitor), never the microphone, by running
//! the standard `parec` tool (PulseAudio/PipeWire), or PipeWire's own `pw-record` as a fallback.
//! Samples are analysed in memory and thrown away; nothing is recorded or stored.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

const RATE: u32 = 22_050;

fn candidates() -> Vec<Command> {
    let rate = RATE.to_string();
    let mut parec = Command::new("parec");
    parec.args(["--raw", "--format=s16le", "--channels=1", "--latency-msec=30", "-d", "@DEFAULT_MONITOR@"]);
    parec.arg(format!("--rate={rate}"));
    let mut pw = Command::new("pw-record");
    pw.args(["-P", "{ stream.capture.sink = true }", "--raw", "--format", "s16", "--channels", "1", "--rate", &rate, "-"]);
    vec![parec, pw]
}

pub struct Audio {
    child: Child,
    level: Arc<AtomicU32>,
    dead: Arc<AtomicBool>,
}

impl Audio {
    /// Start listening. `None` if no capture tool works on this system.
    pub fn start() -> Option<Self> {
        for mut cmd in candidates() {
            let Ok(mut child) = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else {
                continue; // tool not installed
            };
            thread::sleep(Duration::from_millis(250));
            if !matches!(child.try_wait(), Ok(None)) {
                continue; // exited straight away: wrong options or no audio server
            }
            let Some(stdout) = child.stdout.take() else {
                let _ = child.kill();
                let _ = child.wait();
                continue;
            };
            let level = Arc::new(AtomicU32::new(0));
            let dead = Arc::new(AtomicBool::new(false));
            let (l, d) = (Arc::clone(&level), Arc::clone(&dead));
            let _ = thread::Builder::new().name("rgbforge-audio".into()).spawn(move || analyse(stdout, &l, &d));
            return Some(Self { child, level, dead });
        }
        None
    }

    /// Loudness 0..=1 (auto-levelled, bass-weighted).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Running analysis state, kept separate so it can be unit-tested.
pub struct Analyser {
    low: f32,
    peak: f32,
}

impl Default for Analyser {
    fn default() -> Self {
        Self { low: 0.0, peak: 0.05 }
    }
}

impl Analyser {
    /// Feed a block of samples (-1..1); returns the block's level 0..=1.
    pub fn block(&mut self, samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        // One-pole low-pass at ~150 Hz to emphasise bass/kick drums.
        let alpha = 1.0 - (-std::f32::consts::TAU * 150.0 / RATE as f32).exp();
        let (mut bass, mut full) = (0.0f32, 0.0f32);
        for &x in samples {
            let x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
            self.low += alpha * (x - self.low);
            bass += self.low * self.low;
            full += x * x;
        }
        let n = samples.len() as f32;
        let energy = (0.7 * (bass / n).sqrt() + 0.3 * (full / n).sqrt()).max(0.0);
        // Auto-level: follow the loudest recent block, slowly forgetting it.
        self.peak = (self.peak * 0.995).max(energy).max(0.02);
        if energy < 0.003 {
            return 0.0; // silence / noise floor
        }
        (energy / self.peak).clamp(0.0, 1.0)
    }
}

fn analyse(mut src: impl Read, level: &AtomicU32, dead: &AtomicBool) {
    let mut bytes = vec![0u8; 1024]; // 512 samples ≈ 23 ms
    let mut samples = Vec::with_capacity(512);
    let mut a = Analyser::default();
    while src.read_exact(&mut bytes).is_ok() {
        samples.clear();
        samples.extend(bytes.chunks_exact(2).map(|b| f32::from(i16::from_le_bytes(<[u8; 2]>::try_from(b).unwrap_or([0, 0]))) / 32768.0));
        level.store(a.block(&samples).to_bits(), Ordering::Relaxed);
    }
    level.store(0f32.to_bits(), Ordering::Relaxed);
    dead.store(true, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_dark_and_loud_bass_is_bright() {
        let mut a = Analyser::default();
        assert_eq!(a.block(&vec![0.0; 512]), 0.0);
        let bass: Vec<f32> = (0..512).map(|i| (i as f32 * std::f32::consts::TAU * 60.0 / RATE as f32).sin() * 0.8).collect();
        let mut last = 0.0;
        for _ in 0..5 {
            last = a.block(&bass);
        }
        assert!(last > 0.8, "{last}");
        assert_eq!(a.block(&[]), 0.0);
        // Garbage samples can't poison the analyser.
        assert!(a.block(&[f32::NAN, f32::INFINITY]).is_finite());
        assert!(a.block(&bass) > 0.5);
    }
}
