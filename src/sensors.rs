//! CPU and GPU temperatures for the Temperature effect.
//!
//! Read-only, unprivileged, and from the kernel's standard hwmon files
//! (`/sys/class/hwmon/hwmonN/{name,tempK_label,tempK_input}`): no hardware is touched.

use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Temps {
    /// °C, if a known CPU sensor was found.
    pub cpu: Option<f32>,
    /// °C, if a known GPU sensor was found.
    pub gpu: Option<f32>,
}

/// CPU sensor drivers, and the reading that best represents the whole CPU.
const CPU: &[(&str, &[&str])] = &[("coretemp", &["Package id 0"]), ("k10temp", &["Tctl", "Tdie"]), ("zenpower", &["Tdie", "Tctl"])];
/// GPU sensor drivers. (NVIDIA's own driver doesn't publish hwmon temperatures.)
const GPU: &[(&str, &[&str])] = &[("amdgpu", &["edge", "junction"]), ("nouveau", &[]), ("i915", &[]), ("xe", &[])];

pub fn read() -> Temps {
    read_from(Path::new("/sys/class/hwmon"))
}

pub fn read_from(root: &Path) -> Temps {
    let mut t = Temps::default();
    let Ok(entries) = fs::read_dir(root) else { return t };
    let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let Ok(name) = fs::read_to_string(dir.join("name")) else { continue };
        let name = name.trim();
        if t.cpu.is_none()
            && let Some((_, labels)) = CPU.iter().find(|(n, _)| *n == name)
        {
            t.cpu = sensor(&dir, labels);
        }
        if t.gpu.is_none()
            && let Some((_, labels)) = GPU.iter().find(|(n, _)| *n == name)
        {
            t.gpu = sensor(&dir, labels);
        }
    }
    t
}

/// The first input whose label matches (in preference order), else `temp1`.
fn sensor(dir: &Path, labels: &[&str]) -> Option<f32> {
    let inputs: Vec<(String, f32)> = (1..=32)
        .filter_map(|k| {
            let value = fs::read_to_string(dir.join(format!("temp{k}_input"))).ok()?;
            let label = fs::read_to_string(dir.join(format!("temp{k}_label"))).unwrap_or_default();
            let milli: f32 = value.trim().parse().ok()?;
            let c = milli / 1000.0;
            // Ignore obviously broken readings.
            (c.is_finite() && (-40.0..=150.0).contains(&c)).then(|| (label.trim().to_string(), c))
        })
        .collect();
    labels
        .iter()
        .find_map(|want| inputs.iter().find(|(l, _)| l == want).map(|(_, c)| *c))
        .or_else(|| inputs.first().map(|(_, c)| *c))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn hwmon(root: &Path, n: u32, name: &str, temps: &[(&str, &str)]) {
        let d = root.join(format!("hwmon{n}"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("name"), format!("{name}\n")).unwrap();
        for (k, (label, value)) in temps.iter().enumerate() {
            fs::write(d.join(format!("temp{}_label", k + 1)), format!("{label}\n")).unwrap();
            fs::write(d.join(format!("temp{}_input", k + 1)), format!("{value}\n")).unwrap();
        }
    }

    #[test]
    fn reads_intel_cpu_and_amd_gpu_like_the_users_pc() {
        let root = std::env::temp_dir().join(format!("rgbforge-hwmon-{}", std::process::id()));
        hwmon(&root, 0, "nvme", &[("Composite", "38850")]);
        hwmon(&root, 1, "coretemp", &[("Core 0", "51000"), ("Package id 0", "56000")]);
        hwmon(&root, 2, "amdgpu", &[("edge", "47000"), ("junction", "61000"), ("mem", "70000")]);
        let t = read_from(&root);
        fs::remove_dir_all(&root).ok();
        assert_eq!(t, Temps { cpu: Some(56.0), gpu: Some(47.0) });
    }

    #[test]
    fn amd_cpu_broken_values_and_missing_sensors() {
        let root = std::env::temp_dir().join(format!("rgbforge-hwmon2-{}", std::process::id()));
        hwmon(&root, 0, "k10temp", &[("Tccd1", "60000"), ("Tctl", "72500")]);
        hwmon(&root, 1, "amdgpu", &[("edge", "999999"), ("junction", "garbage")]);
        let t = read_from(&root);
        fs::remove_dir_all(&root).ok();
        assert_eq!(t, Temps { cpu: Some(72.5), gpu: None });
        assert_eq!(read_from(Path::new("/nonexistent")), Temps::default());
    }
}
