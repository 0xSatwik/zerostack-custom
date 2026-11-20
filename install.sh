#!/bin/bash
# Zerostack + Rust gateway + recall — one-command installer.
#
# Installs the live Zerostack setup on a FRESH Altare sandbox (Ubuntu 24.04)
# as root. Automates everything that can be automated; the parts that need
# YOUR secrets are printed as a checklist at the end (they can never come
# from GitHub).
#
# Usage (on the sandbox, as root):
#   curl -fsSL https://raw.githubusercontent.com/0xSatwik/muse-pi-setup/main/install.sh -o install.sh
#   bash install.sh
#
# Or, if you already cloned the repo:
#   bash install.sh
#
# What it does:
#   1. System packages, Node 24, Rust, cloudflared, tmux, python3.
#   2. Zerostack binary (official 1.8.6) + custom prompts (Exa removed) + skills.
#   3. Rust gateway (zs-gateway) built from source + chat/admin/settings pages.
#   4. Config with placeholders (you fill in keys after).
#   5. Recall system (index + digests + lessons + auto-updater).
#   6. tmux supervisor sessions for the gateway + recall watcher.
#   7. Prints the manual checklist (keys, tunnel, vault, browser).
#
# What it does NOT do (needs your accounts — run the checklist after):
#   - Cloudflare Tunnel + public domain (needs your Cloudflare account).
#   - Vault Worker + D1 (needs your Cloudflare account; code is public).
#   - Instabox browser machine (needs your Instabox account).
#   - Provider API keys, gateway token, vault master secret (yours only).
#
# Idempotent: safe to re-run. It will not overwrite your keys or config
# secrets; it only (re)installs code and rebuilds the gateway.
set -euo pipefail

REPO_RAW="https://raw.githubusercontent.com/0xSatwik/muse-pi-setup/main"
ZS_VERSION="1.8.6"
BASE="/root/pi-bridge"
WORK="/root/work"
ZS_CONFIG="/root/.config/zerostack/config.toml"

say() { echo "==> $*"; }
warn() { echo "!! $*" >&2; }

# --- 0. sanity ---
if [ "$(id -u)" != "0" ]; then echo "run as root"; exit 1; fi
if ! grep -qi ubuntu /etc/os-release; then warn "not Ubuntu; continuing anyway"; fi

# --- 1. system packages ---
say "system packages"
apt-get update -qq
apt-get install -y -qq curl git python3 python3-pip tmux build-essential pkg-config libssl-dev ca-certificates unzip > /dev/null 2>&1 || true

# --- 2. Node 24 (needed by some tools/skills) ---
if ! command -v node >/dev/null 2>&1 || [ "$(node --version 2>/dev/null | cut -d. -f1 | tr -d v)" != "24" ]; then
  say "Node 24"
  curl -fsSL https://nodejs.org/dist/v24.22.3/node-v24.22.3-linux-x64.tar.xz -o /tmp/node.tar.xz
  tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1
  rm -f /tmp/node.tar.xz
fi
node --version; npm --version

# --- 3. Rust (for the gateway) ---
if ! command -v cargo >/dev/null 2>&1; then
  say "Rust via rustup"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup.sh
  sh /tmp/rustup.sh -y --default-toolchain 1.100.0 > /dev/null 2>&1
  rm -f /tmp/rustup.sh
fi
export PATH="$HOME/.cargo/bin:$PATH"
cargo --version

# --- 4. cloudflared (tunnel) ---
if ! command -v cloudflared >/dev/null 2>&1; then
  say "cloudflared"
  curl -fsSL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb -o /tmp/cloudflared.deb
  dpkg -i /tmp/cloudflared.deb > /dev/null 2>&1 || apt-get install -f -y -qq > /dev/null 2>&1
  rm -f /tmp/cloudflared.deb
fi
cloudflared --version

# --- 5. Zerostack binary (build from source — no release binary is published) ---
# The live binary was built this way. Building takes a few minutes but is
# the reliable path. The customizations (Exa removed) come from the prompts
# + config installed below, not from a patched binary — stock Zerostack +
# custom prompts/config is equivalent.
if ! command -v zerostack >/dev/null 2>&1; then
  say "Zerostack $ZS_VERSION (build from source)"
  mkdir -p /root/.local/bin
  if [ ! -d "$WORK/zerostack-src" ]; then
    git clone -q --depth 1 --branch "v$ZS_VERSION" https://github.com/gi-dellav/zerostack.git "$WORK/zerostack-src" 2>/dev/null \
      || git clone -q --depth 1 https://github.com/gi-dellav/zerostack.git "$WORK/zerostack-src"
  fi
  (cd "$WORK/zerostack-src" && cargo build --release -q)
  cp "$WORK/zerostack-src/target/release/zerostack" /root/.local/bin/zerostack
  chmod +x /root/.local/bin/zerostack
fi
export PATH="/root/.local/bin:$PATH"
zerostack --version

# --- 6. fetch the setup files from the repo ---
say "setup files"
mkdir -p "$BASE" "$WORK"
cd "$WORK"
for f in zs-gateway-rs zs.html config.toml.example; do :; done
# The gateway crate + pages live in the repo under bridge/.
# Clone (or refresh) the public setup repo for its files.
if [ -d "$WORK/muse-pi-setup" ]; then
  (cd "$WORK/muse-pi-setup" && git pull -q 2>/dev/null || true)
else
  git clone -q https://github.com/0xSatwik/muse-pi-setup.git "$WORK/muse-pi-setup" 2>/dev/null || true
fi
# Copy the gateway crate + pages. Prefer the local clone; fall back to raw fetch.
if [ -d "$WORK/muse-pi-setup/bridge/zs-gateway-rs" ]; then
  cp -r "$WORK/muse-pi-setup/bridge/zs-gateway-rs" "$WORK/zs-gateway-rs"
else
  warn "bridge/zs-gateway-rs not in the clone; fetching single files is not supported for a crate — clone the repo manually"
fi
for f in zs.html zs-gateway.py zs-supervise.sh; do
  if [ -f "$WORK/muse-pi-setup/bridge/$f" ]; then cp "$WORK/muse-pi-setup/bridge/$f" "$BASE/$f"; fi
done

# --- 7. build the Rust gateway ---
if [ -d "$WORK/zs-gateway-rs" ]; then
  say "build Rust gateway"
  (cd "$WORK/zs-gateway-rs" && cargo build --release -q)
  cp "$WORK/zs-gateway-rs/target/release/zs-gateway" "$BASE/zs-gateway"
  chmod +x "$BASE/zs-gateway"
  "$BASE/zs-gateway" --version 2>/dev/null || true
fi

# --- 8. gateway env + token (generate once, never overwrite) ---
if [ ! -f "$BASE/gateway.env" ]; then
  say "gateway token (generated once)"
  python3 -c "import secrets; print('PI_GATEWAY_TOKEN=' + secrets.token_hex(21))" > "$BASE/gateway.env"
  chmod 600 "$BASE/gateway.env"
fi
# The /admin opening password is NOT set here — you choose it in /admin.

# --- 9. Zerostack config (placeholder secrets — you fill them in) ---
mkdir -p "$(dirname "$ZS_CONFIG")"
if [ ! -f "$ZS_CONFIG" ]; then
  say "Zerostack config (placeholders)"
  if [ -f "$WORK/muse-pi-setup/zerostack-config.toml" ]; then
    cp "$WORK/muse-pi-setup/zerostack-config.toml" "$ZS_CONFIG"
  else
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
[mcp_servers.browser]
url = "http://127.0.0.1:8931/mcp"
connect_timeout_secs = 30
tool_timeout_secs = 60
connect_retries = 2
EOF
  fi
  chmod 600 "$ZS_CONFIG"
fi

# --- 10. skills + prompts (built-in + installed) ---
say "skills/prompts"
PROMPTS="/root/.local/share/zerostack/prompts"
mkdir -p "$PROMPTS"
# The repo carries the installed skill prompts; copy any that exist.
for d in "$WORK/muse-pi-setup/skills" "$WORK/muse-pi-setup/prompts"; do
  [ -d "$d" ] && cp "$d"/*.md "$PROMPTS/" 2>/dev/null || true
done

# --- 11. recall system ---
say "recall system"
mkdir -p "$WORK/recall/digests" "$WORK/recall/lessons"
if [ -d "$WORK/muse-pi-setup/recall" ]; then
  cp -r "$WORK/muse-pi-setup/recall/"* "$WORK/recall/" 2>/dev/null || true
fi
touch "$WORK/recall/index.jsonl"
[ -f "$WORK/recall/digests/.cursor.json" ] || echo '{}' > "$WORK/recall/digests/.cursor.json"

# --- 12. supervisor + recall watcher (tmux) ---
say "start services (tmux)"
# Gateway: supervisor respawns the Rust binary (~2s) if it exits.
if [ -f "$BASE/zs-supervise.sh" ]; then
  tmux kill-session -t zs-gateway 2>/dev/null || true
  tmux new-session -d -s zs-gateway "$BASE/zs-supervise.sh"
fi
# Recall auto-updater: runs once at start, then every 6h.
if [ -f "$WORK/recall/auto_digest.py" ]; then
  tmux kill-session -t recall-auto 2>/dev/null || true
  tmux new-session -d -s recall-auto "while true; do python3 $WORK/recall/auto_digest.py >> $WORK/recall/auto_digest.log 2>&1; sleep 21600; done"
fi

say "gateway health"
sleep 2
curl -s -m 5 http://127.0.0.1:8766/health || warn "gateway not responding yet — check: tmux attach -t zs-gateway"

cat <<'EOF'

================ MANUAL CHECKLIST (needs YOUR accounts) ================
The automatic part is done. These need your secrets — they can never
come from GitHub. Do them once, in this order:

1. PROVIDER KEYS — add your model API keys.
   Edit /root/pi-bridge/provider-keys.env (chmod 600) and add:
     HMS_API_KEY, HOWTOFIX_API_KEY, XINGCHI_API_KEY
   (MOSS_API_KEY + TOKENLAB_API_KEY can also go in /root/.profile.)
   Then add the models in /admin -> Models -> Add provider, or edit
   /root/pi-bridge/models.json. Restart the gateway after.

2. VAULT — deploy your own Cloudflare Worker + D1 vault.
   Code: https://github.com/0xSatwik/cf-d1-vault (public, full guide in README).
   Deploy it, note the Worker URL + master secret (keep the master in your
   Secure Vault, never in chat), then put the Worker URL + a request-only
   agent token in the Zerostack config [mcp_servers.vault] above.

3. TUNNEL + PUBLIC URL — Cloudflare Tunnel + a domain.
   Create a tunnel in your Cloudflare account, point its ingress to
   127.0.0.1:8766 (and 6081/6080 for the browser, if you use it).
   The hostname MUST be proxied (orange cloud) or the tunnel will not route.
   Use http2 in the tunnel config if the host is overloaded (QUIC drops).

4. OPENING PASSWORD — choose it in /admin.
   Open /admin, log in with the gateway token (in /root/pi-bridge/gateway.env),
   then use the "Set password" card. Never let anyone invent it for you.

5. BROWSER (optional) — Instabox machine + Camoufox + Playwright MCP.
   Create an Instabox machine, install Camoufox + @playwright/mcp@0.0.78
   (pinned — 0.0.86 is rejected), expose MCP on 8931, and point the
   Zerostack [mcp_servers.browser] at it (via the tailnet relay if the
   browser is on another box). See docs/ZEROSTACK.md in the setup repo.

6. VERIFY — open the public URL /pi, log in, send "Say OK".
   Then /admin, /pi/settings, and the recall index (grep index.jsonl).
=========================================================================
EOF
