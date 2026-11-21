# AGENTS.md

> Copied from the author's own operating manual. The rules and lessons are
> the point; the `~/workspace/...` paths are examples from the author's
> setup — adapt them to wherever you put this repo.

Your operating manual for this workspace, written by you. Your main instructions cover how you work in general. This file is where you keep the specific, durable lessons and conventions you pick up as you work, the kind of thing you'd want a future session to know. It's not about the user (that goes in `USER.md` and your memory) or your personality (that's `SOUL.md`); it's about how you get work done here.

## Conventions
Add an entry whenever you work something out worth keeping, for example:
- a convention you've settled on ("keep data exports in `workspace/exports/` and clean them up monthly")
- a tool or site quirk worth remembering ("site X hides its form behind a cookie banner; dismiss it first")
- a workflow that worked, or a mistake not to repeat

It starts empty and is meant to grow slowly. Don't pad it; a short, accurate file beats a long, stale one.

## Altare Pi delegation (standing rule, 2026-11-17)
- Frequently delegate long-running / heavy / architecture work to Pi
  via `~/workspace/altare-pi-setup/pi-bridge/muse-pi.py` (ask / send+get)
  on Altare, or via `pi-api.py` over the Hoody `http-8765` URL on a
  Hoody computer. Reasons: token saving (Pi uses the user's custom APIs),
  extra intelligence (second model/opinion), better architecture (Pi
  double-checks designs). The Pi side (paths, models, sessions) is the
  same on both hosts; only how Muse reaches it changes.
- Models: xingchi/gpt-6-astra, xingchi/gpt-6.1-sol, tokenlab/DeepSeek-V4.1-Flash, moss/nex-n2.6, howtofix/deepSeek-v4.1-flash-1017-sg, 100milionseek/deepseek-v4.1-flash-free. Watcher runs in tmux `pi-bridge` on the box; restart it after a box restart.
- Sessions (standing rule, user choice 2026-11-17): ONE Pi session per topic, reuse by default — never a fresh session when a topic already has one. Registry: `~/workspace/altare-pi-setup/pi-bridge/pi-sessions.json`. Continue with `pi-api.py prompt --session <id>`; register new topics there. Compact when long; fresh start only if a session dies or gets confused. Prefer xingchi/gpt-6.1-sol for long sessions.

## Web research (standing rule, 2026-11-17; Worker added 2026-11-19)
- Always use the combined search/fetch system: `python3 ~/workspace/search/web.py research "q"` — TinyFish free via Monid + cf-search Worker (Bing, no key), then fetch top URLs. Skill: `~/workspace/custom-skills/web-research/SKILL.md`. Pi uses the same file at `/root/pi-bridge/search.py`. Never use paid Monid endpoints; never write API keys literally in files.
- cf-search Worker (Bing, no key): YOUR Worker URL — Muse asks the user for it during setup and sets CF_SEARCH_URL; never hardcoded. Plain text, one line per result. Country-wise filtering works (setlang+cc); pagination does NOT work for web/news (Bing ignores `first=` in every form tested — page=2 returns the same results as page=1). Google News (503) and DuckDuckGo (202 bot-challenge) do NOT work from Cloudflare's network. Full docs: `docs/SEARCH-WORKER.md` and `cf-search-worker/llms.txt`.

## Cloudflare Tunnel — permanent URL (lessons, 2026-11-19)
- A named tunnel gives a permanent URL (`https://<name>.<domain>/admin`); a quick tunnel gives a random `trycloudflare.com` URL that changes on restart. Use the named tunnel; keep the quick tunnel only as a fallback until the permanent URL is verified loading.
- **DNS record MUST be proxied (orange cloud).** With `proxied: false` (DNS-only) the browser hits the tunnel address directly and gets `ERR_CONNECTION_RESET`. A tunnel hostname only works when Cloudflare's edge proxies it to the tunnel. DNS needs ~60s to propagate after the fix.
- **Never restart cloudflared inside its tmux session.** A tmux session closes when its main process exits, and cloudflared IS the main process. Always `tmux kill-session -t pi-tunnel` then `tmux new-session -d -s pi-tunnel 'cloudflared tunnel --config ... run'`.
- The CNAME to `<uuid>.cfargotunnel.com` only works inside the SAME Cloudflare account as the tunnel. The domain/zone must be in the tunnel's account.
- Full guide: `docs/TUNNEL.md`.

## Cloudflare-blocked sites (lesson, 2026-11-17 — user-taught)
- Symptom: Cloudflare 403 "Your request was blocked" on direct curl AND browser.open AND the cloud browser, even with no auth and even on the site root (e.g. numes.ai, trycloudflare.com). That means the VM's egress is blocked, NOT the key or credentials — the key is never even evaluated. Stop retrying direct requests; diagnose once (no-auth curl, site root, browser UA) and move to the ladder.
- Ladder, in order: (1) `web.py` TinyFish fetch/search — server-side, often gets pages direct fetch can't; (2) browser task — a fresh browser/route can clear the block (bot-challenge recovery moved the tab and the page loaded); (3) vault Worker `service_request` — the Worker runs on Cloudflare's own network and reaches blocked APIs (Numes /models returned 200 through it); (4) Pi sandbox egress, different network, last resort.
- Cloudflare's Markdown for Agents (`Accept: text/markdown`) and Web Bot Auth (`tag="web-bot-auth"` signed headers) are NOT bypasses — both need the destination site to opt in / allowlist the agent. Don't waste time on them for a blocking site.
- Never paste API keys into browser tasks or Pi prompts. Put the key in the vault, unlock it for Pi, and let Pi call it via `service_request` — Pi never sees the value.

## Pi Chat page `/pi` + gateway lessons (2026-11-20)
- The gateway serves a full chat UI at `/pi` (`bridge/pi.html`, single self-contained file, no CDN): sessions sidebar, transcript, composer, Stop/Compact/Rename/Delete, model switcher, live events polling, CPU/RAM/Disk stats bar (`/stats`). Same login as `/admin` (opening password or gateway token), same sessions as the CLI, no extra model quota, zero Cloudflare Worker requests — all traffic goes through the tunnel to the gateway. Full guide: `docs/PI-CHAT.md`; patch doc: `docs/PI-CHAT-PATCH.md`.
- **Gateway MUST start with `pi` on PATH**: `tmux new -s pi-gateway -d 'PATH=/root/.pi/agent/bin:$PATH exec python3 /root/pi-bridge/pi-gateway.py >> /root/pi-bridge/gateway.log 2>&1'`. Without the PATH the gateway starts fine but every prompt fails with `FileNotFoundError: pi`. Without the log redirect a crash leaves no trace.
- **The gateway is single-threaded — any uncaught exception in a request handler kills the whole server.** `_body()` now wraps `json.loads` in try/except and returns `{}` on a malformed body (a non-JSON POST used to kill it). Keep every handler exception-proof.
- `/stats` disk is the workspace footprint (`du` of `/root`, cached 5 min) against the plan quota (10 GiB), NOT `statvfs("/")` — inside the container that is the 845 GB host overlay and meaningless.
- `/pi` freeze lesson: never re-render a whole transcript per streamed token. The live bubble updates one DOM node per token delta; the transcript renders the last 100 messages with a Show-older button; the sidebar only re-renders when its signature changes.
- **The sanitizer is a silent breaker (2026-11-20).** `pi.html` routes ALL dynamic HTML through `set()`, which allow-lists tags/attributes and unwraps/drops the rest with no error. One missing entry breaks a whole feature invisibly: `data-act` missing → session clicks dead (the page looked fine, nothing opened); `data-q` missing → suggestion chips dead; `OPTION`/`OPTGROUP`/`value`/`selected` missing → model dropdown empty; `BUTTON` missing → Show-older button rendered as dead text. Rule: whenever `set()`-built markup gains a tag or attribute, add it to `OK_TAGS`/`OK_ATTR` in the same edit, and build `<option>`s with DOM APIs — the HTML parser drops `<option>` tokens outside a `<select>`, so `set()` can never render them.
- **`/sessions` must stay cheap (2026-11-20).** The gateway used to JSON-parse every line of every session file per poll — ~8 s across 71 files — and the page polls every 2–10 s, so polls queued, the listen backlog (default 5) overflowed and cloudflared answered 502. Now the scan only parses header/`model_change` lines, the list is cached 30 s, the backlog is 128, and each entry carries its model. Never let a poll handler do O(all-files) parsing again.
- **Narrow-screen drawer z-index (2026-11-20).** On ≤820 px the fixed drawer (`z-index: 15`) slid under the blurred topbar (`z-index: 20`), hiding the search box and + New button. Drawer is now `z-index: 30`, scrim 25. If a drawer/header element ever looks "covered" on mobile, check stacking before anything else.

## Pi Admin page + keys (2026-11-18)
- After setup, ALL key/model management goes through the Pi Admin page (gateway URL + `/admin`): provider keys (write-only — the API never returns a value), model add/edit with per-model context/output/reasoning/vision, Make default, Test, the Monid key, gateway restart. Console rituals are the fallback, not the default. Login is the opening password (SHA-256 in the box's `gateway.env` as `PI_ADMIN_PASS_SHA256`) or the gateway token; 5 failed logins lock it for 10 minutes.
- Never paste keys in chat. Keys enter via the Admin page, Muse's secure connector pages, or stdin (`pi-api.py setkey` / `--key-stdin`) only — never as arguments, never in a Pi prompt, never in a browser task.
- Search is the combined system ONLY (TinyFish free endpoints via Monid + cf-search Worker + X via FxEmbed) — never paid Monid endpoints. DuckDuckGo was removed 2026-11-19 (returns nothing from Pi/Cloudflare). The Monid leg is load-bearing: no Monid key on a side means no search results on that side.
- Hallmark (`nutlope/hallmark`) is the default design skill on both sides — Muse: `~/.agents/skills/hallmark` (+ `~/workspace/custom-skills/hallmark`); Pi: `/root/.pi/agent/skills/hallmark`. The `npx skills add` route fails in this environment; install by cloning `nutlope/hallmark` and copying its `skills/hallmark/` folder (see COMPLETE-SETUP.md §9).

## Zerostack — the ONLY agent (Pi REMOVED 2026-11-21)
- Zerostack 1.8.6 is the agent. **Pi was fully removed from the server
  on 2026-11-21 (user order) and is NOT a fallback.** There is no Pi
  gateway, no pi.html, no Pi sessions, no /root/.pi, and no Pi/Zerostack
  switcher — POST /switch rejects "pi". The tunnel serves only the
  Zerostack gateway on 8766. Do NOT reinstall Pi or re-add a switcher.
  (A backup of the removed Pi files lives only on the author's Muse VM,
  not on the server, in case the user changes their mind.)
- Gateway files: `bridge/zs-gateway-rs/` (Rust, **live since 2026-11-21**,
  9 MB), `bridge/zs-gateway.py` (Python backup), `bridge/zs.html` (the
  chat page), `bridge/zs-supervise.sh` (runs Rust when the binary
  exists, Python otherwise); config template
  `zerostack/config.toml.example`. Full guide: `docs/ZEROSTACK.md`.
  Build the Rust gateway with `cargo build --release` in
  `bridge/zs-gateway-rs/` and copy the binary to
  `/root/pi-bridge/zs-gateway`. Restarting the supervisor tmux
  session re-reads `zs-supervise.sh` — editing the file alone does
  not change the running loop.
- **One-command install:** `install.sh` (top level) automates the
  mechanical setup on a fresh Altare sandbox; `INSTALL.md` is the
  step-by-step guide for another Muse (including the manual secret
  steps that can never come from GitHub).
- **ACP has only three methods** (`initialize`, `session/new`,
  `session/prompt`) — no cancel, load, list, resume, or set_model.
  Sessions die with the process; the gateway owns transcripts and
  replays a text digest. `/stop` kills the session's process.
- **MCP is disabled in ACP mode.** Vault tools work in the Zerostack
  TUI only, not through the gateway.
- **Zerostack does NOT expand `${ENV}` in MCP headers**
  (expand_env is provider-only). The live config holds the literal
  vault token, substituted at deploy time; the template keeps the
  placeholder. Never commit a literal token.
- **`enable-exa-mcp` must be top-level in the TOML.** After a
  `[table]` header it belongs to that table — the first deploy had it
  inside `[mcp_servers.vault]` and Exa stayed on.
- **xingchi 403 `client_not_allowed`** in Zerostack — the provider
  only admits named coding clients. The Responses API fix does not
  help.
- **Switch = tunnel port change + cloudflared restart** (kill the
  tmux session, recreate it; never restart cloudflared inside its
  session). Verify with a fresh browser load — a search fetch can
  serve a cached copy of the page for minutes after a repoint.
- **The switcher's Pi button needs `ZS_PI_URL`** set to a public Pi
  address (env at gateway startup); the default is box-local and
  dead from a phone. Until a second hostname exists, Pi is reached
  by the rollback port change, not the button.

## Repo rule (standing rule, 2026-11-21 — user correction)
- **zerostack-custom is THE repo.** All new files, all projects, all setup, everything goes there — NOT muse-pi-setup (old Pi-era repo, legacy). Link to zerostack-custom, push everything there. zerostack-custom is PUBLIC since 2026-11-21 ~12:04 IST (user order; secret scan clean before flipping).
- zerostack-custom had STALE top-level duplicates (zs.html, main.rs, admin.html without the bug fixes or home page) while bridge/ was current. Fixed 2026-11-21 ~12:00 IST: top-level synced to bridge/, all files verified SAME. Keep them in sync — the top-level copies are the ones people read first.
- **README.md is our setup guide** (rewritten 2026-11-21 ~12:17 IST for the Zerostack-only system). The old Pi-era README is gone. If the README mentions Pi as the agent, it is stale — rewrite it.

## Three bugs fixed (2026-11-21 ~11:48 IST)
- **/pi stuck on "thinking":** gateway acp.rs push_block() stored text blocks with NO "type" field, so zs.html blocksToHtml() dropped every text block. Fixed: blocks carry {"type": kind}, render by field presence, single pending transcript timer.
- **/admin login:** setAdminPw sent the password untrimmed but login trims (stray space broke login) — now trims on set; loadAll retries once after 1.2s on transient failure.
- **/admin mobile:** full responsive CSS ≤760px (44px min touch targets, stacked fields, scrolling tables, fluid login card).
- Long-project test PASSED in one /pi session (session-report.py, 527 events).
