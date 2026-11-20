# tmux-closes-when-process-exits
- trigger: a tmux session closed when its main process exited; the tunnel/gateway disappeared and the URL 502'd
- rule: a tmux session closes when its main process exits — recreate it with tmux new-session -d -s <name> '<command>'; never restart cloudflared from inside its own session
- applies: tmux, cloudflared, gateways, any long-running process
