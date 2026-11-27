#!/usr/bin/env bash
# Builds and installs RGB Forge.
#
# Security model: only the small, sandboxed `rgbforge-helper` system service may touch
# lighting hardware. The app (and every other program you run) gets NO direct device
# access; the app asks the service to show colours, and the service checks every request
# against each device's safety allow-list.
#
# Run as your normal user (not with sudo); it asks for your password for the system steps.
set -Eeuo pipefail
cd "$(dirname "$0")"
# If any step fails, say which one instead of stopping silently halfway.
trap 'echo; echo "STOPPED: the step on line $LINENO of install.sh failed (see the message above). Nothing after it was done; fix the problem and run ./install.sh again." >&2' ERR

if [ "$(id -u)" -eq 0 ]; then
    echo "Please run ./install.sh as your normal user, not with sudo (it asks for your password when needed)."
    exit 1
fi

BIN=/usr/local/bin/rgbforge

# Which init system runs this PC: the hardware service is installed for that one.
if [ -d /run/systemd/system ]; then
    INIT=systemd
elif command -v openrc-run >/dev/null 2>&1 && [ -d /run/openrc ]; then
    INIT=openrc
else
    echo "RGB Forge's hardware service needs systemd or OpenRC, and neither seems to be running here."
    echo "Nothing was changed. Please open an issue saying which init system you use."
    exit 1
fi

# sudo, or doas (common on Gentoo and Alpine).
if command -v sudo >/dev/null 2>&1; then
    as_root() { sudo "$@"; }
elif command -v doas >/dev/null 2>&1; then
    as_root() { doas "$@"; }
else
    echo "Please install sudo or doas first; the installer needs it for the system steps."
    exit 1
fi
echo "==> Hardware service will be installed for: $INIT"

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
as_root install -o root -g root -m 755 "$SRC" "$BIN"
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
    as_root "$BIN" --allow-hardware "${allow:-none}"
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
# Some systems (e.g. Gentoo with OpenRC) don't create these folders by default.
as_root mkdir -p /etc/udev/rules.d /etc/modules-load.d
"$SRC" --print-udev-rules | as_root tee /etc/udev/rules.d/60-rgbforge.rules >/dev/null
# i2c-dev exposes I2C buses as device files, which only the hardware service may open.
echo i2c-dev | as_root tee /etc/modules-load.d/rgbforge-i2c.conf >/dev/null
as_root modprobe i2c-dev
as_root udevadm control --reload-rules
as_root udevadm trigger

echo "==> Allowing your account (and only yours) to use the hardware service"
# Only root and members of this group can connect to rgbforge-helper's socket.
as_root groupadd -f --system rgbforge
as_root usermod -aG rgbforge "$USER"

echo "==> Installing and starting the hardware service (rgbforge-helper)"
if [ "$INIT" = systemd ]; then
    as_root install -o root -g root -m 644 packaging/rgbforge-helper.service /etc/systemd/system/rgbforge-helper.service
    as_root systemctl daemon-reload
    as_root systemctl enable rgbforge-helper >/dev/null
    as_root systemctl restart rgbforge-helper
    running() { systemctl is-active --quiet rgbforge-helper; }
    status_cmd="systemctl status rgbforge-helper"
else
    as_root install -o root -g root -m 755 packaging/rgbforge-helper.openrc /etc/init.d/rgbforge-helper
    as_root rc-update add rgbforge-helper default >/dev/null
    as_root rc-service rgbforge-helper restart
    running() { as_root rc-service rgbforge-helper status >/dev/null 2>&1; }
    status_cmd="rc-service rgbforge-helper status; cat /var/log/rgbforge-helper.log"
fi

echo
if running; then
    echo "Done! The hardware service is running."
else
    echo "Installed, but the hardware service didn't start. See: $status_cmd"
fi
echo "Open \"RGB Forge\" from your app menu, or run: rgbforge"
echo "Restart your PC (or log out and back in) once, so your account's new 'rgbforge' group membership takes effect."
