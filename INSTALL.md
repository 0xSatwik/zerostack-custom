# Install — zerostack custom build

Private fork of [zerostack](https://github.com/gi-dellav/zerostack) (1.8.6, GPL-3.0) with our search, vault, gateway, skills and config. Additive-only: everything upstream ships stays, except the search (see FEATURES.md).

## 1. Install the binary

The release binary is the supported path (no cargo on the target box):

```sh
# from the zerostack releases (install.sh) — installs 1.8.6 to ~/.local/bin/zerostack
curl -fsSL https://github.com/gi-dellav/zerostack/releases/download/v1.8.6/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"   # add to ~/.profile and ~/.bashrc
zerostack --version                     # 1.8.6
```

Building from this source instead: follow upstream's AGENTS.md — never `cargo build`, no `--release` during development, `cargo test` instead of `cargo check`, always `cargo fmt`, install with `cargo install --path . --debug`. Note the running binary has its prompts baked in: editing `data/prompts/` only affects a rebuild.

## 2. Configure providers and models

Copy the template and fill in your own keys (never commit real keys):

```sh
mkdir -p ~/.config/zerostack
cp config/config.toml.example ~/.config/zerostack/config.toml
chmod 600 ~/.config/zerostack/config.toml
```

The template carries six OpenAI-compatible providers (100milionseek default, xingchi, tokenlab, moss, howtofix) with `max_tokens=65536` and `context_window=197488`. Each provider reads its key from an env var (`HMS_API_KEY`, `XINGCHI_API_KEY`, `TOKENLAB_API_KEY`, `MOSS_API_KEY`, `HOWTOFIX_API_KEY`). Export them from `~/.profile` (and `~/.bashrc`) — the gateway and the ask-route watcher read keys from there, not from the gateway store.

Known provider notes:
- **xingchi** returns 403 `client_not_allowed` in zerostack — the provider only admits named coding clients (Codex, Claude Code, Cursor, pi). The `api_style = "responses"` setting does not fix it.
- **100milionseek** has returned 502/503/524 intermittently; **tokenlab** is the healthy fallback. Switch a live session's model with `POST /sessions/<id>/model`.
- **tokenlab** can 429 "at capacity" (transient concurrency limit) — retry later.

## 3. Vault (API keys without ever seeing them)

The vault is a Cloudflare Worker + D1 MCP server. Zerostack connects to it as an MCP server (URL-based `mcp_servers.vault` in the config). Two rules:

- **MCP headers do NOT expand env vars** in zerostack (`expand_env` is provider-only). The live config must hold the literal vault agent token, substituted at deploy time from `$VAULT_AGENT_TOKEN`. The template and this repo keep the `${VAULT_AGENT_TOKEN}` placeholder — never a real token.
- **Vault tools do NOT work through the gateway** (MCP is disabled in ACP mode). They work in the Zerostack terminal (TUI) only. A vault tool call returns `unlock_required` until Muse unlocks the agent for that service (2-hour window, per service).

Register an agent (`zerostack-altare` is the live one), save its request-only token first, then deploy the config with the literal token in place.

## 4. Search (ours only — no Exa)

Exa is off: `enable-exa-mcp` is a **top-level** config field (put it at the top of the TOML, not inside `[mcp_servers.vault]`, or it stays on). The nine bundled prompt files in `data/prompts/` have the Exa instruction removed and replaced with one line: use `search/web.py research` — one combined call (Monid/TinyFish + Bing + X), cite source URLs.

`search/web.py` needs a Monid key (`MONID_API_KEY`). Set it as an env export; never write the key literally in a file. The Bing leg goes through the `workers/cf-search` Worker (deploy it separately, see its README); the X leg uses the FxEmbed mirrors in `workers/x-fxembed/README.md` (a User-Agent header is mandatory or FxEmbed 401s). DuckDuckGo is removed from the system everywhere.

## 5. Gateway + web chat (`bridge/`)

`bridge/zs-gateway.py` is a stdlib HTTP gateway (port **8766**) speaking ACP to one zerostack process per session (ports 7300–7399). `bridge/zs.html` is the chat page (Pi | Zerostack switcher), `bridge/zs-supervise.sh` respawns the gateway, `bridge/zs.py` is the gateway client used by the file-queue bridge.

```sh
# run the gateway in tmux (a tmux session closes when its main process exits — kill then recreate)
tmux kill-session -t zs-gateway
tmux new -s zs-gateway -d '/root/pi-bridge/zs-supervise.sh'
```

Login is the gateway token (43 chars, in `gateway.env` as `PI_GATEWAY_TOKEN`) pasted into the password field — no opening password exists unless you set `PI_ADMIN_PASS_SHA256`. Point your tunnel at port 8766 to serve the page at `/pi`. Rollback is repointing the tunnel at the Pi gateway (8765).

Gateway behaviour to know:
- ACP registers only `initialize`, `session/new`, `session/prompt` — no cancel (stop kills the process), no load/list/resume/set_model. Sessions die with their process; the gateway owns transcripts and replays a text digest after respawn. Idle sessions release their process after 15 minutes.
- `/prompt` returns `accepted: true`; streaming arrives on `/events`.
- `/stats` reports CPU/RAM/disk against the plan quota.

## 6. Skills (`skills/`)

Zerostack has **no skills directory**; prompts are the only mechanism. Install the four skills as prompts:

```sh
python3 skills/install-skills.py
```

This places them in the prompts directory (`~/.local/share/zerostack/prompts/`). The four: `web-research` (our combined search), `pi-delegation` (rewritten to delegate to Zerostack), `hallmark` (full bundle beside the prompts dir), `resilient-batch-jobs` (checkpointed batch runs). Each was tested on a real task — see FEATURES.md.

## 7. Memory

Zerostack's built-in memory (plain-Markdown, auto-injected): a global `MEMORY.md` plus per-project daily logs, scratchpad and notes. The Pi memory was migrated once (`notes/pi-lessons.md`, `notes/pi-altare-facts.md`, `SCRATCHPAD.md`); Pi's SQLite store stays in Pi and is untouched. Find the exact layout in `src/extras/memory/mod.rs` before moving anything else.

## 8. Browser / heavy tasks (optional, separate box)

The agent box does not run the browser. Browser and heavy tasks go on a second, on-demand box (Instabox) with Tailscale + a Playwright/Chromium MCP server bound to Tailscale only, added to zerostack's MCP config by Tailscale address. That box is not part of this repo.

## 9. Verify

```sh
zerostack -p --name smoke-test "Say OK"          # expect: OK
zerostack -c -p "What did I just ask you to say?" # expect: OK. (session resume)
python3 search/web.py research "zerostack"         # expect: real results + sources
curl -s http://127.0.0.1:8766/stats | head -c 200    # expect: 200, disk vs plan quota
```

## Secrets

Never commit keys, tokens, passwords, Worker URLs or account IDs. Config templates keep `${ENV}` placeholders only. The repo is private, but the sweep runs before every push.
