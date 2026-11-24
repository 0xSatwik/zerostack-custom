#!/bin/bash
# Stop the camofox-browser server (node server.js). Used by the browser
# control page's Stop button. The server idle-shuts-down on its own, but
# this is the explicit stop.
pkill -f "node server.js" 2>/dev/null || true
sleep 1
# Confirm it is gone.
if pgrep -f "node server.js" >/dev/null 2>&1; then
  echo "stop-browser: server still running" >&2
  exit 1
fi
echo "stop-browser: stopped"
