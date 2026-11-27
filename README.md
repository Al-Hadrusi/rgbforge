# RGB Forge

A Linux app for controlling the RGB lighting in your PC, written from scratch in Rust.
No OpenRGB, no background services, no root.

- **Two clicks:** pick an effect, pick a colour. Settings save automatically and come back next time.
- **Effects:** Solid, Gradient, Breathe, Rainbow, Rainbow Wave (flows across *all* your devices), Comet,
  **Music** (pulses to whatever your PC plays; check it with `rgbforge --music-check`), Off.
- **Temperature:** colour follows your CPU or GPU temperature (cool colour → hot colour).
- **Tray icon:** closing the window keeps RGB Forge (and your effects) running in the tray. Left-click the icon to
  reopen the window; right-click for saved looks, lights off/on, and quit. With "Start with my computer", it starts in the tray.
- **Sync** everything, or switch Sync off and give each device its own look.
- **Saved looks:** store a setup ("Gaming", "Night") and switch back with one click.
- **Live preview** of every LED, and **demo devices** so you can try it with no hardware.

## Install

**Easiest:** download the ready-made `rgbforge-…-x86_64-linux.tar.gz` from
[Releases](https://github.com/Al-Hadrusi/rgbforge/releases/latest), then:

```bash
tar -xzf rgbforge-*-x86_64-linux.tar.gz && cd rgbforge-*-x86_64-linux && ./install.sh
```

**From source** (needs Rust):

```bash
./install.sh
```

Run it as your normal user (not with `sudo`); it asks for your password for the system steps. It:

1. builds RGB Forge and installs it to `/usr/local/bin/rgbforge` (root-owned, so nothing can tamper with it),
2. adds **RGB Forge** to your app menu,
3. installs and starts **`rgbforge-helper`**, the sandboxed system service that is the only thing
   allowed to talk to lighting hardware (see [Security](#security)).

You need Rust installed (<https://rustup.rs>). To update later: `git pull && ./install.sh`.
Upgrading from an older version? Restart once afterwards so its old device permissions are fully removed.

To try the app without installing (demo devices only): `cargo run --release`.

## Supported hardware

Talks directly to these USB lighting controllers, plus anything plugged into them
(ARGB fans, AIO pump/fan RGB, LED strips):

| Controller | USB ID |
|---|---|
| ASUS motherboard Aura controller (motherboard lights + ARGB headers, e.g. ROG Strix Z790) | 0b05:18f3, 0b05:1939, 0b05:19af, 0b05:1aa6 |
| ASUS Aura ARGB controller | 0b05:1867, 0b05:1872, 0b05:18a3, 0b05:18a5 |
| NZXT Kraken X53/X63/X73 (pump ring, logo, accessory channel) | 1e71:2007, 1e71:2014 |
| NZXT Kraken Z53/Z63/Z73 (accessory channel) | 1e71:3008 |
| NZXT Kraken Elite RGB (2024) | 1e71:3012 |
| NZXT Smart Device V2, HUE 2, HUE 2 Ambient | 1e71:2006, 200d, 200f, 2001, 2002 |
| NZXT RGB & Fan Controller (all versions), RGB Controller (2023) | 1e71:2009, 200e, 2010, 2011, 2019, 201f, 2020, 2022, 2012, 2021 |
| Lian Li UNI HUB SL V2 / AL V2 | 0cf2:a103, a104, a105 |
| Gigabyte RGB Fusion 2 (USB): board lighting + ARGB headers D_LED1/D_LED2 | 048d:5702, 8297, 5711, 8950 |
| ASRock Polychrome RGB (USB), one colour per header | 26ce:01a2, 01a6 |
| Razer Chroma (wired): BlackWidow Elite/2019/V3/V3 TKL/V3 Pro/V3 Mini/V4/V4 Pro/V4 75%/V4 X, Huntsman/Elite/TE/Mini/V2/V2 TKL/V2 Analog/V3 Pro, Ornata Chroma/V2/V3/V3 X, Cynosa Chroma/V2, Tartarus Pro/V2, DeathAdder V2/V2 Mini/Elite/Essential V2, Basilisk/V2/V3/V3 Pro/Ultimate, Viper/Mini/Ultimate (wired)/8KHz, Naga Trinity, Mamba Elite, Firefly V2/V2 Pro, Goliathus/Extended/3XL, Strider Chroma, Base Station Chroma/V2 | 1532:… (see `src/drivers/razer.rs`) |
| Corsair Lighting Node Pro | 1b1c:0c0b |
| Corsair Lighting Node Core | 1b1c:0c1a |
| Corsair Commander Pro (lighting only) | 1b1c:0c10 |
| Sapphire Nitro+ RX 9070 XT (opt-in, see below) | PCI 1002:7550, subsystem 1da2:e489 |

These drivers are written from the published protocol descriptions and are covered by
tests of the exact bytes they send, but they **have not yet been run on real hardware**.
They're marked *experimental* in the app.

Addressable headers can't report how many LEDs are connected, so select the device and
enter the counts under **LED SETUP** (e.g. 3 fans × 8 LEDs = 24).

### Sapphire Nitro+ RX 9070 XT (opt-in)

Off by default; switch on in **Help → Graphics card**. Run `rgbforge --gpu-check` first:
it only reads system information and reports whether everything is ready. The driver talks only to
this exact card, only on the GPU's dedicated RGB I²C bus (needs Linux 6.17+), only to the
lighting chip at 0x28, and only writes the colour/mode registers. Another owner of this card has
reported a freeze when other RGB software detected it, so save your work before enabling it.

### G.Skill Trident Z5 RGB memory (opt-in)

Off by default; switch on in **Help → Memory**. Run `rgbforge --ram-check` first (it only reads).
RGB Forge refuses to touch the RAM unless the chipset's **SPD Write Disable** hardware lock is on
(checked from the SMBus controller's own register each boot), refuses while OpenRGB runs, only
addresses the RGB chips at 0x70-0x77 after they identify themselves, never remaps addresses,
and cannot send the save-to-chip command.

### Cooler safety

AIO coolers and fan hubs use the same USB connection for pump and fan speed. Each of these
drivers has a byte-level allow-list: only its lighting commands can be sent, so pump/fan
speed, fan sync, LCD and firmware commands are unreachable (tests check this). The same applies
to motherboard controllers: save-to-flash, calibration and config-table writes are blocked.

### Deliberately not supported (yet)

| Device | Why |
|---|---|
| Corsair Commander Core / XT / ST (iCUE Elite AIOs) | Lighting requires "software mode", where the host appears to take over pump speed, and nothing documents what the pump does if it isn't told. Overheating risk can't be ruled out without hardware testing. |
| NZXT Kraken 2023 / 2023 Elite | The pump has no lighting controller (lights go through the separate RGB Controller, which is supported). |
| NZXT Kraken 2024 Plus | Different protocol whose effects save to the device. |
| Lian Li UNI HUB (original, SL v1, AL v1, SL-Infinity) | Need a different USB transport or have known firmware issues. |
| MSI Mystic Light | OpenRGB documents controllers being bricked by certain commands. |
| Razer wireless (via receiver), legacy-matrix and single-LED models | Need different command sets; not yet researched to the same standard. RGB Forge also skips Razer devices while OpenRazer or OpenRGB is running. |
| Gigabyte RGB Fusion 2 over SMBus (older boards) | Probing it has bricked Z390 boards. Only the USB version is supported. |
| Corsair DDR5 RGB RAM (Vengeance, Dominator Platinum/Titanium) | RGB chips sit next to the DDR5 power chip, which the chipset's write lock doesn't cover; documented lock-ups and boot failures. |
| Kingston FURY DDR5 RGB RAM | Identification needs page-select writes to the memory-settings chip, the documented DDR5 corruption mechanism. |
| Corsair DDR4 / Kingston FURY DDR4 RGB RAM | Both need writes before the chip identifies itself. If you own one, run `rgbforge --ram-check` (reads only) and share the output: it shows whether a safe, read-only identification is possible. |

Want your device added? Run `rgbforge --list-devices` and share the output.

### Other RAM and graphics cards: not yet

RAM and most GPUs are controlled over SMBus/I²C. That bus also carries the SPD chip that
tells your PC how to run its memory, and writing the wrong bytes there can **permanently
break** a RAM stick or motherboard. Doing it safely means writing a driver for each exact
chip and checking it against the real part, so they're left out until that can be done
per model.

## Safety

| | |
|---|---|
| No `unsafe` code | `unsafe_code = "forbid"`, a compile error if any is added |
| No crash points | `unwrap`, `expect`, `panic!` and unchecked indexing are denied by lints |
| Fixed allow-list | Only the USB IDs above are ever opened; every other device is ignored |
| Colour commands only | Never writes firmware or device flash/EEPROM |
| Least privilege | Only the sandboxed `rgbforge-helper` service touches hardware; see [Security](#security) |
| Gives up safely | After repeated errors it stops talking to a device until you click *Find devices* |
| No strobe effects | Avoids photosensitivity risks; every effect changes smoothly |
| Settings can't be corrupted | Atomic saves; a damaged file is kept as `.bak` and defaults load |

USB access uses plain reads and writes on `/dev/hidraw*`, except Gigabyte (HID feature reports via the
`hidapi` crate's pure-Rust backend); I²C uses the `i2cdev` crate. No C libraries for hardware access.

## Security

**Nothing but `rgbforge-helper` can reach your lighting hardware.** Earlier versions gave your user
direct access to the controllers and buses, which meant any program you ran had it too (including,
on some coolers, pump control). Now:

- **No device permissions for users.** The udev rule grants nothing; device nodes stay root-only.
- **A narrow service.** `rgbforge-helper` (systemd) accepts only *list devices*, *show these colours*,
  *set an LED count* and *run a read-only check* over a local socket. There is no "send raw bytes"
  request, and every write still passes the driver's allow-list. Only root and members of the
  `rgbforge` group (the installer adds your account) may connect, and even they can only change
  colours. The service also asks the kernel who is connecting and refuses anyone else even if the
  socket's permissions were ever mis-set.
- **Administrator switch for RAM and graphics cards.** Their drivers only start if the root-owned
  file `/etc/rgbforge/hardware-allow` allows them (`sudo rgbforge --allow-hardware ram,gpu`, or
  `none`; the installer asks once). A program running as your account can change colours, but
  can't turn RAM or GPU writes on. A file anyone but root could change counts as "allow nothing".
- **Audit trail.** Security-relevant actions (connections refused, device scans with the RAM/GPU
  opt-ins, LED-count changes, read-only checks, writes a safety list blocked) are logged with the
  caller's user and process ID: `journalctl -u rgbforge-helper | grep audit`. Colours and message
  contents are never logged. Messages are size-limited and validated; frames are trimmed
  to each device's real size; rate limits are enforced in the service itself.
- **Sandboxed.** The service has no capabilities, no network, no access to home folders, a read-only
  system view, can only open I²C and USB-HID device nodes, and runs with a restricted system-call set.
  `systemd-analyze security rgbforge-helper` rates it **1.1 / 10 ("OK")**.
- **Tamper-resistant install.** The binary the service runs lives in root-owned `/usr/local/bin`.
- **Supply chain.** `Cargo.lock` is committed; CI runs a RustSec dependency audit on every change and
  weekly, and Dependabot proposes updates.

Found a problem? See [SECURITY.md](SECURITY.md) to report it privately.

## Project layout

| File | What it does |
|---|---|
| `src/ui.rs` | The window |
| `src/engine.rs` | Background thread: renders effects at 30 fps and sends them to devices |
| `src/effects.rs` | Effect maths |
| `src/drivers/` | `aura.rs`, `corsair.rs`, `demo.rs` and the allow-list in `mod.rs` |
| `src/hidraw.rs` | Safe USB HID access |
| `src/helper.rs`, `src/proto.rs`, `src/remote.rs` | The hardware service, its message format, and the app side |
| `packaging/rgbforge-helper.service` | The sandboxed systemd service |
| `src/config.rs` | Settings and saved looks (`~/.config/rgbforge/settings.json`) |

## Development

```bash
cargo test
```
```bash
cargo clippy --all-targets
```

## License

GPL-3.0-only (see `LICENSE`). The device drivers are independent implementations of protocols
documented by the [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB) and
[liquidctl](https://github.com/liquidctl/liquidctl) projects, whose research made this possible.
