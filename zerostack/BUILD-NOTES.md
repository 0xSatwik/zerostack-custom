# Zerostack build notes (2026-11-24)

Upstream: gi-dellav/zerostack, commit 291046d, version 1.8.6.
HEAD == origin/main — already on latest upstream (Phase A is a no-op).

## Our modifications vs upstream
1. Exa removal: 9 prompt files in data/prompts/ (Exa search instruction
   replaced with our web.py research line).
2. Subagent init in ACP mode: src/extras/acp/mod.rs — see
   subagent-acp-init.patch (33 lines). Without it, any task-tool call in
   ACP/gateway mode panics with 'SubagentConfig not initialized'.
   Verified live 2026-11-24: a /pi (ACP) session used the task tool,
   subagent ran list_dir, returned a verified listing, no panic.

## Build command (must match the old binary's feature set)
The binary the gateway spawns must be built with:
  cargo build --release --features advisor,hooks,loop,acp
Default features alone are NOT enough:
- Without 'acp' the binary rejects '--acp' and exits immediately
  (gateway sessions fail with 'zerostack exited immediately').
- Without 'advisor'/'hooks'/'loop' those flags vanish (--advisor,
  --hooks-test, --loop) and item 5/7 regress.
Also requires: full cargo PATH, and mold installed (apt-get install mold)
because .cargo/config.toml sets linker cc + -fuse-ld=mold.
Build takes ~8-9 min (first) / ~4 min (feature change) / ~9 min (acp deps).
Deploy: cp target/release/zerostack /root/.local/bin/zerostack
(old binary backed up at /root/.local/bin/zerostack.bak-1.8.6-20261124).
