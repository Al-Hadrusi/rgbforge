//! Settings and profiles, saved as JSON in `~/.config/rgbforge/settings.json`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::drivers::DemoMode;
use crate::engine::Plan;

/// Refuse to load absurdly large settings files.
const MAX_FILE: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub plan: Plan,
    pub demo: DemoMode,
    pub zone_sizes: HashMap<String, Vec<u16>>,
    pub profiles: BTreeMap<String, Plan>,
    pub seen_welcome: bool,
    /// Device that was open last time.
    pub last_selected: Option<String>,
    /// User explicitly enabled graphics-card lighting control.
    pub gpu_enabled: bool,
    /// User explicitly enabled RAM lighting control.
    pub ram_enabled: bool,
}

pub fn path() -> Option<PathBuf> {
    Some(config_dir()?.join("rgbforge").join("settings.json"))
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// The standard XDG autostart entry (works on KDE, GNOME, XFCE, Cinnamon, …).
fn autostart_file() -> Option<PathBuf> {
    Some(config_dir()?.join("autostart").join("rgbforge.desktop"))
}

pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|p| p.exists())
}

pub fn autostart_entry(exe: &std::path::Path) -> String {
    // Desktop-entry quoting: wrap in double quotes, escape backslash, quote, backtick and dollar.
    let mut quoted = String::from("\"");
    for c in exe.to_string_lossy().chars() {
        if matches!(c, '\\' | '"' | '`' | '$') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    format!(
        "[Desktop Entry]\nType=Application\nName=RGB Forge\nComment=Restore your RGB lighting at login\nExec={quoted} --minimized\nIcon=preferences-desktop-color\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    )
}

pub fn set_autostart(on: bool) -> io::Result<()> {
    let p = autostart_file().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    if on {
        let exe = std::env::current_exe()?;
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&p, autostart_entry(&exe))
    } else {
        match fs::remove_file(&p) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

/// Load settings; a missing or damaged file just gives defaults (damaged files are kept as `.bak`).
pub fn load() -> Settings {
    let Some(p) = path() else { return Settings::default() };
    let Ok(meta) = fs::metadata(&p) else { return Settings::default() };
    if meta.len() > MAX_FILE {
        return Settings::default();
    }
    match fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(s) => s,
        None => {
            let _ = fs::rename(&p, p.with_extension("json.bak"));
            Settings::default()
        }
    }
}

/// Save atomically (write a temp file, then rename) so a crash can't corrupt settings.
pub fn save(s: &Settings) -> io::Result<()> {
    let p = path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home directory"))?;
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(s).map_err(io::Error::other)?;
    let tmp = p.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &p)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tolerant_of_old_files() {
        let mut s = Settings::default();
        s.profiles.insert("Gaming".into(), Plan::default());
        s.zone_sizes.insert("aura:x".into(), vec![16, 8]);
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
        // Missing fields fall back to defaults instead of failing.
        let partial: Settings = serde_json::from_str(r#"{"seen_welcome": true}"#).unwrap();
        assert!(partial.seen_welcome);
    }

    #[test]
    fn autostart_entry_quotes_path() {
        let e = autostart_entry(std::path::Path::new("/home/me/my apps/rgb\"forge"));
        assert!(e.contains("Exec=\"/home/me/my apps/rgb\\\"forge\" --minimized\n"), "{e}");
    }
}
