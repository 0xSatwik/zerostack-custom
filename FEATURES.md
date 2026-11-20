# Features — what this custom build adds (and changes vs upstream)

Upstream: [zerostack](https://github.com/gi-dellav/zerostack) 1.8.6, GPL-3.0, by gi-dellav. This fork is private. Upstream credit and the GPL-3.0 licence stay intact.

## Scope rule (the one rule that governs everything below)

**Additive-only.** We keep ALL of zerostack's existing features and remove NOTHING except the search. The search is the single exception: upstream's Exa search is fully removed and replaced with our own (Monid/TinyFish + Bing + X). Context7 and Grep.app stay exactly as upstream ships them. Everything else here is added on top, never a removal.

## Changes vs upstream

### Search — replaced (the only removal)
- `enable-exa-mcp = false`, top-level in the config. It is a top-level field; putting it inside `[mcp_servers.vault]` silently leaves Exa on.
- Exa instruction removed from all 9 bundled prompt files in `data/prompts/` (ask, brainstorm, code, debug, default, plan, review-security, tutor, write-prompt), replaced with one line: use `search/web.py research` — one combined call, cite source URLs. Context7 / Grep.app / Focus lines untouched.
- Caveat: the running 1.8.6 binary has its prompts baked in, so prompt edits live in the source only. Exa cannot run anyway (no tool, no connection, flag off).
- Our search (`search/web.py` + `search/SEARCH.md`): TinyFish via Monid (primary) + Bing via the `workers/cf-search` Worker + X via FxEmbed. DuckDuckGo is removed from the system everywhere. Monid key via `MONID_API_KEY` env export, never literal in a file.

### Vault support — added
- MCP server `vault` (Cloudflare Worker + D1) in the config, agent `zerostack-altare`, request-only token. Vault tool calls return `unlock_required` until Muse unlocks the agent per service (2-hour window).
- **MCP headers do NOT expand env vars** — the live config holds the literal vault token, substituted at deploy time from `$VAULT_AGENT_TOKEN`. The template keeps the placeholder.
- **Vault tools do NOT work through the gateway** (MCP disabled in ACP mode). Terminal (TUI) only.

### Gateway + web chat — added (`bridge/`)
- `bridge/zs-gateway.py`: stdlib HTTP gateway on port 8766, speaks ACP, one zerostack process per session (ports 7300–7399), transcript store, `/pi`, `/admin`, `/stats`, `/sessions`, `/prompt`, `/models`, `/stop`, `/rename`, `/model`, `/compact`, `/settings`, `/switch`, `/restart`, `/keys`. Same gateway token as Pi; no password invented.
- `bridge/zs.html`: the chat page (Pi | Zerostack switcher), all Pi page fixes retained (streaming-bubble-only transcript updates, sanitizer fix, deduped sessions, fast session list).
- `bridge/zs-supervise.sh`: respawn loop for the gateway.
- `bridge/zs.py`: gateway client used by the file-queue bridge (the delegation path after the bridge flip).

### Skills — added (`skills/`)
Zerostack has no skills directory; prompts are the only mechanism. All four are installed as prompts (`skills/install-skills.py`) and each was tested on a real task:
- `web-research` — real research via the gateway and CLI, correct version + cited sources. **PASS.**
- `pi-delegation` — rewritten to delegate to Zerostack; delegated a real task, killed the session, re-prompted, reused the standing session, output verified on disk. **PASS.**
- `hallmark` — built a hero, ran `sloplint.mjs`; 2 FAIL + 7 REFLEX found and fixed, independent re-lint 0/0/0. **PASS.** (187-file bundle sits beside the prompts dir.)
- `resilient-batch-jobs` — 6-unit batch with a planted permanent error and a hang; 6 fsynced records, run continued, resume re-ran nothing. **PASS.**

Three real bugs surfaced during the skill tests and were fixed: `zs.py show` crashed on list-shaped content; the reconciliation rule wrongly counted recorded failures as orphans (now `succeeded - written`); the resume policy now retries a timeout instead of retiring it.

### Config — added (`config/`)
- `config/config.toml.example`: six OpenAI-compatible providers (100milionseek default, xingchi gpt-6.1-sol + gpt-6-astra, tokenlab, moss, howtofix), `max_tokens=65536`, `context_window=197488`, env-var keys only, vault MCP wired, Exa off. Deploy with the literal vault token substituted; the repo keeps the placeholder.

### Workers — added (`workers/`)
- `workers/cf-search/`: the full Bing search Worker source (Bing only, plain-text, `/llm.txt` + `/llms.txt`). Country filtering works; pagination does not (Bing ignores the offset). Deploy separately with wrangler.
- `workers/x-fxembed/`: the X search manual + FxEmbed mirror base URLs (see its README). A User-Agent header is mandatory.

### Memory — moved
- Pi's portable memory copied into zerostack's built-in layout: 5 user preferences + 15 box facts into `MEMORY.md` (auto-injected), 50 lessons into `notes/pi-lessons.md`, 15 facts into `notes/pi-altare-facts.md`, 3 open items into `SCRATCHPAD.md`, plus a daily log entry. Verified live (`memory_search` found a probe codeword; injected context recalled a rule with no tools). Pi's SQLite store, FTS5 index, hit counts and category filters stayed in Pi, untouched.

### Sessions — standing topics
Three standing sessions created through the live gateway: `vault`, `setup-repo`, `general-work` (the default delegation target). The gateway replays a text digest after a respawn, so a session survives a process death but not a gateway restart losing its store.

## What did NOT change
Everything else is upstream as shipped: Context7, Grep.app, all tools, permission modes, sessions, compaction, subagents, hooks, advisor, TUI, ACP, memory system, sandbox/loop/worktree families. Nothing was removed except the search.

## Known limits
- **MCP disabled in ACP mode** — vault tools (and any MCP tools) are unavailable through the gateway; terminal only.
- **xingchi 403** `client_not_allowed` — provider admits only named coding clients. Not fixable from our side.
- **ACP is minimal** — only `initialize`, `session/new`, `session/prompt`. No cancel, load, list, resume or set_model; sessions die with their process; the gateway replays a digest.
- **100milionseek flaky** (502/503/524 at times); **tokenlab 429 "at capacity"** transiently. Sessions were switched to tokenlab; switch back per session with `POST /sessions/<id>/model`.
- **Read outside cwd asks permission** and headless `-p` denies it (use `bash`; ACP auto-approves).
- **Tunnel** is Cloudflare Tunnel (unmetered); the `/pi` page makes zero Worker requests. The gateway's only outbound call is `/provider-models`, only when manually triggered from `/admin`.
