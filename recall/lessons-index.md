admin-login-silent-fallback — on login failure always show the real error; never fall through to using a password as a token
pkill-f-kills-ssh — never pkill -f inside an SSH command; kill by port PID instead
cloudflare-403-egress-blocked — a 403 on every direct request means the VM's egress is blocked, not the key; use the ladder
tunnel-hostname-must-be-proxied — a tunnel hostname MUST be proxied (orange cloud); DNS-only gives ERR_CONNECTION_RESET
tmux-closes-when-process-exits — a tmux session closes when its main process exits; recreate it, never restart cloudflared from inside its own session
editing-supervisor-doesnt-change-running-loop — editing a supervisor script does NOT change the running loop; kill and recreate the tmux session
never-pkill-zs-gateway — never pkill -f "zs-gateway"; kill by PID
zerostack-no-env-expansion-mcp — Zerostack does NOT expand ${ENV} in MCP headers; live config holds the literal vault token
exa-mcp-top-level-toml — enable-exa-mcp MUST be top-level TOML; nested inside [mcp_servers.*] it is ignored
verify-repoint-with-browser-open — TinyFish fetch caches; verify tunnel repoints with browser.open, not web.py fetch
zerostack-acp-no-mcp — ACP mode does NOT load MCP servers; gateway drives the browser via browser.py CLI; verify with a non-famous page
camoufox-ws-ipv6-only — Camoufox ws binds IPv6 [::1]:9223 only; never normalise to 127.0.0.1
playwright-mcp-pin-0078 — use @playwright/mcp@0.0.78; 0.0.86 is rejected (Playwright 1.62.0, 428)
browser-control-setsid-nohup — start the control server with setsid nohup ... & disown; plain nohup dies when SSH closes
hoody-get-computer-not-proof — a "GET A COMPUTER" button is NOT proof of a slot; confirm a server is actually claimable
gateway-single-threaded — the gateway is single-threaded; any uncaught exception in a handler kills the server; wrap and return an error
