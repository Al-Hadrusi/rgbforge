#!/usr/bin/env bash
# Builds RGB Forge and installs it for the current user.
# The only step that needs sudo is the device-permission rule (you'll be asked).
set -euo pipefail
cd "$(dirname "$0")"

echo "==> Building (this takes a minute the first time)…"
cargo build --release

echo "==> Installing the app to ~/.local/bin"
install -Dm755 target/release/rgbforge "$HOME/.local/bin/rgbforge"
mkdir -p "$HOME/.local/share/applications"
# Use the full path: app menus often don't search ~/.local/bin.
sed "s|^Exec=.*|Exec=$HOME/.local/bin/rgbforge|" packaging/rgbforge.desktop > "$HOME/.local/share/applications/rgbforge.desktop"
update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true

echo "==> Allowing your user to talk to supported RGB controllers (needs sudo)"
target/release/rgbforge --print-udev-rules | sudo tee /etc/udev/rules.d/60-rgbforge.rules >/dev/null
# i2c-dev gives access to I2C buses as files (only the GPU's RGB bus is unlocked by the rule above).
echo i2c-dev | sudo tee /etc/modules-load.d/rgbforge-i2c.conf >/dev/null
sudo modprobe i2c-dev
sudo udevadm control --reload-rules
sudo udevadm trigger

echo
echo "Done! Open \"RGB Forge\" from your app menu, or run: rgbforge"
case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) echo "(Add ~/.local/bin to your PATH to run it from a terminal.)";; esac
