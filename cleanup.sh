#!/bin/bash
# cleanup.sh — remove the useless files that accumulate on the box.
#
# Frees ~5 GB on a box that has been through the full setup. Safe and
# idempotent: it only removes known-useless items (caches, installer
# leftovers, dependency folders) and KEEPS results, source code, configs
# and secrets. Run it any time disk feels high.
#
# Usage (on the box, as root):
#   bash cleanup.sh
#
# What it removes (and what it keeps):
#   - /root/.pi-lens            Pi's static-analysis tools (opengrep, rust-analyzer,
#                               zizmor, ruff). Pi is removed, nothing uses these.
#   - /root/.cache/uv, /root/.cache/pip
#                               Package-manager caches (pyarrow etc. from the model
#                               eval). Re-downloaded automatically if ever needed.
#   - /tmp installer leftovers  *.tar.gz, *.tgz, *.deb, hallmark-download, ts-src,
#                               ts-deb, pi-lens-ast-grep, jiti, node-compile-cache.
#   - npm cache                 `npm cache clean --force` (if npm exists).
#   - /root/work/model-eval/venv + node_modules
#                               The model-eval dependencies. KEEPS the results
#                               (PROGRESS.md, SUMMARY.md, logs, arena configs).
#   - /root/work/cf-vault/node_modules
#                               The vault dev dependencies. KEEPS the source
#                               (src, public, tests, scripts, wrangler.toml).
#   - /root/work/zs-gateway-rs.bak-pre-fix/target
#                               Build artifacts of the backup crate. KEEPS the source.
#
# What it NEVER touches:
#   - /root/.rustup (Rust toolchain) — needed to rebuild the gateway.
#   - /root/work/zs-gateway-rs (the live gateway source + target).
#   - /root/.config/zerostack, /root/pi-bridge (configs, keys, token).
#   - /root/work/recall, /root/work/zerostack-src, /root/work/taste-skill.
#   - /root/.local/bin/zerostack, /opt/node24, /root/ts (Tailscale).
#   - Any session files, transcripts, or the vault.
set -euo pipefail

say() { echo "==> $*"; }
freed_before=$(df --output=used -k / 2>/dev/null | tail -1 || echo 0)

# --- Pi leftover tools (Pi is removed; nothing references these) ---
if [ -d /root/.pi-lens ]; then
  say "remove /root/.pi-lens (Pi analysis tools, $(du -sh /root/.pi-lens 2>/dev/null | cut -f1))"
  rm -rf /root/.pi-lens
fi

# --- package-manager caches (re-downloaded if ever needed) ---
for d in /root/.cache/uv /root/.cache/pip; do
  if [ -d "$d" ]; then
    say "remove $d (cache, $(du -sh "$d" 2>/dev/null | cut -f1))"
    rm -rf "$d"
  fi
done
if command -v npm >/dev/null 2>&1; then
  say "npm cache clean"
  npm cache clean --force >/dev/null 2>&1 || true
fi

# --- /tmp installer leftovers ---
say "remove /tmp installer leftovers"
rm -f /tmp/*.tar.gz /tmp/*.tgz /tmp/*.deb 2>/dev/null || true
rm -rf /tmp/hallmark-download /tmp/ts-src /tmp/ts-deb /tmp/pi-lens-ast-grep \
       /tmp/jiti /tmp/node-compile-cache /tmp/node.tar.xz /tmp/rustup.sh 2>/dev/null || true

# --- model-eval dependencies (keep the results) ---
if [ -d /root/work/model-eval/venv ] || [ -d /root/work/model-eval/node_modules ]; then
  say "remove model-eval venv + node_modules (keep results)"
  rm -rf /root/work/model-eval/venv /root/work/model-eval/node_modules
fi

# --- vault dev dependencies (keep the source) ---
if [ -d /root/work/cf-vault/node_modules ]; then
  say "remove cf-vault node_modules (keep source)"
  rm -rf /root/work/cf-vault/node_modules
fi

# --- backup crate build artifacts (keep the source) ---
if [ -d /root/work/zs-gateway-rs.bak-pre-fix/target ]; then
  say "remove zs-gateway-rs.bak-pre-fix/target (keep source)"
  rm -rf /root/work/zs-gateway-rs.bak-pre-fix/target
fi

freed_after=$(df --output=used -k / 2>/dev/null | tail -1 || echo 0)
say "done. /root is now $(du -sh /root 2>/dev/null | cut -f1)."
say "gateway check:"
curl -s -m 5 http://127.0.0.1:8766/health 2>/dev/null || echo "(gateway not responding — check tmux zs-gateway)"
