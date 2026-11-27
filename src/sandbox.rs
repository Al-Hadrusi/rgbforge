//! The hardware service locks itself down after it starts listening, whatever started it.
//!
//! systemd already runs `rgbforge-helper` in a sandbox (see the unit file). OpenRC and other
//! init systems have nothing like it, so the service applies the parts it can itself:
//!
//! * **No root powers.** Every capability is dropped (effective, permitted, inheritable and
//!   the bounding set), and the "root regains powers" rules are switched off and locked.
//!   Device nodes are owned by root, so they still open; nothing else root could do works.
//! * **No new privileges**, ever (no setuid programs, no regaining capabilities).
//! * **Landlock file confinement** (Linux 5.13+, if the kernel has it enabled): the service
//!   can read `/etc`, `/sys`, `/proc` and `/run`, use device nodes in `/dev`, and nothing
//!   else: no home folders, no writing system files, no running programs.
//!
//! Must be called on the main thread before any other thread starts: all of this is
//! per-thread in Linux and is inherited by threads created afterwards.

use landlock::{ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules};
use rustix::thread::{
    CapabilitiesSecureBits, CapabilitySet, CapabilitySets, capability_is_in_bounding_set, remove_capability_from_bounding_set,
    set_capabilities, set_capabilities_secure_bits, set_no_new_privs,
};

/// What could be enforced; logged at start-up so the administrator can see it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub capabilities_dropped: bool,
    pub no_new_privs: bool,
    pub landlock: &'static str,
}

impl Report {
    pub fn line(&self) -> String {
        format!(
            "sandbox: capabilities {}, no-new-privileges {}, file access confinement (Landlock) {}",
            if self.capabilities_dropped { "dropped" } else { "NOT dropped" },
            if self.no_new_privs { "on" } else { "OFF" },
            self.landlock
        )
    }
}

/// Files the service may still use. Reads: configuration and account lists (`/etc`),
/// device identity (`/sys`), process checks (`/proc`), safety evidence (`/run`).
/// Read/write and device ioctls: lighting device nodes (`/dev`).
const READ_ONLY: [&str; 4] = ["/etc", "/sys", "/proc", "/run"];
const DEVICES: &str = "/dev";

pub fn harden() -> Report {
    // Landlock first: creating and applying a ruleset needs no privileges, but applying
    // it needs no-new-privileges (or CAP_SYS_ADMIN), which the crate sets for us.
    let landlock = confine_files(&READ_ONLY, DEVICES);
    let no_new_privs = set_no_new_privs(true).is_ok();
    let (none_held, bounding_empty) = drop_capabilities();
    // With no-new-privileges, nothing (setuid or file-capability programs) can grant
    // capabilities again, so an unprivileged caller that can't empty its bounding set is
    // still left with none. The service itself runs as root and empties it too.
    Report { capabilities_dropped: none_held && (bounding_empty || no_new_privs), no_new_privs, landlock }
}

fn confine_files(read_only: &[&str], devices: &str) -> &'static str {
    let abi = ABI::V5;
    let read = AccessFs::from_read(abi);
    let device = AccessFs::ReadFile | AccessFs::WriteFile | AccessFs::ReadDir | AccessFs::IoctlDev;
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .and_then(|r| r.create())
        .and_then(|r| r.add_rules(path_beneath_rules(read_only, read)))
        .and_then(|r| r.add_rules(path_beneath_rules([devices], device)))
        .and_then(|r| r.restrict_self());
    match status {
        Ok(s) => match s.ruleset {
            RulesetStatus::FullyEnforced => "fully enforced",
            RulesetStatus::PartiallyEnforced => "partly enforced (older kernel)",
            _ => "not available on this kernel",
        },
        Err(_) => "failed to apply",
    }
}

/// Drop every capability for good. Returns (none held, bounding set empty). Emptying the
/// bounding set needs CAP_SETPCAP, which root has.
fn drop_capabilities() -> (bool, bool) {
    // While we still hold CAP_SETPCAP: stop root from regaining capabilities (on exec or
    // by switching user ids), and lock that setting.
    let _ = set_capabilities_secure_bits(
        CapabilitiesSecureBits::NO_ROOT
            | CapabilitiesSecureBits::NO_ROOT_LOCKED
            | CapabilitiesSecureBits::NO_SETUID_FIXUP
            | CapabilitiesSecureBits::NO_SETUID_FIXUP_LOCKED
            | CapabilitiesSecureBits::KEEP_CAPS_LOCKED,
    );
    for bit in 0..64u32 {
        let cap = CapabilitySet::from_bits_retain(1u64 << bit);
        if capability_is_in_bounding_set(cap).unwrap_or(false) {
            let _ = remove_capability_from_bounding_set(cap);
        }
    }
    let none = CapabilitySet::empty();
    let cleared = set_capabilities(None, CapabilitySets { effective: none, permitted: none, inheritable: none }).is_ok();
    let bounding_empty = (0..64u32).all(|bit| !capability_is_in_bounding_set(CapabilitySet::from_bits_retain(1u64 << bit)).unwrap_or(false));
    (cleared, bounding_empty)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;

    /// Everything here is per-thread, so each test hardens a fresh thread of its own and
    /// the rest of the test run is unaffected.
    fn in_hardened_thread<T: Send + 'static>(f: impl FnOnce(Report) -> T + Send + 'static) -> T {
        std::thread::spawn(move || f(harden())).join().unwrap()
    }

    #[test]
    fn no_capabilities_or_new_privileges_remain() {
        let had_setpcap = rustix::thread::capabilities(None).unwrap().effective.contains(CapabilitySet::SETPCAP);
        let (report, caps, bounding) = in_hardened_thread(|r| {
            let b = (0..64u32).filter(|bit| capability_is_in_bounding_set(CapabilitySet::from_bits_retain(1u64 << bit)).unwrap_or(false)).count();
            (r, rustix::thread::capabilities(None).unwrap(), b)
        });
        assert!(report.no_new_privs, "{}", report.line());
        assert!(report.capabilities_dropped, "{}", report.line());
        assert!(caps.effective.is_empty() && caps.permitted.is_empty() && caps.inheritable.is_empty());
        if had_setpcap {
            // As root (like the real service): the bounding set is emptied too.
            assert_eq!(bounding, 0, "bounding set must be empty");
        }
    }

    #[test]
    fn files_outside_the_allowed_folders_are_out_of_reach_when_landlock_is_available() {
        // A file in a folder the service has no business with (like a home folder).
        let dir = std::env::temp_dir().join(format!("rgbforge-sandbox-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("secret.txt");
        fs::write(&secret, "private").unwrap();
        let (landlock, read_secret, write_secret, read_etc) = in_hardened_thread(move |r| {
            (r.landlock, fs::read_to_string(&secret).is_ok(), fs::write(&secret, "x").is_ok(), fs::read_to_string("/etc/passwd").is_ok())
        });
        assert!(read_etc, "allowed reads keep working");
        if landlock == "fully enforced" || landlock.starts_with("partly") {
            assert!(!read_secret && !write_secret, "Landlock {landlock}: outside files must be unreachable");
        } else {
            eprintln!("Landlock {landlock} here: confinement not testable on this kernel");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn report_line_says_what_is_missing() {
        let r = Report { capabilities_dropped: false, no_new_privs: true, landlock: "not available on this kernel" };
        assert_eq!(r.line(), "sandbox: capabilities NOT dropped, no-new-privileges on, file access confinement (Landlock) not available on this kernel");
    }
}
