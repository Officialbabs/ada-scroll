#!/bin/sh
# Builds Scrollvil and installs it for this user: the program in ~/.local/bin
# and an entry in the app launcher. `./install.sh --remove` takes both away.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BIN="$HOME/.local/bin"
APPS="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
ENTRY="$APPS/scrollvil.desktop"

if [ "${1:-}" = "--remove" ]; then
    rm -f "$BIN/scrollvil" "$BIN/scrollvil-launch" "$ENTRY"
    echo "Scrollvil removed."
    exit 0
fi

command -v cargo >/dev/null || { echo "Scrollvil needs Rust's cargo to build." >&2; exit 1; }

# Few jobs: building Smithay in parallel takes a lot of memory.
(cd "$HERE" && cargo build --release -p scrollvil -j "${JOBS:-2}")

mkdir -p "$BIN" "$APPS"
install -m 755 "$HERE/target/release/scrollvil" "$BIN/scrollvil"
install -m 755 "$HERE/tools/launch.py" "$BIN/scrollvil-launch"

cat > "$ENTRY" <<EOF
[Desktop Entry]
Type=Application
Name=Scrollvil
GenericName=Scrolling desktop
Comment=Windows in columns on a strip that scrolls sideways
Exec=$BIN/scrollvil
Icon=view-paged-symbolic
Categories=System;
Actions=alt;

[Desktop Action alt]
Name=Open with Alt shortcuts
Exec=$BIN/scrollvil --mod alt
EOF

echo "Scrollvil installed: $BIN/scrollvil, and Scrollvil in the app launcher."
