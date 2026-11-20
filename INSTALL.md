# INSTALL — set up the Zerostack system on a fresh account

This guide is for another Muse (or a human) setting up this whole system
from scratch on a fresh account. It is the same setup that is live at
`https://hardeepwalia.pitunnel.dpdns.org/pi` — Zerostack + Rust gateway +
chat/admin/settings pages + recall + vault + Instabox browser.

Read `AGENTS.md` in this repo first. It is the operating manual and holds
the lessons that will save you hours (Cloudflare 403 = egress blocked,
tunnel hostname must be proxied, Playwright MCP pin, `pkill -f` kills
your own SSH session, and more).

## What you need before you start

| Thing | Where | Notes |
|---|---|---|
| Altare account | altare.gg | Free pool: 6 GiB / 150% CPU / 10 GiB disk. Create a sandbox (Ubuntu 24.04). |
| Cloudflare account | dash.cloudflare.com | For the Tunnel + the vault Worker + D1. Free tier is enough. |
| A domain (or subdomain) | any registrar | Must be a zone in the SAME Cloudflare account as the tunnel. A `dpdns.org` subdomain works (free, in the PSL). |
| Provider API keys | your model providers | At least one OpenAI-compatible provider (e.g. 100milionseek, tokenlab, moss, howtofix, xingchi). The keys go in the box only, never in chat or in GitHub. |
| Instabox account (optional) | instabox.sh | Only if you want the browser. Free VM: 4 vCPU / 8 GiB / 20 GiB. |
| Tailscale account (optional) | login.tailscale.com | Only if the browser runs on a different box than the agent. |

## Step 0 — the sandbox

1. Create an Altare sandbox (Ubuntu 24.04). Note its SSH endpoint
   (host + port) and its server id.
2. SSH in as root. Confirm you are root (`id -u` → `0`).
3. Enable Direct network protocols in the Muse app if you are Muse
   (Outbound SSH, etc.) — otherwise SSH from the Muse VM is blocked.

## Step 1 — run the installer

On the sandbox, as root:

```
curl -fsSL https://raw.githubusercontent.com/0xSatwik/muse-pi-setup/main/install.sh -o install.sh
bash install.sh
```

What it does (all automatic): system packages, Node 24, Rust 1.100.0,
cloudflared, the Zerostack 1.8.6 binary, the Rust gateway built from
source, the chat (`/pi`), admin (`/admin`) and settings (`/pi/settings`)
pages, the Zerostack config with placeholders, the recall system, and
the tmux supervisor sessions (`zs-gateway`, `recall-auto`). It prints
a manual checklist at the end.

Verify: `curl -s http://127.0.0.1:8766/health` should return
`{"backend":"zerostack","impl":"rust","ok":true,...}`.

If the gateway is not responding: `tmux attach -t zs-gateway` to see
its log. The supervisor respawns it ~2s after an exit. Editing
`zs-supervise.sh` on disk does NOT change the running loop — kill and
recreate the tmux session.

## Step 2 — provider keys + models

1. Add your provider API keys to `/root/pi-bridge/provider-keys.env`
   (chmod 600). Format: `HMS_API_KEY=...` (one per line). Also
   `MOSS_API_KEY` / `TOKENLAB_API_KEY` can go in `/root/.profile`.
   Never put a key in chat, in a browser task, or in GitHub.
2. Open `/admin` (once the tunnel is up, Step 4) and add the models:
   Add provider → Load models (uses the stored key server-side) → tick
   the models to add → set one default → reorder the rest for fallback.
   Or edit `/root/pi-bridge/models.json` directly and restart the gateway.
3. Restart the gateway: kill its PID (supervisor respawns it). Never
   `pkill -f "zs-gateway"` — it kills both the Rust and Python binaries;
   kill by PID.

## Step 3 — the vault (Cloudflare Worker + D1)

The vault lets any agent use API keys without ever seeing them. The
code is public with a full guide:

- Repo: https://github.com/0xSatwik/cf-d1-vault (README carries the
  whole connecting system, all code written in it).

Deploy it to YOUR Cloudflare account (Worker + D1), note the Worker URL
and the master secret. Keep the master secret in your Secure Vault —
never in chat, never in GitHub. Then put the Worker URL + a request-only
agent token in the Zerostack config `[mcp_servers.vault]` (literal
token — Zerostack does NOT expand `${ENV}` in MCP headers). The vault
MCP works in the Zerostack terminal (TUI) only; ACP (the /pi chat)
never loads MCP servers. To use the vault, ask Muse to unlock it for a
service, then use the Zerostack terminal.

## Step 4 — tunnel + public URL

1. In your Cloudflare account, create a named tunnel (e.g.
   `zerostack`) and a DNS record for it. The hostname MUST be proxied
   (orange cloud) — DNS-only gives `ERR_CONNECTION_RESET`.
2. The CNAME must point at `<tunnel-uuid>.cfargotunnel.com` and the
   domain must be a zone in the SAME Cloudflare account as the tunnel.
3. Tunnel config ingress: everything else → `127.0.0.1:8766`. If you
   use the browser, add `/browser*` → `127.0.0.1:6081` and
   `/browser-vnc*` → `127.0.0.1:6080`. cloudflared forwards the FULL
   path — backends must serve the prefixed path.
4. If the host is overloaded (load avg > 50, tunnel 502s), set
   `protocol: http2` in the tunnel config (QUIC drops under load).
5. Start it in tmux: `tmux new-session -d -s pi-tunnel 'cloudflared tunnel --config /root/.cloudflared/config.yml run'`.
   Never restart cloudflared inside its tmux session — a tmux session
   closes when its main process exits.
6. Verify with a fresh browser load of `https://<your-domain>/pi`
   (not a search fetch — that can serve a cached copy for minutes).

## Step 5 — opening password

Open `/admin`, log in with the gateway token (in
`/root/pi-bridge/gateway.env`, 43 chars — paste it in the password
field), then use the "Set password" card to choose an opening password.
You must choose it; never let anyone invent one. If you forget it, log
in with the gateway token and reset it from the same card.

## Step 6 — browser (optional, Instabox)

1. Create an Instabox machine (agent-browser template, Debian 13).
   Note its id. It auto-pauses after 30 min idle; resume it with the
   Instabox API when you need it.
2. SSH in (token as username, port 2222). Install Camoufox and
   Playwright MCP **pinned to `@playwright/mcp@0.0.78`** (0.0.86 is
   rejected by Camoufox's Playwright 1.62.0 with a 428 version
   mismatch). Camoufox's ws binds IPv6 `[::1]` only — never use
   `127.0.0.1` for it.
3. Start the Playwright MCP on `0.0.0.0:8931` and the viewer
   (noVNC on 6080, control page on 6081). Start long-running servers
   with `setsid nohup ... & disown` — plain nohup dies when SSH closes.
   Kill Playwright MCP by port PID, never `pkill -f` inside an SSH
   command (it kills your own session).
4. Password-protect the browser control page with the gateway token.
5. If the browser is on a different box than the agent, connect both
   to the same Tailscale tailnet and use the SOCKS5 relay pattern from
   `AGENTS.md` (Altare has no `/dev/net/tun`, so normal processes
   cannot dial Tailscale IPs — go through the relay on localhost).

## Step 7 — verify everything

- `/pi` loads, you log in, and "Say OK" returns "OK".
- `/admin` loads and shows your providers/models.
- `/pi/settings` loads and shows the Zerostack version, the skills,
  and the MCP servers (browser + vault, noted TUI-only).
- The recall index has your sessions: `grep . /root/work/recall/index.jsonl`.
- The auto-updater is running: `tmux ls` shows `recall-auto`.
- A web research query works (TinyFish + Bing + X via the combined
  search system — never paid Monid endpoints).
- The vault is locked until Muse unlocks it for a service.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `/pi` 502 | Altare host overloaded (load avg > 50) | Tunnel `protocol: http2`; wait for the host to recover; it self-heals on reconnect. |
| `ERR_CONNECTION_RESET` on the tunnel URL | DNS record is DNS-only, not proxied | Set the CNAME to proxied (orange cloud). |
| Cloudflare 403 on a site | The VM's egress is blocked, not your key | Use the ladder in AGENTS.md (search fetch → browser task → vault Worker → other egress). |
| Load models shows nothing | You clicked it from the Providers pane | The tick panel lives in the Models pane; the button now switches to it first. |
| Admin login "not happening" | Wrong password fell through to a token attempt | The login now shows the real error. Use the gateway token, then reset the password. |
| Zerostack session dies empty | Provider 502/429 (100milionseek, tokenlab) | Provider fault, not integration. Retry or switch model. |
| `cargo: command not found` in SSH | rustup bin not on PATH | `export PATH="$HOME/.cargo/bin:$PATH"`. |
| SSH "Host key verification failed" | `/root/.ssh/known_hosts` wiped on VM replacement | Use the durable copy: `-o UserKnownHostsFile=/home/hatch/.ssh/known_hosts`. |

## What lives where

| Piece | On the box | In GitHub |
|---|---|---|
| Zerostack binary | `/root/.local/bin/zerostack` | gi-dellav/zerostack (release) |
| Zerostack config | `/root/.config/zerostack/config.toml` | `zerostack-config.toml` (template, placeholders) |
| Rust gateway | `/root/pi-bridge/zs-gateway` | `bridge/zs-gateway-rs/` |
| Chat/Admin/Settings pages | served from the gateway binary | `bridge/zs.html` (+ `admin.html`, `settings.html` in the crate) |
| Recall system | `/root/work/recall/` | `recall/` |
| Install script | — | `install.sh` (top level) |
| Vault Worker + D1 | Cloudflare (your account) | 0xSatwik/cf-d1-vault (public) |
| Secrets (keys, tokens, master) | the box only, chmod 600 | NEVER in GitHub |

## Updating

Re-running `install.sh` is safe (idempotent) — it reinstalls code and
rebuilds the gateway but does not overwrite your keys or config
secrets. After any system update, sync the repos (the setup repo and
the private custom repo) so the next install gets the latest code.
