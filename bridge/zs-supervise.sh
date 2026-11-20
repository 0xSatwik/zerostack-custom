#!/bin/sh
# zs-supervise.sh — keeps zs-gateway.py running inside the tmux session.
#
# Why: /restart used to try to recreate its own tmux session from inside that
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
  python3 /root/pi-bridge/zs-gateway.py >>"$LOG" 2>&1
  rc=$?
  echo "$(date '+%Y-%m-%d %H:%M:%S') zs-gateway exited rc=$rc — respawning in 2s" >>"$LOG"
  sleep 2
done
