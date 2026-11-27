## Install (no compiling needed)

**Recommended: the verifying installer.** It downloads this release and refuses to unpack anything
unless the file matches GitHub's own checksum and its signature proves it was built by this
repository's release workflow:

```bash
curl -fsSLO https://raw.githubusercontent.com/Al-Hadrusi/rgbforge/main/get.sh
less get.sh      # read it first
bash get.sh
```

The signature check needs the GitHub CLI, logged in (`gh auth login`), or `cosign`.

**Manual:** download **`rgbforge-…-x86_64-linux.tar.gz`** below and verify it before unpacking:

```bash
gh attestation verify rgbforge-*-x86_64-linux.tar.gz --repo Al-Hadrusi/rgbforge --signer-workflow Al-Hadrusi/rgbforge/.github/workflows/release.yml
tar -xzf rgbforge-*-x86_64-linux.tar.gz
cd rgbforge-*-x86_64-linux
./install.sh     # as your normal user, not with sudo
```

Without the GitHub CLI, `cosign` can check the `.sigstore.json` file attached below:

```bash
cosign verify-blob-attestation --new-bundle-format --bundle rgbforge-*-x86_64-linux.tar.gz.sigstore.json \
  --type https://slsa.dev/provenance/v1 --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github.com/Al-Hadrusi/rgbforge/\.github/workflows/release\.yml@refs/tags/v' \
  rgbforge-*-x86_64-linux.tar.gz
```

Then restart your PC once and open **RGB Forge** from your app menu.

Works on 64-bit (x86_64) Linux distributions from about 2022 onwards (glibc 2.35+) with systemd or OpenRC, including Arch, Fedora, Ubuntu 22.04+, Debian 12+ and Gentoo. Needs `sudo` or `doas`.

## What's in it
- One app for your motherboard, ARGB fans, liquid coolers, GPU, RAM and peripherals.
- Effects: Solid, Gradient, Breathe, Rainbow, Rainbow Wave, Comet, **Music**, **Temperature** (follows CPU/GPU heat), Off.
- Sync every device or give each its own look; saved looks; tray icon; start with your computer.
- Supported hardware and its status: see the [README](https://github.com/Al-Hadrusi/rgbforge#supported-hardware). Drivers marked *experimental* haven't been confirmed on real hardware yet; please report how they work with the "Device works / doesn't work" issue form.

## Safety
Only a sandboxed system service (`rgbforge-helper`) can touch hardware, and every command is checked against a per-device allow-list. Pump/fan speed, firmware and memory-settings writes are unreachable. See [SECURITY.md](https://github.com/Al-Hadrusi/rgbforge/blob/main/SECURITY.md).
