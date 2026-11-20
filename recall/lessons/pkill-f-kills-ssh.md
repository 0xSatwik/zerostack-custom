# pkill-f-kills-ssh
- trigger: pkill -f "playwright-mcp" run inside an SSH command killed the SSH session itself, because the remote command line contains the pattern
- rule: never pkill -f inside an SSH command; kill by port PID instead (ss -tlnp -> pid -> kill)
- applies: instabox, altare, any remote shell
