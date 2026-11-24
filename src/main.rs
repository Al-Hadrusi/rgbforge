//! RGB Forge: control the RGB lighting in your Linux PC.
#![forbid(unsafe_code)]

mod audio;
mod config;
mod single;
mod tray;
mod drivers;
mod effects;
mod engine;
mod hidraw;
mod model;
mod ui;

use std::io::Write;
use std::process::ExitCode;

fn udev_rules() -> String {
    let mut out = String::from(
        "# RGB Forge: let the logged-in user access these RGB lighting controllers (and nothing else).\n",
    );
    for s in drivers::SUPPORTED {
        out.push_str(&format!(
            "# {}\nSUBSYSTEM==\"hidraw\", ATTRS{{idVendor}}==\"{:04x}\", ATTRS{{idProduct}}==\"{:04x}\", TAG+=\"uaccess\"\n",
            s.name, s.vid, s.pid
        ));
    }
    out.push_str("# Sapphire Nitro+ RX 9070 XT: only the GPU's dedicated RGB I2C bus, no other I2C bus\n");
    out.push_str("SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", ATTR{name}==\"AMDGPU DM i2c OEM bus*\", TAG+=\"uaccess\"\n");
    out.push_str("# G.Skill RAM lighting: the chipset SMBus. RGB Forge additionally refuses to use it unless\n");
    out.push_str("# the chipset's SPD Write Disable lock is on (hardware-blocks writes to memory-settings chips).\n");
    out.push_str("SUBSYSTEM==\"i2c-dev\", KERNEL==\"i2c-[0-9]*\", ATTR{name}==\"SMBus I801 adapter*\", TAG+=\"uaccess\"\n");
    out.push_str("# Each boot, as root, copy the SMBus HOSTC byte (holds the SPD Write Disable bit) to /run for RGB Forge to check.\n");
    out.push_str("ACTION==\"add|change|bind\", SUBSYSTEM==\"pci\", DRIVER==\"i801_smbus\", RUN+=\"/bin/sh -c 'mkdir -p /run/rgbforge && od -An -tx1 -j64 -N1 /sys%p/config > /run/rgbforge/smbus-hostc-%k'\"\n");
    out
}

fn main() -> ExitCode {
    // RGB Forge finds devices itself (read-only, from sysfs); hidapi is only used to open
    // a device it was handed, so its own scanning is switched off before first use.
    hidapi::HidApi::disable_device_discovery();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut minimized = false;
    match args.first().map(String::as_str) {
        Some("--minimized") => minimized = true,
        Some("--list-devices") => {
            println!("USB devices RGB Forge can see but doesn't support yet (send these IDs to request a driver):");
            for line in drivers::unsupported_usb() {
                println!("  {line}");
            }
            return ExitCode::SUCCESS;
        }
        Some("--music-check") => {
            println!("Listening to your PC's sound for 5 seconds (nothing is recorded). Play some music…");
            let Some(a) = audio::Audio::start() else {
                println!("Couldn't capture sound. Install the 'libpulse' package (provides parec).");
                return ExitCode::FAILURE;
            };
            let mut out = std::io::stdout().lock();
            for _ in 0..25 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let l = a.level();
                let bar = "█".repeat((l * 40.0).round() as usize);
                if writeln!(out, "{:>4.0}% {bar}", l * 100.0).is_err() {
                    break; // output closed (e.g. piped into `head`)
                }
            }
            return ExitCode::SUCCESS;
        }
        Some("--ram-check") => {
            print!("{}", drivers::ene_ram::check_report());
            return ExitCode::SUCCESS;
        }
        Some("--gpu-check") => {
            print!("{}", drivers::sapphire::check_report());
            return ExitCode::SUCCESS;
        }
        Some("--print-udev-rules") => {
            print!("{}", udev_rules());
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!("RGB Forge: control your PC's RGB lighting.\n\nUsage:\n  rgbforge                     open the app\n  rgbforge --minimized         open the app minimized (used when starting with your computer)\n  rgbforge --list-devices      list USB devices without a driver (for driver requests)\n  rgbforge --music-check       show live sound levels (checks the Music effect can hear your PC)\n  rgbforge --ram-check         read-only check of RAM lighting support\n  rgbforge --gpu-check         read-only check of graphics-card lighting support\n  rgbforge --print-udev-rules  print the permission rules (see Help in the app)");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("unknown option '{other}' (try --help)");
            return ExitCode::FAILURE;
        }
        None => {}
    }

    let guard = match single::acquire() {
        single::Instance::AlreadyRunning => {
            println!("RGB Forge is already running; showing its window.");
            return ExitCode::SUCCESS;
        }
        single::Instance::First(g) => g,
    };

    let mut core = ui::Core::new(guard);
    // Started at login with a tray available: stay in the tray until asked for the window.
    let mut open_now = !(minimized && core.has_tray());
    let mut first = true;
    loop {
        if !open_now && !core.wait_for_open() {
            break;
        }
        open_now = false;
        // Without a tray, a login start opens minimized instead.
        let start_minimized = first && minimized && !core.has_tray();
        first = false;
        let options = eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default()
                .with_title("RGB Forge")
                .with_app_id("rgbforge")
                .with_inner_size([1240.0, 820.0])
                .with_min_inner_size([900.0, 600.0]),
            ..Default::default()
        };
        let result = eframe::run_native(
            "RGB Forge",
            options,
            Box::new(|cc| Ok(Box::new(ui::App::new(cc, &mut core, start_minimized)))),
        );
        if let Err(e) = result {
            eprintln!("Couldn't open the window: {e}");
            return ExitCode::FAILURE;
        }
        // Window closed. With a tray icon, keep the lights running there; otherwise quit.
        if !core.has_tray() {
            break;
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    #[test]
    fn udev_rules_cover_allow_list() {
        let r = super::udev_rules();
        assert!(r.contains("idVendor}==\"0b05\", ATTRS{idProduct}==\"1867\""));
        assert_eq!(r.matches("TAG+=\"uaccess\"").count(), crate::drivers::SUPPORTED.len() + 2);
    }
}
