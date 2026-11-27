//! The administrator's switch for the two riskiest kinds of hardware: RAM sticks and the
//! graphics card. Both are off unless a root-owned file turns them on:
//!
//! ```text
//! /etc/rgbforge/hardware-allow      e.g. "ram gpu"
//! ```
//!
//! Only an administrator (sudo) can create or change it, so a program running as your
//! normal account can't turn RAM or GPU writes on, even though it can reach the
//! hardware service. Set it with `sudo rgbforge --allow-hardware ram,gpu`.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

pub const DIR: &str = "/etc/rgbforge";
pub const FILE: &str = "/etc/rgbforge/hardware-allow";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    pub ram: bool,
    pub gpu: bool,
}

impl Allow {
    pub fn text(self) -> String {
        match (self.ram, self.gpu) {
            (true, true) => "ram gpu",
            (true, false) => "ram",
            (false, true) => "gpu",
            (false, false) => "none",
        }
        .into()
    }
}

/// Words separated by spaces, commas or newlines; `#` starts a comment. Unknown words are
/// ignored (never treated as "allow").
pub fn parse(text: &str) -> Allow {
    let mut a = Allow::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("");
        for word in line.split(|c: char| c.is_whitespace() || c == ',') {
            match word.trim().to_ascii_lowercase().as_str() {
                "ram" => a.ram = true,
                "gpu" => a.gpu = true,
                _ => {}
            }
        }
    }
    a
}

/// What the administrator allowed. A missing file, or one that isn't root-owned and
/// write-protected (file and folder), allows nothing.
pub fn load() -> Allow {
    load_from(Path::new(FILE))
}

pub fn load_from(path: &Path) -> Allow {
    crate::drivers::root_owned_read(path).map(|t| parse(&t)).unwrap_or_default()
}

/// `--allow-hardware ram,gpu` / `none`. Must be run as root; the file is replaced atomically.
pub fn parse_arg(arg: &str) -> Option<Allow> {
    let mut a = Allow::default();
    for word in arg.split(',') {
        match word.trim().to_ascii_lowercase().as_str() {
            "ram" => a.ram = true,
            "gpu" => a.gpu = true,
            "none" => {}
            _ => return None,
        }
    }
    Some(a)
}

/// The file's text. Must stay within the 64-byte limit of the safe reader.
fn contents(a: Allow) -> String {
    format!("# sudo rgbforge --allow-hardware\n{}\n", a.text())
}

pub fn save(a: Allow) -> io::Result<()> {
    fs::create_dir_all(DIR)?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o755))?;
    let tmp = Path::new(DIR).join(".hardware-allow.tmp");
    let _ = fs::remove_file(&tmp);
    let mut f = fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(&tmp)?;
    f.write_all(contents(a).as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, FILE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_allowed_by_default() {
        assert_eq!(parse(""), Allow::default());
        assert_eq!(parse("# ram gpu\nyes please\ntrue\n1\n*"), Allow::default());
        assert_eq!(load_from(Path::new("/nonexistent/rgbforge/hardware-allow")), Allow::default());
    }

    #[test]
    fn administrator_choices_parse() {
        assert_eq!(parse("ram gpu\n"), Allow { ram: true, gpu: true });
        assert_eq!(parse("# comment\nRAM, # gpu\n"), Allow { ram: true, gpu: false });
        assert_eq!(parse_arg("gpu"), Some(Allow { ram: false, gpu: true }));
        assert_eq!(parse_arg("none"), Some(Allow::default()));
        assert_eq!(parse_arg("ram,everything"), None);
        for a in [Allow::default(), Allow { ram: true, gpu: false }, Allow { ram: true, gpu: true }] {
            assert_eq!(parse(&a.text()), a);
            // What `save` writes must be readable back by the 64-byte safe reader.
            assert!(contents(a).len() <= 64, "{}", contents(a).len());
            assert_eq!(parse(&contents(a)), a);
        }
    }

    #[test]
    fn a_file_the_user_could_write_allows_nothing() {
        // A temp dir is owned by whoever runs the tests; unless that is root it must be refused,
        // and even as root a group/world-writable file must be refused.
        let dir = std::env::temp_dir().join(format!("rgbforge-policy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let f = dir.join("hardware-allow");
        fs::write(&f, "ram gpu\n").unwrap();
        fs::set_permissions(&f, fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(load_from(&f), Allow::default(), "writable by others: refused");
        fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
        let root = std::fs::metadata(&f).map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0).unwrap();
        assert_eq!(load_from(&f), if root { Allow { ram: true, gpu: true } } else { Allow::default() });
        let _ = fs::remove_dir_all(&dir);
    }
}
