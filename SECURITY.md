# Security policy

RGB Forge talks to hardware, so we treat **hardware-safety problems as security problems**.
Please report privately:

- anything that lets another program (or user) control hardware through RGB Forge beyond
  changing colours,
- any way to make RGB Forge or `rgbforge-helper` send a command outside a driver's allow-list
  (for example pump/fan speed, save-to-flash, firmware, or anything to RAM settings chips),
- crashes or hangs of `rgbforge-helper` caused by input from its socket,
- problems in the installer, the systemd or OpenRC service or the udev rule.

## How to report

Use **GitHub's private vulnerability reporting**: the repository's *Security* tab →
*Report a vulnerability*. Please don't open a public issue for these.

Include what you did, what happened, your hardware (`rgbforge --list-devices` output and
the in-app device report help), and your distribution and kernel version.

You'll get an acknowledgement within 7 days. Fixes for confirmed issues are released as soon
as possible, and you'll be credited unless you'd rather not be.

## Security design (summary)

- Only `rgbforge-helper` (a service that sandboxes itself; under systemd also sandboxed by the unit) can access lighting hardware. Users and
  other programs get no device permissions; the helper accepts only "list devices / show these
  colours / change an LED count / run a read-only check" requests.
- Every driver checks every outgoing write against a byte-level allow-list; cooling, flash,
  firmware and address-change commands are unreachable, and tests enforce this.
- RAM lighting requires the chipset's SPD Write Disable lock, never addresses SPD/PMIC ranges,
  and identifies chips with reads before any write.
- No `unsafe` code in RGB Forge itself (compiler-enforced); CI runs strict lints, all tests
  and a RustSec dependency audit on every change and weekly.

## Hardware allow-lists

What each driver may send, what it was checked against, and what is still open: [docs/ALLOWLIST-REVIEW.md](docs/ALLOWLIST-REVIEW.md). Reviews of any protocol are welcome.
