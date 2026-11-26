## Install (no compiling needed)

1. Download **`rgbforge-…-x86_64-linux.tar.gz`** below.
2. Unpack it, then run the installer **as your normal user** (not with `sudo`; it asks for your password itself):

```bash
tar -xzf rgbforge-*-x86_64-linux.tar.gz
cd rgbforge-*-x86_64-linux
./install.sh
```

3. Restart your PC once, then open **RGB Forge** from your app menu.

Optional: check the download.
```bash
sha256sum -c rgbforge-*-x86_64-linux.tar.gz.sha256
```
To verify it was built by this repository's GitHub workflow (needs the GitHub CLI):
```bash
gh attestation verify rgbforge-*-x86_64-linux.tar.gz --repo Al-Hadrusi/rgbforge
```

Works on 64-bit (x86_64) Linux distributions from about 2022 onwards (glibc 2.35+) with systemd, including Arch, Fedora, Ubuntu 22.04+ and Debian 12+.

## What's in it
- One app for your motherboard, ARGB fans, liquid coolers, GPU, RAM and peripherals.
- Effects: Solid, Gradient, Breathe, Rainbow, Rainbow Wave, Comet, **Music**, **Temperature** (follows CPU/GPU heat), Off.
- Sync every device or give each its own look; saved looks; tray icon; start with your computer.
- Supported hardware and its status: see the [README](https://github.com/Al-Hadrusi/rgbforge#supported-hardware). Drivers marked *experimental* haven't been confirmed on real hardware yet; please report how they work with the "Device works / doesn't work" issue form.

## Safety
Only a sandboxed system service (`rgbforge-helper`) can touch hardware, and every command is checked against a per-device allow-list. Pump/fan speed, firmware and memory-settings writes are unreachable. See [SECURITY.md](https://github.com/Al-Hadrusi/rgbforge/blob/main/SECURITY.md).
