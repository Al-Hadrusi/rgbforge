#!/usr/bin/env bash
# Builds and installs RGB Forge.
#
# Security model: only the small, sandboxed `rgbforge-helper` system service may touch
# lighting hardware. The app (and every other program you run) gets NO direct device
# access; the app asks the service to show colours, and the service checks every request
# against each device's safety allow-list.
#
# Run as your normal user (not with sudo); it asks for your password for the system steps.
set -euo pipefail
cd "$(dirname "$0")"

if [ "$(id -u)" -eq 0 ]; then
    echo "Please run ./install.sh as your normal user, not with sudo (it asks for your password when needed)."
    exit 1
fi

BIN=/usr/local/bin/rgbforge

if [ -x ./rgbforge ] && [ ! -f Cargo.toml ]; then
    # Release download: the program is already built.
    SRC=./rgbforge
    echo "==> Using the included program"
else
    if ! command -v cargo >/dev/null; then
        echo "Rust isn't installed. Either download the ready-made release from"
        echo "https://github.com/Al-Hadrusi/rgbforge/releases, or install Rust (https://rustup.rs) and try again."
        exit 1
    fi
    echo "==> Building (this takes a minute the first time)…"
    cargo build --release
    SRC=target/release/rgbforge
fi

echo "==> Installing the program to $BIN (root-owned, so no other program can tamper with it)"
sudo install -o root -g root -m 755 "$SRC" "$BIN"
# Older versions lived in a folder your user can write to; remove that copy.
rm -f "$HOME/.local/bin/rgbforge"

if [ ! -f /etc/rgbforge/hardware-allow ]; then
    echo "==> RAM and graphics card lighting (off unless you allow them)"
    echo "    These write to your RAM sticks' and graphics card's lighting chips. RGB Forge only"
    echo "    sends lighting commands, but only allow them if you want to control that lighting."
    allow=""
    if [ -t 0 ] || [ -r /dev/tty ]; then
        read -r -p "    Allow RAM lighting control? [y/N] " a </dev/tty || a=n
        case "$a" in [yY]*) allow="ram" ;; esac
        read -r -p "    Allow graphics card lighting control? [y/N] " a </dev/tty || a=n
        case "$a" in [yY]*) allow="${allow:+$allow,}gpu" ;; esac
    fi
    sudo "$BIN" --allow-hardware "${allow:-none}"
    echo "    (Change later with: sudo rgbforge --allow-hardware ram,gpu   or   none)"
fi

echo "==> Adding RGB Forge to your app menu"
mkdir -p "$HOME/.local/share/applications"
sed "s|^Exec=.*|Exec=$BIN|" packaging/rgbforge.desktop > "$HOME/.local/share/applications/rgbforge.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
# "Start with my computer" entries from older versions point at the old location.
AUTOSTART="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/rgbforge.desktop"
if [ -f "$AUTOSTART" ]; then
    sed -i "s|^Exec=.*|Exec=\"$BIN\" --minimized|" "$AUTOSTART"
fi

echo "==> Removing old device permissions and installing the safety rule"
# This replaces the old rules that gave every program direct device access. The new file
# grants no access at all; it only records the RAM safety-lock state at boot (see README).
"$SRC" --print-udev-rules | sudo tee /etc/udev/rules.d/60-rgbforge.rules >/dev/null
# i2c-dev exposes I2C buses as device files, which only the hardware service may open.
echo i2c-dev | sudo tee /etc/modules-load.d/rgbforge-i2c.conf >/dev/null
sudo modprobe i2c-dev
sudo udevadm control --reload-rules
sudo udevadm trigger

echo "==> Allowing your account (and only yours) to use the hardware service"
# Only root and members of this group can connect to rgbforge-helper's socket.
sudo groupadd -f --system rgbforge
sudo usermod -aG rgbforge "$USER"

echo "==> Installing and starting the hardware service (rgbforge-helper)"
sudo install -o root -g root -m 644 packaging/rgbforge-helper.service /etc/systemd/system/rgbforge-helper.service
sudo systemctl daemon-reload
sudo systemctl enable rgbforge-helper >/dev/null
sudo systemctl restart rgbforge-helper

echo
if systemctl is-active --quiet rgbforge-helper; then
    echo "Done! The hardware service is running."
else
    echo "Installed, but the hardware service didn't start. See: systemctl status rgbforge-helper"
fi
echo "Open \"RGB Forge\" from your app menu, or run: rgbforge"
echo "Restart your PC (or log out and back in) once, so your account's new 'rgbforge' group membership takes effect."
