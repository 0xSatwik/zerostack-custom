# zerostack-custom

MIT-licensed setup for **Muse controlling Zerostack on a free box** — the
whole system, with all the code, so a friend can hand this repo to their
Muse and have it set up for them the same way.

**Zerostack is the only agent.** Zerostack (Rust, 1.8.6, open source,
GPL-3.0, `gi-dellav/zerostack`) runs on a free Altare sandbox and does
the long, heavy work on your own model APIs — so it costs your API
credits, not Muse tokens. Pi was fully removed on 2026-11-21 and is
not a fallback. This repo is a customized fork of Zerostack plus the
whole operating system around it: a Rust gateway with a web chat UI,
an admin page, a browser on Instabox, a secrets vault, free search,
and a recall system.

Muse is the planner and the one that talks to you. Zerostack is the
second agent that runs on the free box. Muse reaches Zerostack through
an HTTP gateway (Rust, on the box) over Tailscale (primary) and a
Cloudflare Tunnel (public URL). Zerostack never sees your keys for
other services — for that, see the companion vault below.

## What you get

| Piece | What it is |
| --- | --- |
| **Zerostack 1.8.6** | The agent. Rust, sessions, memory, subagents, hooks. Runs on the Altare sandbox. |
| **Rust gateway** | Serves `/` (home), `/pi` (chat), `/admin` (keys/models), `/pi/settings`. Port 8766. Single binary, std HTTP + serde_json. |
| **Home page** | True-black landing page at `/` with links to Chat, Admin, Browser, Settings + live gateway status. |
| **Browser** | camofox-browser (Node, port 9377) on an Instabox machine, with a noVNC viewer and a password-gated control page at `/browser`. The old Playwright MCP on 8931 is dead. |
| **Vault** | [cf-d1-vault](https://github.com/0xSatwik/cf-d1-vault) — Cloudflare Worker + D1 secrets vault. Agents hold only a request-only token; keys are never seen by the agent. |
| **Search** | Free search/fetch: TinyFish via Monid + cf-search Worker (Bing) + X via FxEmbed. No paid endpoints. |
| **Recall** | Session memory: index + digests + one lesson per mistake. Instant recall without loading transcripts. |

## Live system

- **Public URL:** https://hardeepwalia.pitunnel.dpdns.org — `/` home, `/pi` chat, `/admin`, `/pi/settings`, `/browser`
- **Host:** Altare sandbox "pi" (Ubuntu 24.04, free pool 6 GiB / 150% CPU / 10 GiB disk), SSH `root@kg-bsz02.altare.global -p 25355`
- **Browser host:** Instabox `muse-browser` (4 vCPU / 8 GiB / 20 GiB, Debian 13), reached over the Tailscale tailnet
- **Models:** 6 providers — 100milionseek, howtofix, moss, tokenlab, xingchi (plus any OpenAI-compatible API you add)

## Install — one file, one command, then a short checklist

On a fresh Altare sandbox (Ubuntu 24.04) as root:

```
curl -fsSL https://raw.githubusercontent.com/0xSatwik/zerostack-custom/main/install.sh -o install.sh
bash install.sh
```

**`install.sh` is the single-file install.** It does everything
mechanical, so nobody has to edit files by hand: system packages,
Node 24, Rust 1.100.0, mold (the Zerostack linker), cloudflared,
Zerostack 1.8.6 built from source **with `--features
advisor,hooks,loop,acp`** (default features drop `--acp` and the
binary exits immediately) **plus the subagent ACP-init patch**,
the Rust gateway built from source, **all seven Rust tools**
(`zs`, `search`, `tailnet-relay`, `watcher`, `recall`,
`auto-digest`, `zs-gateway`), the chat/admin/settings pages, the
Zerostack config from the repo template (placeholders only),
`features.json` + `search.json` defaults, the skills/prompts, the
recall system, and the tmux supervisor sessions. It is idempotent —
safe to re-run; it never overwrites your keys, config secrets,
gateway token, or sessions.

It prints a manual checklist at the end for the parts that need YOUR
accounts — provider API keys, Tailscale auth, the vault Worker + D1,
the Cloudflare Tunnel + domain, the opening password, the search
worker URLs, and the Instabox browser. Those can never come from
GitHub.

**For another Muse setting this up for a new user:** hand it this
repo and this README. The order is: (1) run `install.sh` on a fresh
Altare sandbox as root; (2) work through the checklist it prints —
keys in `/admin`, Tailscale up (userspace networking + SOCKS5),
vault deploy, named tunnel (proxied hostname), opening password,
search worker URLs; (3) verify with the checks in "Verify the whole
setup" below. Full step-by-step guide: [`INSTALL.md`](INSTALL.md).
It lists every step, what it does, and what to check after each one.

## After setup: keys and models live in the Admin page

The gateway serves an admin page at **`<gateway-url>/admin`** — this
is THE way to manage keys and models after setup, not console rituals.

**Login:** an opening password whose **SHA-256 hash** lives in the
box's `gateway.env` as `PI_ADMIN_PASS_SHA256` (set it there, restart
the gateway). The page exchanges it for the gateway token via
`POST /admin-login`; the raw gateway token also works in the same
box. Whoever holds the password or token can also prompt Zerostack,
and Zerostack runs as root in the sandbox — treat both like a root
password.

From the page you can: set/remove provider keys (**write-only** — the
API lists names and set/not-set booleans and can never read a key
back), load a provider's **live model list** and add models with
per-model context/output limits and reasoning/vision toggles, edit
models, **make one the default**, test a model ("Say OK"), paste the
**Monid key** (writes the box's Monid credentials file), and restart
the gateway.

**Feature toggles** live in the same Admin page (Features section,
backed by `features.json` on the box): prompt chaining, advisor,
lifecycle hooks, prompt-to-model mapping, status signals, and
built-in memory. All are off by default except built-in memory.
A feature that is off does not run. The agent reads
`skills/capabilities.md` at session start, which lists the live
feature states, tools, MCP servers, and search legs, so it knows
its own capabilities instead of guessing.

**Gateway resilience:** when a provider returns 503, 429, or 502,
the gateway retries the same request after the Retry-After seconds
(up to 3 times), then falls through to the next provider in the
configured fallback order instead of failing the turn.

**The chat page** (`/pi`) is a single self-contained HTML file
(`bridge/zs.html`) served by the same gateway: session sidebar, full
transcript with markdown, collapsible thinking and tool calls,
composer, Stop/Compact/Rename/Delete, model switcher, live updates.
Same login as `/admin`, same sessions as the CLI, no extra model
quota, **zero Cloudflare Worker requests** — everything goes through
the tunnel to the gateway on the box.

**Settings** (`/pi/settings`) shows Zerostack info, installed skills
with Load buttons, and MCP servers (browser + vault — TUI only, see
below).

## Models

Six providers, all OpenAI-compatible (keys never committed — they live
only on the box, `chmod 600`):

| Provider | Model | Base URL | Key env var |
| --- | --- | --- | --- |
| 100milionseek | deepseek-v4.1-flash-free | https://100milionseek.top/v1 | `HMS_API_KEY` |
| howtofix | deepSeek-v4.1-flash-1017-sg | https://api.howtofix.id/v1 | `HOWTOFIX_API_KEY` |
| moss | nex-n2.6 | https://api.mosi.cn/v1 | `MOSS_API_KEY` |
| tokenlab | DeepSeek-V4.1-Flash | https://api.tokenlab.amd.com/v1 | `TOKENLAB_API_KEY` |
| xingchi | gpt-6-astra, gpt-6.1-sol | https://api.xingchiapi.com/v1 | `XINGCHI_API_KEY` |

Keys go in **two** places on the box (both `chmod 600`):
`/root/pi-bridge/provider-keys.env` (the gateway reads this at start)
and `/root/.profile` + `/root/.bashrc` (so interactive shells see
them). After adding or rotating a key, restart the gateway.

**Provider quirks:** xingchi returns 403 `client_not_allowed` in
Zerostack (the provider only admits named coding clients). Providers
are flaky — 100milionseek 502s, tokenlab 429s — a provider fault, not
an integration fault. Moss dies empty on long sessions; prefer
xingchi/gpt-6.1-sol for long work.

## How it fits together

```text
You
  |
  v
Muse (your VM) — plans, talks to you, verifies
  |  zs (delegation client, Rust)      |  browser control (Rust)
  v                                     v
Altare: Tailscale tailnet (100.x)       Instabox: camofox-browser (Node, :9377)
  |  SOCKS5 relay (Altare has no /dev/net/tun)
  v
Zerostack gateway (Rust, tmux `zs-gateway`, :8766)
  |  Bearer token in gateway.env
  v
Zerostack 1.8.6 on the free box (Altare sandbox "pi")
  config: /root/.config/zerostack/config.toml (chmod 600)
  skills: /root/.local/share/zerostack/prompts/
  sessions: /root/pi-bridge/zs-sessions/
```

**Tailscale:** Altare has no `/dev/net/tun`, so tailscaled runs
`--tun=userspace-networking` + `-socks5-server=localhost:1055`
(tmux `tailscaled`); a Rust relay (`bridge/tailnet-relay-rs/`, tmux
`tailnet-relay`) forwards `127.0.0.1:8931/6081/6080` to the Instabox
machine. Ordinary Altare processes CANNOT dial Tailscale IPs directly.
tailscaled uses a custom socket (`/root/tailscale/tailscaled.sock`);
a symlink from the default `/var/run/tailscale/tailscaled.sock` keeps
the `tailscale` CLI working. If the Instabox shows offline in
Tailscale after a resume, restart its tailscaled — it sometimes fails
to reconnect on its own. Port 8931's backend (old Playwright MCP) is
dead; the live browser is camofox on 9377, reached via the
browser-control page on 6081.

**Tunnel:** named tunnel `hardeepwalia` gives the permanent URL.
The DNS record for the tunnel hostname MUST be proxied (orange cloud);
DNS-only gives `ERR_CONNECTION_RESET`. cloudflared forwards the FULL
request path — backends must serve the prefixed path (`/browser`,
`/browser-vnc`).

## Search and fetch (both Muse and Zerostack)

Both agents use the same free system — never the paid endpoints, never
a key written literally in a file. See `search/SEARCH.md` and the skill
in `skills/web-research/`:

- **Tool (Zerostack, on the box):** `/root/pi-bridge/search` — the Rust
  search/fetch binary (same CLI as `web.py`). This is what the
  web-research skill calls now. If the binary is ever missing, fall back
  to `python3 /root/pi-bridge/search.py` (same file, Python reference).
- **Tool (Muse, on the VM):** `python3 ~/workspace/search/web.py` — the
  same system; the Rust binary is box-only (no Cargo on the VM).
- **Engines:** TinyFish free search+fetch via a Monid key (`$0`; search
  12k/day, fetch 1k/day) + cf-search Worker (Bing, no key, free) +
  X/social via FxEmbed (descriptive User-Agent mandatory).
- **Default:** `search research "query"` (or `python3 search/web.py
  research "query"` on the VM) — web results from two engines (deduped)
  + an X ask pack + fetch of the top 5 URLs.
- **Monid key:** only in `~/.config/monid/credentials.yaml` (chmod 600,
  format `key: "<value>"`) — on the Muse VM and on the box.
- **DuckDuckGo was removed (2026-11-19).** It returns nothing from
  datacenter/Cloudflare IPs. The Monid/TinyFish leg is load-bearing.
- **Cloudflare-blocked sites:** when a site 403s every direct request,
  the egress is blocked, not the key. Ladder: (1) `web.py` TinyFish
  fetch/search; (2) a browser task; (3) the vault Worker's
  `service_request`; (4) sandbox egress, last resort. Never paste keys
  into browser tasks or prompts.

## Sessions and delegation rules

- **Delegate frequently.** Long-running, heavy, or architecture work
  goes to Zerostack — it saves Muse tokens (Zerostack runs on your own
  APIs), gives a second opinion, and double-checks designs.
- **One Zerostack session per topic, reuse by default.** Never a fresh
  session when a topic already has one. Registry:
  `/root/work/zerostack-bridge/zerostack-sessions.json`. Standing
  sessions: vault `8012a3a7`, setup-repo `5d1d7b2d`, general-work
  `f685b22e`, model-system `5f7642a3`. Compact when long; fresh only if
  a session dies or gets confused.
- **Report real progress.** Silence during delegated work reads as
  stalled work — check the session, report file-by-file progress, and
  say plainly when a model died empty.
- **Verify before trusting.** A claim counts once it is confirmed
  (on-chain, on the page, in the file), not when it is asserted.

## Known limits (honest, not hidden)

- **Stop works now (2026-11-24).** Pressing Stop (or `POST
  /sessions/<id>/stop`) ends the turn: it sets a `stop_requested`
  flag so the turn does NOT fall back to the next model, kills the
  ACP process, and returns `{"stopped": true}`. Live-verified: a
  long turn stopped mid-flight, no fallback spawn, count back to 0.
- **`/health` is lock-free (2026-11-24).** It reads an atomic
  running-count, so it answers even during a turn. The count is
  guarded on every path that flips `running` (run_turn end,
  stop_session, and the ACP reader thread when a connection ends —
  the reader was the leak: it cleared `running` without decrementing,
  fixed 2026-11-24). A stopped turn leaves the count at 0.
  `rpc` now reports a dead connection as "ACP connection closed
  (process ended)" instead of a fake "timed out after 3600s".
- **The deeper gateway freeze is still open (2026-11-24).** When an
  ACP process dies mid-turn on its own (not via Stop), the turn can
  wedge in fallback and the session endpoints (`/sessions`,
  `/sessions/<id>`) can stop responding — while `/health` still
  answers. Root cause is unpinned: the box runs under `proot`,
  which already traces the gateway, so `gdb` cannot attach for
  stack traces. Recovery is still: kill the gateway PID (the
  supervisor respawns it in ~2s) and retry. A full non-blocking
  refactor of the ACP wait path is the remaining work.
- **Zerostack subagents in ACP mode — FIXED 2026-11-24.** They used
  to panic (`SubagentConfig not initialized`) because ACP never called
  the init the TUI path runs. A 33-line patch in
  `src/extras/acp/mod.rs` (saved as
  `zerostack/subagent-acp-init.patch`) initializes it in ACP `serve()`,
  and the binary is built with `--features advisor,hooks,loop,acp`
  (see `zerostack/BUILD-NOTES.md` — default features alone drop
  `--acp` and the binary exits immediately). Live-verified: a `/pi`
  session's task tool ran a subagent and returned a result, no panic.
- **Vault MCP works in the TUI only.** ACP mode never loads MCP
  servers (`mcp_manager: None`). The gateway drives the browser via
  the Rust `browser` CLI instead.
- **ACP has only three methods** (`initialize`, `session/new`,
  `session/prompt`) — no cancel, load, resume, or model switch.
  Sessions die with their process; the gateway owns transcripts and
  replays a text digest after a respawn.
- **Zerostack does NOT expand `${ENV}` in MCP headers.** The live
  config holds the literal vault token, substituted at deploy time;
  the template keeps the placeholder. Never commit a literal token.
- **`enable-exa-mcp` must be top-level in the TOML.** After a `[table]`
  header it belongs to that table. Exa search is fully removed — search
  is ours only (Monid + Bing + X).
- **The Instabox machine auto-pauses after 30 min idle.** Resume it
  before use — a suspended machine makes `/browser` 502.

## Recall system

Sessions are recalled via the recall system, NOT by loading full
transcripts: `recall/index.jsonl` (one line per session) +
`recall/digests/<id>.md` (~200-400 tokens each) +
`recall/lessons/<slug>.md` (one file per lesson). The auto-updater
runs in tmux `recall-auto` on the box (every 6h) as a Rust binary
(`bridge/auto-digest-rs/`, ported from `auto_digest.py`). It has a
deletion guard: a digest is removed ONLY when the session id is in
`digests/.deleted.json`, which the gateway writes when a session is
explicitly deleted. A session file that is merely LOST (no marker)
keeps its digest — that is how the old standing sessions' digests
survived after their files vanished from the gateway store.
`muse-main` digest is updated by Muse after important work. Full
guide: `recall/SKILL.md`.

## Verify the whole setup

```bash
curl -fsS https://hardeepwalia.pitunnel.dpdns.org/health   # {"ok": true, "impl": "rust"}
# /pi, /admin, /browser, /pi/settings all return 200
# prompt "Say OK" in /pi — should answer OK
/root/pi-bridge/search research "zerostack agent"          # web results + fetches (Rust)
/root/pi-bridge/search search "Rust programming language"  # same CLI as web.py
# Stop check: start a long turn in /pi, press Stop — the turn ends,
# no fallback model spawns, /health sessions_running returns to 0.
```

## Lessons baked in

- **Update existing files in place — never a second live copy of a
  config.** Staging copies are deleted; `.bak` backups are fine.
- **Never `pkill -f "zs-gateway"`** — it kills both binaries. Kill by PID.
- **Never `pkill -f` any pattern inside an SSH command** — it matches
  the SSH session's own shell. Kill by port PID.
- **A "GET A COMPUTER" button is NOT proof of a slot** (Hoody watch
  lesson) — confirm a server is actually claimable before calling it open.
- **A site 403ing everything is an egress block, not a key problem.**
  See the ladder in the Search section.
- **Long-running processes do not see keys added later.** Restart the
  gateway after any key change.
- **The gateway token variable is `PI_GATEWAY_TOKEN`.** The gateway
  reads exactly that name from `/root/pi-bridge/gateway.env`.
- **On Hoody the gateway must bind `0.0.0.0`.** Its default is
  `127.0.0.1`, which the Hoody proxy cannot reach.

## Security

- No secret is ever in this repo: no API keys, no tokens, no
  passwords, no private keys. Files that hold them
  (`gateway-token.txt`, `provider-keys.env`, `tailnet-url.txt`) are in
  `.gitignore` — only `.example` templates are committed.
- Keys live only on the box (`chmod 600`) and in your own messages when
  you first supply them. They are passed to the gateway through stdin
  or the Admin page, never as arguments, never in a file on the Muse
  VM, never in chat beyond your own message.
- A *model* key is visible to the Zerostack process by design (it has
  to call the provider). The vault exists for keys Zerostack *calls via
  MCP* — those Zerostack never sees.

## Repository layout

```text
README.md            this guide
INSTALL.md           step-by-step guide for another Muse
install.sh           one-command installer (fresh Altare sandbox)
cleanup.sh           removes caches/junk, keeps .rustup + live configs
AGENTS.md            the author's operating manual: delegation, search, lessons
LICENSE              MIT
bridge/
  zs-gateway-rs/     the Rust gateway crate (main.rs, acp.rs, http.rs, ...)
  zs.html            the /pi chat page (single self-contained file)
  zs-gateway.py      Python gateway (supervisor fallback only)
  zs-supervise.sh    respawn loop (runs Rust binary if present)
  zs-rs/             Rust delegation client (binary: zs; ask/list/new/show/status)
  browser-control-rs/ Rust /browser control page + Stop/Start (Instabox)
  browser.md         the browser skill prompt (Zerostack)
  session-report.py  session inspector (lists sessions, --model filter)
  tailnet-relay-rs/  Rust SOCKS5 relay Altare -> Instabox (Tailscale)
  watcher-rs/        Rust inbox watcher (tmux pi-bridge, every 5s)
  search-rs/         Rust search/fetch (TinyFish + Bing + X; binary: search)
  recall-rs/         Rust recall CLI (find/digest/lesson/lessons/sessions)
  auto-digest-rs/    Rust recall auto-updater (tmux recall-auto, every 6h)
search/
  web.py             free search/fetch (TinyFish + Bing + X) — the Python
                     reference; the Rust binary (`/root/pi-bridge/search`)
                     is the live tool on the box, same CLI
  SEARCH.md          search system notes
# Recall lives on the box at /root/work/recall/ (index.jsonl + digests/
# + lessons + .deleted.json guard), not in this repo. The repo carries
# the tools: bridge/recall-rs/ (CLI) and bridge/auto-digest-rs/
# (auto-updater, tmux recall-auto, every 6h).
zerostack/
  subagent-acp-init.patch  the ACP subagent-init fix (33 lines, live-verified)
  BUILD-NOTES.md     how to build Zerostack (feature set, mold, the patch)
  config.toml.example  Zerostack config template (placeholders only)
skills/
  web-research.md    the research skill
  hallmark.md        design skill (Taste Skill)
  pi-delegation.md   delegation skill (Zerostack now)
workers/
  cf-search/         Bing search Worker (Cloudflare)
  x-fxembed/         X/FxEmbed Worker
prompts/
  browser.md         browser skill prompt (Zerostack)
  recall.md          recall skill prompt (Zerostack)
```

## Companion repos

- [cf-d1-vault](https://github.com/0xSatwik/cf-d1-vault) — the secrets
  vault (Cloudflare Worker + D1). Separate repo, own full guide.
- [muse-pi-setup](https://github.com/0xSatwik/muse-pi-setup) — the old
  Pi-era repo. Legacy; do not use as the current setup.

## License

MIT. Zerostack itself is GPL-3.0 (`gi-dellav/zerostack`); this repo
is a customized fork of it plus our operating system around it.
