#!/bin/sh
# zs-supervise.sh — keeps the Zerostack gateway running inside the tmux session.
#
# The gateway is Rust now (bridge/zs-gateway-rs, built to /root/pi-bridge/zs-gateway,
# 9 MB vs the Python gateway's 19 MB). If the Rust binary is not present (a fresh
# setup that has not built it yet), fall back to the Python gateway zs-gateway.py.
#
# Why a loop: /restart used to try to recreate its own tmux session from inside that
# session, which killed the very subprocess it had just spawned and left the
# old process serving. A loop is simpler and honest: the gateway exits, this
# respawns it, and the tmux session (and its log) never churn.
#
# Run (tmux session zs-gateway):
#   /root/pi-bridge/zs-supervise.sh
export PATH=/root/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
LOG=/root/pi-bridge/zs-gateway.log
cd /root/pi-bridge || exit 1
while :; do
  if [ -x /root/pi-bridge/zs-gateway ]; then
    /root/pi-bridge/zs-gateway >>"$LOG" 2>&1
  else
    python3 /root/pi-bridge/zs-gateway.py >>"$LOG" 2>&1
  fi
  rc=$?
  echo "$(date '+%Y-%m-%d %H:%M:%S') zs-gateway exited rc=$rc — respawning in 2s" >>"$LOG"
  sleep 2
done
