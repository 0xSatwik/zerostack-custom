# ARCHITECTURE.md — zerostack-custom

The whole system in one file: what runs where, how the pieces talk, and where
the config lives. Keep it under ~300 lines; update it when the shape changes.

## The pieces

| Piece | What it is | Where it runs |
| --- | --- | --- |
| Zerostack 1.8.6 | The agent. Rust, sessions, memory, subagents, hooks. | Altare sandbox "pi" (Ubuntu 24.04, free pool) |
| Rust gateway | Serves `/` (home), `/pi` (chat), `/admin` (keys/models), `/pi/settings`. Port 8766. Single binary, std HTTP + serde_json. | Same Altare sandbox, `/root/pi-bridge/zs-gateway` |
| Cloudflare Tunnel | Permanent public URL for the gateway. | `hardeepwalia.pitunnel.dpdns.org` (tunnel `hardeepwalia`) |
| Browser | Camoufox + Playwright MCP, noVNC viewer, password control page at `/browser`. | Instabox `muse-browser` (4 vCPU / 8 GiB), reached over Tailscale |
| Vault | Cloudflare Worker + D1 secrets vault. Agents hold only a request-only token; keys are never seen by the agent. | `cf-d1-vault` Worker + D1 `cf-vault` |
| Search | Free search/fetch: TinyFish via Monid + cf-search Worker (Bing) + X via FxEmbed. No paid endpoints. | `web.py` on the box + the Monid / worker URLs in `search.json` |
| Recall | Session memory: index + digests + one lesson per mistake. Instant recall without loading transcripts. | `/root/work/recall/` on the box |

## How a prompt flows

1. You open `/pi` (or Muse sends a prompt via the gateway). The gateway owns
   the transcript and spawns a Zerostack ACP process for the session.
2. Zerostack reads `config.toml` (providers, models, fallback order, chain,
   advisor, prompt_to_model) and the prompts in
   `/root/.local/share/zerostack/prompts/`.
3. The model answers with tool calls (file read/write, shell, search). Tool
   results come back; the turn ends when the model stops calling tools.
4. The gateway stores every event in `/root/pi-bridge/zs-sessions/<id>.json`
   + `.events.jsonl` and replays a digest after a restart.

ACP exposes only three methods (`initialize`, `session/new`,
`session/prompt`). There is no cancel; Stop kills the session process.
Sessions die with the process; the gateway owns the transcripts.

## Gateway

- Source: `bridge/zs-gateway-rs/` (Rust). Build with `cargo build --release`,
  copy `target/release/zs-gateway` to `/root/pi-bridge/zs-gateway`, restart by
  killing the gateway PID — the supervisor respawns it. Never `pkill -f
  zs-gateway` (kills both binaries); kill by PID.
- Auth: everything except page routes + `/health` needs `Authorization:
  Bearer <gateway token>`. `POST /admin-login` exchanges the opening
  password for the token. Pages store one token in `localStorage` as `zsgtok`.
- `/admin` manages provider keys (write-only), models (add/edit/default/test),
  search worker URLs, Apify keys/actors, feature toggles, and multi-key health.
- `/pi` (chat) is served from disk (`/root/pi-bridge/zs.html`); it makes zero
  Cloudflare Worker requests. `/admin`, `/pi/settings`, and `/` are embedded
  in the binary — changes need a rebuild + restart.
- Provider 503/429/502 is retried after Retry-After (up to 3 times), then the
  gateway falls to the next provider in the fallback order.
- The gateway is single-threaded: one stuck turn blocks `/health` and all
  pages. Recovery is killing the gateway PID (supervisor respawns it). A real
  non-blocking fix is still open (task.md batch 5).

## Models and fallback

- Providers in `config.toml` `[custom_providers.*]` + `models.json` (the
  gateway projects `models.json` into `config.toml` on `/model-order`).
- Default model: 100milionseek DeepSeek (`deepseek-v4.1-flash-free`). Fallback
  order is in `config.toml` (`fallback_enabled = true`).
- Multiple keys per provider rotate round-robin; a failing key leaves
  rotation immediately, is retried after 2 hours, and is flagged if it fails
  again. Key health shows in `/admin`.
- 100milionseek returns 503 `upstream_busy` when its single upstream is at
  capacity — that is a provider capacity problem, not our key or config.
  The gateway retries it (see above).

## Vault

- Worker `https://cf-d1-vault.hardeepwalia77.workers.dev`, D1 `cf-vault`.
  MCP `POST /mcp`. Unlock is a scoped 2-hour session per service.
- The master secret and DEK backup live in the workspace (chmod 600, never in
  chat). Zerostack holds only a request-only token; the live `config.toml`
  holds the literal token because Zerostack does not expand `${ENV}` in MCP
  headers — the template keeps a placeholder, never a literal token.
- Vault MCP works in the Zerostack TUI only. ACP/gateway mode never loads MCP
  servers; `/pi` sessions use the `vault` CLI / `service_request` instead.
- `enable-exa-mcp` must be top-level TOML. Exa is fully removed.

## Search

- `web.py` (box `/root/pi-bridge/search.py`, repo `search/web.py`) is the only
  search/fetch system: TinyFish via Monid + cf-search (Bing) + X via the
  fixtweet worker. Settings live in `/root/pi-bridge/search.json` (worker URLs
  + leg toggles + backend selector). Never hardcode a worker URL in the repo;
  the repo has placeholders only.
- Backend selector: Monid (default), Bing combination, or Apify (user's own
  actors, key then actor by name). Fetch always uses Monid first, then falls
  back to the Camoufox browser on Instabox.

## Browser (Instabox)

- Machine `muse-browser`, Debian 13. The stack is camofox-browser (Node) on
  127.0.0.1:9377 — not the old Python Camoufox + Playwright MCP (dead).
- Viewer: noVNC + websockify on 6080, control server on 6081, public at
  `/browser`. Login is the gateway token or the opening password.
- The tunnel forwards the full path, so backends must serve the prefixed path.
- Auto-pause: 30 min idle only when nothing is running; a running job keeps
  the box up, with an 8-hour max runtime cap.

## Recall

- Index + digests + lessons at `/root/work/recall/`. A digest is ~200 tokens
  against a multi-MB transcript. Never load a full transcript unless the
  digest is not enough.
- One lesson file per mistake, written the moment it is found. Before risky
  work (gateway restart, tunnel change, MCP config), grep the lessons index.
- The auto-digester is currently stopped: it must not delete the digests of
  sessions that were lost (vs user-deleted) — restart only after that guard
  lands.

## Repos

- `0xSatwik/zerostack-custom` (public) is THE repo for everything. `bridge/`
  is the source of truth; keep top-level copies in sync. Pushes are per-file
  via `gh.py put-file` from the Muse VM (the box has no `gh.py` — it needs the
  Muse VM's Secure Vault). Never push a worker URL, a key/token/secret, or
  anything model-specific that is box-only.
- `0xSatwik/muse-pi-setup` is legacy. `0xSatwik/cf-d1-vault` is the vault repo.
- `README.md` is the setup guide. `task.md` is the master task list.

## Config files (box)

- `/root/.config/zerostack/config.toml` — providers, models, fallback, chain,
  advisor, prompt_to_model, MCP servers. chmod 600.
- `/root/pi-bridge/models.json` — provider/model/key registry the gateway
  manages; projected into `config.toml`.
- `/root/pi-bridge/search.json` — search worker URLs, leg toggles, backend,
  Apify keys/actors. chmod 600.
- `/root/pi-bridge/features.json` — feature toggles (chaining, advisor,
  hooks, prompt_to_model, status signals, built-in memory). chmod 600.
- `/root/pi-bridge/provider-keys.env` — provider API keys. chmod 600.
- `/root/pi-bridge/gateway.env` — gateway token + admin password hash.
- `/root/.local/share/zerostack/prompts/` — the 22+ skill prompts, including
  `capabilities.md` (live feature/tool/MCP/search summary, read at session
  start).

## Limits that are real

- Zerostack subagents panic in ACP mode (`SubagentConfig not initialized`) —
  subagents work in the TUI only. Every delegated brief must say so.
- The gateway is single-threaded (see above).
- Tailscale on Altare is userspace-networking (no `/dev/net/tun`): normal
  processes cannot dial Tailscale IPs. Use the SOCKS5 relay on localhost.
- The Instabox browser idle-shuts-down and launches on demand; the viewer
  needs a login.
