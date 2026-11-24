#!/bin/bash
# Zerostack + Rust gateway + all Rust tools — ONE-COMMAND installer.
#
# Installs the complete live Zerostack system on a FRESH Altare sandbox
# (Ubuntu 24.04) as root. One file, one command, no manual file editing.
# Everything mechanical is automated; the parts that need YOUR secrets
# are printed as a checklist at the end (they can never come from GitHub).
#
# Usage (on the sandbox, as root):
#   curl -fsSL https://raw.githubusercontent.com/0xSatwik/zerostack-custom/main/install.sh -o install.sh
#   bash install.sh
#
# Or, if you already cloned the repo:
#   bash install.sh
#
# What it installs (all from the zerostack-custom repo — THE canonical repo):
#   1. System packages, Node 24, Rust 1.100.0, mold (linker), cloudflared, tmux.
#   2. Zerostack 1.8.6 built from source WITH --features advisor,hooks,loop,acp
#      (default features DROP --acp and the binary exits immediately) + the
#      subagent ACP-init patch (zerostack/subagent-acp-init.patch).
#   3. The Rust gateway (zs-gateway) + chat/admin/settings pages.
#   4. ALL Rust tools, built from source and installed to their live paths:
#      zs (delegation client), search (Rust search/fetch), tailnet-relay,
#      watcher (inbox), recall (CLI), auto-digest (recall auto-updater).
#   5. Zerostack config from zerostack/config.toml.example (placeholders).
#   6. features.json (toggles, all off except builtin_memory) + search.json
#      (worker URLs — empty until you set them in /admin).
#   7. Skills/prompts (web-research, capabilities, goldfish, drift-report).
#   8. Recall system (index + digests + lessons + auto-digest, guarded).
#   9. tmux supervisor sessions: zs-gateway, tailnet-relay, pi-bridge
#      (watcher), recall-auto.
#  10. Prints the manual checklist (keys, tunnel, vault, browser, Tailscale).
#
# What it does NOT do (needs your accounts — run the checklist after):
#   - Cloudflare Tunnel + public domain (needs your Cloudflare account).
#   - Vault Worker + D1 (needs your Cloudflare account; code is public).
#   - Instabox browser machine (needs your Instabox account).
#   - Tailscale auth (needs your Tailscale account / auth key).
#   - Provider API keys, gateway token, vault master secret (yours only).
#
# Idempotent: safe to re-run. It will not overwrite your keys, config
# secrets, gateway token, or existing sessions; it only (re)installs code
# and rebuilds binaries. A second run refreshes the repo clone and rebuilds.
set -euo pipefail

REPO="0xSatwik/zerostack-custom"
REPO_RAW="https://raw.githubusercontent.com/$REPO/main"
REPO_GIT="https://github.com/$REPO.git"
ZS_VERSION="1.8.6"
BASE="/root/pi-bridge"
WORK="/root/work"
ZS_CONFIG="/root/.config/zerostack/config.toml"
REPO_DIR="$WORK/zerostack-custom"

say() { echo "==> $*"; }
warn() { echo "!! $*" >&2; }

# --- 0. sanity ---
if [ "$(id -u)" != "0" ]; then echo "run as root"; exit 1; fi
if ! grep -qi ubuntu /etc/os-release; then warn "not Ubuntu; continuing anyway"; fi
# Full PATH from the start: cargo lives in $HOME/.cargo/bin and the
# Zerostack build fails with a bare linker error without it.
export PATH="$HOME/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

# --- 1. system packages (incl. mold — the Zerostack linker) ---
say "system packages"
apt-get update -qq
apt-get install -y -qq curl git python3 python3-pip tmux build-essential pkg-config libssl-dev ca-certificates unzip mold > /dev/null 2>&1 || true
which mold >/dev/null 2>&1 && say "mold $(mold --version 2>/dev/null | head -1)" || warn "mold not installed — Zerostack build may fail (linker)"

# --- 2. Node 24 (needed by some tools/skills) ---
if ! command -v node >/dev/null 2>&1 || [ "$(node --version 2>/dev/null | cut -d. -f1 | tr -d v)" != "24" ]; then
  say "Node 24"
  curl -fsSL https://nodejs.org/dist/v24.22.3/node-v24.22.3-linux-x64.tar.xz -o /tmp/node.tar.xz
  tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1
  rm -f /tmp/node.tar.xz
fi
node --version; npm --version

# --- 3. Rust 1.100.0 (for the gateway + all tools) ---
if ! command -v cargo >/dev/null 2>&1; then
  say "Rust 1.100.0 via rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
  sh /tmp/rustup.sh -y --default-toolchain 1.100.0 > /dev/null 2>&1
  rm -f /tmp/rustup.sh
fi
cargo --version

# --- 4. cloudflared (tunnel) ---
if ! command -v cloudflared >/dev/null 2>&1; then
  say "cloudflared"
  curl -fsSL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb -o /tmp/cloudflared.deb
  dpkg -i /tmp/cloudflared.deb > /dev/null 2>&1 || apt-get install -f -y -qq > /dev/null 2>&1
  rm -f /tmp/cloudflared.deb
fi
cloudflared --version

# --- 5. fetch the setup repo (zerostack-custom — THE canonical repo) ---
say "setup repo ($REPO)"
mkdir -p "$BASE" "$WORK"
if [ -d "$REPO_DIR" ]; then
  (cd "$REPO_DIR" && git pull -q 2>/dev/null || true)
else
  git clone -q "$REPO_GIT" "$REPO_DIR" 2>/dev/null || warn "clone failed — continuing with what is on disk"
fi

# --- 6. Zerostack binary (build from source — no release binary is published) ---
# CRITICAL: build with --features advisor,hooks,loop,acp. A default-features
# build DROPS --acp (gateway sessions fail: "zerostack exited immediately" /
# "unexpected argument '--acp'"), --advisor, --hooks-test, and --loop.
# Full notes: zerostack/BUILD-NOTES.md in the repo.
# The source sets linker = "cc" + mold in .cargo/config.toml — mold (step 1)
# must be installed or the build fails with "collect2: cannot find 'ld'".
if ! command -v zerostack >/dev/null 2>&1 || [ ! -f /root/.local/bin/zerostack ]; then
  say "Zerostack $ZS_VERSION (build from source, ~9 min)"
  mkdir -p /root/.local/bin
  if [ ! -d "$WORK/zerostack-src" ]; then
    git clone -q --depth 1 --branch "v$ZS_VERSION" https://github.com/gi-dellav/zerostack.git "$WORK/zerostack-src" 2>/dev/null \
      || git clone -q --depth 1 https://github.com/gi-dellav/zerostack.git "$WORK/zerostack-src"
  fi
  # Subagent ACP-init patch: ACP serve() never called SubagentConfig::init,
  # so the task tool panicked in gateway sessions. Apply once (idempotent).
  if [ -f "$REPO_DIR/zerostack/subagent-acp-init.patch" ]; then
    (cd "$WORK/zerostack-src" && git apply --check "$REPO_DIR/zerostack/subagent-acp-init.patch" 2>/dev/null && git apply "$REPO_DIR/zerostack/subagent-acp-init.patch" && say "subagent ACP patch applied" || echo "subagent ACP patch already applied (or not clean — check manually)")
  fi
  (cd "$WORK/zerostack-src" && cargo build --release --features advisor,hooks,loop,acp)
  cp "$WORK/zerostack-src/target/release/zerostack" /root/.local/bin/zerostack
  chmod +x /root/.local/bin/zerostack
fi
export PATH="/root/.local/bin:$PATH"
zerostack --version
zerostack --help 2>&1 | grep -o -- '--acp\|--advisor\|--hooks-test\|--loop' | sort -u | tr '\n' ' '; echo "(flags present above = correct build)"

# --- 7. build ALL Rust tools from the repo (one crate each) ---
# Each crate is std-only + serde_json (no TLS crate — HTTPS goes via curl).
say "build Rust tools"
build_crate() {
  # $1 = repo dir (bridge/<name>), $2 = binary name, $3 = install path
  local crate="$1" bin="$2" dest="$3"
  if [ -d "$REPO_DIR/bridge/$crate" ]; then
    (cd "$REPO_DIR/bridge/$crate" && cargo build --release -q)
    cp "$REPO_DIR/bridge/$crate/target/release/$bin" "$dest"
    chmod +x "$dest"
    say "  $bin -> $dest"
  else
    warn "crate bridge/$crate not found — skipping $bin"
  fi
}
build_crate zs-gateway-rs zs-gateway "$BASE/zs-gateway"
build_crate zs-rs zs "$BASE/zs"
build_crate search-rs search "$BASE/search"
build_crate tailnet-relay-rs tailnet-relay "$BASE/tailnet-relay"
build_crate watcher-rs watcher "$BASE/watcher"
mkdir -p "$WORK/recall"
build_crate recall-rs recall "$WORK/recall/recall"
build_crate auto-digest-rs auto-digest "$WORK/recall/auto-digest"

# --- 8. gateway pages + supervisor script ---
for f in zs.html zs-gateway.py zs-supervise.sh; do
  if [ -f "$REPO_DIR/bridge/$f" ]; then cp "$REPO_DIR/bridge/$f" "$BASE/$f"; fi
done
chmod +x "$BASE/zs-supervise.sh" 2>/dev/null || true
# The Python search.py + zs.py stay on disk as fallbacks/references;
# the live tools are the Rust binaries (step 7). Copy them if present.
[ -f "$REPO_DIR/search/web.py" ] && cp "$REPO_DIR/search/web.py" "$BASE/search.py" 2>/dev/null || true
[ -f "$REPO_DIR/bridge/zs.py" ] && cp "$REPO_DIR/bridge/zs.py" "$BASE/zs.py" 2>/dev/null || true

# --- 9. gateway env + token (generate once, never overwrite) ---
if [ ! -f "$BASE/gateway.env" ]; then
  say "gateway token (generated once)"
  python3 -c "import secrets; print('PI_GATEWAY_TOKEN=' + secrets.token_hex(21))" > "$BASE/gateway.env"
  chmod 600 "$BASE/gateway.env"
fi
# The /admin opening password is NOT set here — you choose it in /admin.

# --- 10. Zerostack config (from the repo template — placeholders only) ---
mkdir -p "$(dirname "$ZS_CONFIG")"
if [ ! -f "$ZS_CONFIG" ]; then
  say "Zerostack config (from template)"
  if [ -f "$REPO_DIR/zerostack/config.toml.example" ]; then
    cp "$REPO_DIR/zerostack/config.toml.example" "$ZS_CONFIG"
  else
    warn "config.toml.example not in repo — writing a minimal config"
    cat > "$ZS_CONFIG" <<'EOF'
# Zerostack config. Fill in the placeholders, then restart the gateway.
# Vault token goes in literally (Zerostack does not expand ${ENV} in MCP headers).
# enable-exa-mcp MUST be top-level TOML, and false (search is Monid + Bing + X).
enable-exa-mcp = false
[mcp_servers.vault]
url = "https://YOUR-WORKER.workers.dev/mcp"
headers = { Authorization = "Bearer YOUR-VAULT-AGENT-TOKEN" }
connect_timeout_secs = 30
tool_timeout_secs = 30
EOF
  fi
  chmod 600 "$ZS_CONFIG"
fi
# Hooks settings (a single safe SessionStart hook that only logs; can never
# block a session). Created if missing.
if [ ! -f /root/.config/zerostack/settings.json ]; then
  mkdir -p /root/.config/zerostack/hooks
  cat > /root/.config/zerostack/hooks/session-start.sh <<'EOF'
#!/bin/bash
# SessionStart hook: log only, exit 0, output nothing — can never block a session.
echo "$(date -u +%FT%TZ) session started" >> /root/.config/zerostack/hooks.log
exit 0
EOF
  chmod +x /root/.config/zerostack/hooks/session-start.sh
  cat > /root/.config/zerostack/settings.json <<'EOF'
{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "/root/.config/zerostack/hooks/session-start.sh" } ] }
    ]
  }
}
EOF
  chmod 600 /root/.config/zerostack/settings.json
fi

# --- 11. features.json + search.json (defaults; user fills values in /admin) ---
if [ ! -f "$BASE/features.json" ]; then
  say "features.json (defaults)"
  cat > "$BASE/features.json" <<'EOF'
{
  "advisor": false,
  "builtin_memory": true,
  "chaining": false,
  "hooks": false,
  "prompt_to_model": false,
  "status_signals": false
}
EOF
  chmod 600 "$BASE/features.json"
fi
if [ ! -f "$BASE/search.json" ]; then
  say "search.json (empty — set worker URLs in /admin)"
  cat > "$BASE/search.json" <<'EOF'
{
  "tinyfish": true,
  "bing": true,
  "x": true,
  "cf_search_url": "",
  "fxembed_url": "",
  "fxembed_url_2": ""
}
EOF
  chmod 600 "$BASE/search.json"
fi

# --- 12. skills + prompts (built-in + installed) ---
say "skills/prompts"
PROMPTS="/root/.local/share/zerostack/prompts"
mkdir -p "$PROMPTS"
for d in "$REPO_DIR/skills" "$REPO_DIR/prompts"; do
  [ -d "$d" ] && cp "$d"/*.md "$PROMPTS/" 2>/dev/null || true
done
# The web-research skill must be the box copy (it calls /root/pi-bridge/search).
[ -f "$REPO_DIR/skills/web-research.md" ] && cp "$REPO_DIR/skills/web-research.md" "$PROMPTS/web-research.md"

# --- 13. recall system (index + digests + lessons + guarded auto-updater) ---
say "recall system"
mkdir -p "$WORK/recall/digests" "$WORK/recall/lessons"
touch "$WORK/recall/index.jsonl"
[ -f "$WORK/recall/digests/.cursor.json" ] || echo '{}' > "$WORK/recall/digests/.cursor.json"
[ -f "$WORK/recall/digests/.deleted.json" ] || echo '{}' > "$WORK/recall/digests/.deleted.json"
# Box MEMORY.md for Zerostack (fresh template; the agent fills it in).
if [ ! -f /root/.local/share/zerostack/agent/memory/MEMORY.md ]; then
  mkdir -p /root/.local/share/zerostack/agent/memory
  cat > /root/.local/share/zerostack/agent/memory/MEMORY.md <<'EOF'
# Zerostack MEMORY.md
<!-- The agent's own memory. Fill in as it learns: the system, the configs,
the rules, what is live and what is not. Keep it current. -->
## System
- Zerostack is the ONLY agent. Gateway (Rust) on 127.0.0.1:8766, token in /root/pi-bridge/gateway.env.
- Search: /root/pi-bridge/search (Rust binary, same CLI as search.py); Python search.py is the fallback.
- MCP (vault + browser) works in the TUI only — ACP/gateway mode never loads MCP servers.
- GitHub pushes run from the Muse VM via gh.py, never from this box.
- Never push worker URLs, keys, tokens, or secrets to GitHub.
EOF
  chmod 600 /root/.local/share/zerostack/agent/memory/MEMORY.md
fi

# --- 14. supervisor + background services (tmux) ---
say "start services (tmux)"
# Gateway: supervisor respawns the Rust binary (~2s) if it exits.
if [ -f "$BASE/zs-supervise.sh" ]; then
  tmux kill-session -t zs-gateway 2>/dev/null || true
  tmux new-session -d -s zs-gateway "$BASE/zs-supervise.sh"
fi
# Tailnet relay (Altare -> Instabox over SOCKS5). Needs tailscaled running
# with --tun=userspace-networking + -socks5-server=localhost:1055 first
# (see the checklist — Tailscale auth is manual). Start it anyway; it
# reconnects once the SOCKS5 proxy is up.
if [ -f "$BASE/tailnet-relay" ]; then
  tmux kill-session -t tailnet-relay 2>/dev/null || true
  tmux new-session -d -s tailnet-relay "$BASE/tailnet-relay"
fi
# Watcher (inbox -> Zerostack, every 5s). Calls the Rust zs binary.
if [ -f "$BASE/watcher" ]; then
  tmux kill-session -t pi-bridge 2>/dev/null || true
  tmux new-session -d -s pi-bridge "$BASE/watcher"
fi
# Recall auto-updater (Rust): once at start, then every 6h. Guarded — a
# digest is deleted ONLY when its session id is in .deleted.json.
if [ -f "$WORK/recall/auto-digest" ]; then
  tmux kill-session -t recall-auto 2>/dev/null || true
  tmux new-session -d -s recall-auto "while true; do $WORK/recall/auto-digest >> $WORK/recall/auto_digest.log 2>&1; sleep 21600; done"
fi

# --- 15. cleanup (so the install itself leaves no junk) ---
say "cleanup install leftovers"
rm -f /tmp/*.tar.gz /tmp/*.tgz /tmp/*.deb /tmp/node.tar.xz 2>/dev/null || true
if command -v npm >/dev/null 2>&1; then npm cache clean --force >/dev/null 2>&1 || true; fi

say "gateway health"
sleep 2
curl -s -m 5 http://127.0.0.1:8766/health || warn "gateway not responding yet — check: tmux attach -t zs-gateway"

cat <<'EOF'

================ MANUAL CHECKLIST (needs YOUR accounts) ================
The automatic part is done. These need your secrets — they can never
come from GitHub. Do them once, in this order:

1. PROVIDER KEYS — add your model API keys.
   Open /admin (log in with the gateway token in /root/pi-bridge/gateway.env),
   go to Models -> Add provider, and paste each key. Or edit
   /root/pi-bridge/provider-keys.env (chmod 600) and add:
     HMS_API_KEY, HOWTOFIX_API_KEY, XINGCHI_API_KEY
   (MOSS_API_KEY + TOKENLAB_API_KEY can also go in /root/.profile.)
   Then Load models for each provider and make one the default.
   Restart the gateway after any key change.

2. TAILSCALE — join the tailnet (Altare has no /dev/net/tun).
   Install tailscale, then run it userspace-networking with a SOCKS5 proxy:
     tailscaled --tun=userspace-networking --socket=/root/tailscale/tailscaled.sock -socks5-server=localhost:1055
     (tmux session `tailscaled`; the CLI needs the socket symlink:
      ln -sf /root/tailscale/tailscaled.sock /var/run/tailscale/tailscaled.sock)
   Then `tailscale up` with your auth key. The tailnet relay (step 14)
   forwards 127.0.0.1:8931/6081/6080 to the Instabox over that SOCKS5 proxy.
   Ordinary processes on this box CANNOT dial Tailscale IPs directly.

3. VAULT — deploy your own Cloudflare Worker + D1 vault.
   Code: https://github.com/0xSatwik/cf-d1-vault (public, full guide in README).
   Deploy it, note the Worker URL + master secret (keep the master in your
   Secure Vault, never in chat), then put the Worker URL + a request-only
   agent token in the Zerostack config [mcp_servers.vault] (literal token —
   Zerostack does not expand ${ENV} in MCP headers).

4. TUNNEL + PUBLIC URL — Cloudflare Tunnel + a domain.
   Create a named tunnel in your Cloudflare account, point its ingress to
   127.0.0.1:8766 (and 127.0.0.1:6081 for /browser and /browser-vnc).
   The hostname MUST be proxied (orange cloud) or the tunnel will not route.
   cloudflared forwards the FULL path — backends must serve the prefixed path.
   Never restart cloudflared inside its tmux session; kill the session and
   recreate it (tmux kill-session -t pi-tunnel, then new-session).

5. OPENING PASSWORD — choose it in /admin.
   Open /admin, log in with the gateway token, then use the "Set password"
   card. Never let anyone invent it for you. The password (SHA-256) lives in
   gateway.env as PI_ADMIN_PASS_SHA256.

6. SEARCH WORKER URLS — set them in /admin.
   /admin -> Keys & secrets -> Web search — worker URLs: your cf-search
   Worker URL (Bing) and your fixtweet Worker URL(s) (X). They are stored in
   /root/pi-bridge/search.json (chmod 600) — never hardcoded, never in GitHub.
   Monid key: /admin -> Monid key (writes ~/.config/monid/credentials.yaml).

7. BROWSER (optional) — Instabox machine + Camoufox.
   Create an Instabox machine (4 vCPU / 8 GiB), install the camofox-browser
   server (port 9377) + noVNC viewer (6080) + the Rust browser-control
   (6081, from bridge/browser-control-rs/). The /browser page is the
   password-gated control page; /browser-vnc is gated through it too.
   The Instabox auto-pauses after 30 min idle (or 8h max runtime) — resume
   it before use, and restart its tailscaled if it shows offline after resume.

8. VERIFY — open the public URL /pi, log in, send "Say OK".
   Then /admin, /pi/settings, /browser, and the recall index (grep index.jsonl).
   Quick checks on the box:
     curl -s http://127.0.0.1:8766/health
     /root/pi-bridge/zs list
     /root/pi-bridge/search search "Rust programming language"
=========================================================================
EOF
