#!/usr/bin/env bash
# setup.sh — build and register cosmic-ext-applet-timer with COSMIC
#
# Run once to make the applet visible in Panel Settings → Add Applet.
# The desktop entry points to target/release/ so dev-reload.sh just
# builds and restarts the panel — no copy step needed.
set -e

BINARY="cosmic-ext-applet-timer"
EXEC="$(pwd)/target/release/$BINARY"
DESKTOP_DIR="$HOME/.local/share/applications"
DESKTOP_FILE="$DESKTOP_DIR/com.krul.CosmicAppletTimer.desktop"

# ── Build ─────────────────────────────────────────────────────────────────────
echo "Building (release)…"
cargo build --release

# ── Desktop entry ─────────────────────────────────────────────────────────────
mkdir -p "$DESKTOP_DIR"
cat > "$DESKTOP_FILE" << EOF
[Desktop Entry]
Type=Application
Name=Timer
Comment=Pomodoro, sleep timer, eye rest, stretch reminders
Exec=$EXEC
Icon=alarm-symbolic
Categories=Utility;
NoDisplay=true
X-CosmicApplet=true
EOF
echo "Desktop entry → $DESKTOP_FILE  (Exec=$EXEC)"

if command -v update-desktop-database &>/dev/null; then
    update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
fi

echo ""
echo "Done. Add via: COSMIC Panel Settings → Add Applet → Timer"
echo "Future updates: ./dev-reload.sh  (builds + restarts panel, no copy needed)"
