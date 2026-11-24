#!/bin/bash
# Start (or restart) the camofox-browser server. Used by the browser
# control page's Start button. Mirrors the start block in
# camofox-browser/turnstile/install.sh so the server comes up the same way.
set -u
REPO="/data/workspace/camofox-browser"
LOG="/data/workspace/camofox-browser.log"
PORT="${CAMOFOX_PORT:-9377}"
DISPLAY_NUM="${DISPLAY_NUM:-:1}"
BIN="${CAMOUFOX_EXECUTABLE:-/data/home/.cache/camoufox/camoufox-bin}"
if [ ! -x "$BIN" ]; then
  BIN=$(find /data/home/.cache/camoufox /root/.cache/camoufox "$HOME/.cache/camoufox" -name 'camoufox-bin' -type f 2>/dev/null | head -1)
fi
pkill -f "node server.js" 2>/dev/null || true
sleep 2
cd "$REPO" || exit 1
nohup env DISPLAY="$DISPLAY_NUM" NODE_ENV=development CAMOFOX_PORT="$PORT" \
  CAMOFOX_INTERACTIVE=desktop CAMOFOX_BIND_HOST=127.0.0.1 \
  CAMOUFOX_EXECUTABLE="$BIN" CAMOFOX_DISABLE_DEFAULT_ADDONS=1 \
  CAMOFOX_CRASH_REPORT_ENABLED=false \
  BROWSER_RSS_RESTART_THRESHOLD_MB=6000 NATIVE_MEM_RESTART_THRESHOLD_MB=3000 \
  node server.js >> "$LOG" 2>&1 &
echo "browser-stack: server pid $!"
