#!/usr/bin/env bash
# Builds unibz terminal and installs it for the current user.
#   ./install.sh              install to ~/.local (binary + desktop entry)
#   ./install.sh --uninstall  remove it again (keeps your config, tokens and downloaded files)
#   PREFIX=/usr/local sudo -E ./install.sh   system-wide
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
APP_ID="it.unibz.Terminal"
BIN="$PREFIX/bin/unibz-terminal"
DESKTOP="$PREFIX/share/applications/$APP_ID.desktop"
cd "$(dirname "$0")"

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "$BIN" "$DESKTOP"
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true
    echo "Removed. Config, tokens and downloads are left in ~/.config/unibz-terminal and ~/Downloads/unibz."
    exit 0
fi

missing=()
command -v cargo >/dev/null || missing+=("cargo (install rustup from https://rustup.rs)")
for lib in gtk4 libadwaita-1 webkitgtk-6.0; do
    pkg-config --exists "$lib" 2>/dev/null || missing+=("$lib development files")
done
if ((${#missing[@]})); then
    printf 'Missing:\n'; printf '  - %s\n' "${missing[@]}"
    echo "Arch: pacman -S rustup gtk4 libadwaita webkitgtk-6.0"
    echo "Debian/Ubuntu: apt install cargo libgtk-4-dev libadwaita-1-dev libwebkitgtk-6.0-dev"
    echo "Fedora: dnf install cargo gtk4-devel libadwaita-devel webkitgtk6.0-devel"
    exit 1
fi

cargo build --release --locked
install -Dm755 target/release/unibz-terminal "$BIN"
install -Dm644 /dev/stdin "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=unibz terminal
Comment=mySNS, OLE and Teams in one place
Exec=$BIN
Icon=accessories-dictionary
Terminal=false
Categories=Education;
StartupWMClass=$APP_ID
EOF
command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" || true

echo "Installed $BIN"
case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "Note: $PREFIX/bin is not on your PATH; start it from the app menu or add it." ;; esac
