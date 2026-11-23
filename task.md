# task.md — Zerostack feature gaps: minute-checked, 2026-11-22

Source: deep research comparing upstream zerostack 1.8.6 (cloned) against our binary,
config, and live box state. Our binary is stock upstream 1.8.6 + 9 edited prompt files
(Exa removal). The question for each item was: is it compiled in, and did we switch it on?

## DO — in priority order

### 1. Fix subagents in ACP mode (gateway /pi)
- **Evidence:** `src/extras/subagents/mod.rs:43` — `.expect("subagents: SubagentConfig not initialized (call init() in main.rs)")`. `init()` is called in `main.rs` (TUI entry) but never on the ACP path. `src/extras/acp/mod.rs:331` sets `mcp_manager: None`; ACP only announces subagent calls (`[subagent] name`, lines 398–414) without running them.
- **Problem:** subagents are compiled in (default feature) but panic in ACP — our main mode. They only work in the terminal.
- **Fix:** call `SubagentConfig::init()` in the ACP startup path too, rebuild, test a `task` call via /pi.
- **Why first:** unlocks parallel codebase exploration in /pi, and the EGM multi-lens reviewer swarm (item 9) depends on it.
- **Effort:** small Rust patch + rebuild + live test. Verify what `init()` needs before patching.

### 2. Wire recall into the built-in memory (stop MEMORY.md going stale)
- **Evidence:** `/root/.local/share/zerostack/agent/memory/MEMORY.md` is 9,200 bytes, last touched **Nov 20 16:48**, full of stale Pi-era content (mentions Pi, muse-pi-setup "now private", old tunnel origin port 8765). `projects/` has only `work-1d4b30ea`. Memory injection itself works — `src/agent/builder.rs:172,541` calls `append_memory_block` into the system prompt preamble (shared builder, so ACP sessions get it too). Recall is healthy but separate: 8 sessions in `index.jsonl`, 17 lessons, 192K total.
- **Problem:** two memory systems that don't talk. Built-in memory injects stale text into every session; recall is fast and current but never injected.
- **Fix:** make the auto-digester also maintain a curated MEMORY.md from recall digests (fresh facts only, deduped), and one-time clean the stale Pi-era lines out of the live MEMORY.md.
- **Effort:** medium (script change + one cleanup).

### 3. Enable prompt chaining
- **Evidence:** `src/ui/app.rs:1195` — chaining only runs `if let Some(ref chain_cfg) = self.ui.cfg.chain`. Our `config.toml` has no `[chain]` section, so `cfg.chain` is `None` and chaining is OFF. Defaults when the section exists: `brainstorm-to-plan = true`, `plan-to-code = true`, `code-to-review = false` (`src/config/types.rs:167-173`, `ChainConfig::default`).
- **Fix:** add to `config.toml`:
  ```toml
  [chain]
  brainstorm-to-plan = true
  plan-to-code = true
  code-to-review = false
  ```
- **Caveat:** the chain prompt is rendered by the TUI renderer (`src/ui/renderer.rs`). Verify whether it fires in ACP mode (/pi) or is TUI-only before promising it in /pi.
- **Effort:** 3 lines.

### 4. Write ARCHITECTURE.md for zerostack-custom
- **Evidence:** the `archmd` feature is in the default build, and our own prompts already reference it — `ask.md` ("read … ARCHITECTURE.md if present"), `code.md` / `default.md` ("update ARCHITECTURE.md to match, keep it under ~300 lines"). No `ARCHITECTURE.md` exists in the repo (root or `docs/`).
- **Problem:** the feature is wired but the file it reads/writes doesn't exist.
- **Fix:** write one for zerostack-custom (<300 lines): gateway, tunnel, vault, recall, browser, repos.
- **Effort:** low, one file.

### 5. Enable Advisor (second model, mid-session)
- **Evidence:** binary has all `--advisor` flags. `src/config/types.rs:219-240` — `AdvisorConfig { enabled: false, model: Some("deepseek-v4-pro"), max_uses: Some(3), human_handoff: true, advisor_kilobytes_limit: 256 }`. Our config has no `[advisor]` section, so it's off. Default model `deepseek-v4-pro` is **not** on any of our providers — must be overridden.
- **Fix:** add to `config.toml`, e.g. model `deepseek-v4.1-flash-free` (100milionseek) or `nex-n2.6` (moss), `max_uses = 3`, keep `human_handoff = true`.
- **Effort:** a few lines. Verify the advisor tool fires in ACP (it is listed in `--help` tools).

### 6. Add prompt_to_model mapping
- **Evidence:** `src/config/mod.rs` has `prompt_to_model: Option<HashMap<String, String>>` — switching prompt can auto-switch the model. Our `config.toml` doesn't set it, so prompts switch but the model stays put.
- **Fix:** map e.g. `plan`/`brainstorm` → cheap fast model, `code`/`review` → stronger model.
- **Effort:** a few lines.

### 7. Configure lifecycle hooks (settings.json)
- **Evidence:** binary has `--no-hooks` / `--hooks-test`. Schema in `src/extras/hooks/settings.rs`: top-level `hooks` key in `settings.json`, `HashMap<event, Vec<HookGroup>>`; `HookGroup { matcher, hooks }`; `HookHandler { type: "command"` (only type supported in v1), `command, args, timeout, async, if, once }`. No `settings.json` exists anywhere under `/root/.config` or `/root/.local/share/zerostack` — zero hooks run.
- **Fix:** start with 1–2 safe hooks (log every write; block destructive commands). Look up the exact event names (`PreToolUse`, etc.) in `docs/CONFIG.md` before writing the file, then dry-run with `--hooks-test`.
- **Effort:** low.

### 8. Status signals — pass --status-socket from the gateway + a listener
- **Evidence:** feature is in the default build, binary has `--status-socket`. Grep of the gateway (`zs-gateway.py`, `zs-gateway-rs/src/*.rs`) for `status-socket` returns nothing — the gateway never passes it, so no run-state events (start/stop/permission waits) go anywhere.
- **Fix:** gateway spawns Zerostack with `--status-socket <path>`; add a tiny Unix-socket listener that surfaces run state (e.g. stuck on permission wait).
- **Value:** visibility into stuck sessions — we've had gateway blocks where nothing told us a session was waiting.
- **Effort:** medium (gateway change + listener).

### 9. EGM Goldfish review (fresh-eyes reviewer)
- **Evidence:** from `google/elephant-goldfish-model` — Elephant holds context and drafts the spec; Goldfish reviewers read the draft with deliberately **zero** prior conversation to catch unstated assumptions. Our Advisor is the opposite (consulted *with* context).
- **Fix:** add a Goldfish prompt + a fresh-session pattern: new Zerostack session given only the design doc, no history, returns findings. No code change. The parallel multi-lens swarm (structure/style/security/cost reviewers at once) needs item 1 fixed first.
- **Effort:** low (one prompt file + a session pattern).

### 10. EGM drift report
- **Evidence:** EGM's drift report checks the codebase against the design doc and reports what's drifted. Our recall digests are the feed for it.
- **Fix:** a periodic Zerostack task: compare live code against AGENTS.md / ARCHITECTURE.md / design docs, report drift.
- **Effort:** low–medium.

### 11. Apify integration — multi-key + multi-actor, UI-driven (user's own actors)
- **Evidence:** user's Apify profile `agentxpro` has 4 public actors (tweet scraper, web search API, website scraper, reddit scraper). The X one matches our fixtweet worker logic — skip it. The other 3 (web search API, website scraper, reddit scraper) are the user's own. Running your own actors costs only platform compute from the $5/month free credits; the per-event prices ($0.35/1K, $0.90/1K) are what *other* people pay — the owner is "Not charged" for their own runs per Apify docs, and earns 80% of others' payments.
- **What to build:** in `/admin`, same pattern as the Monid and Instabox key cards — an Apify section that accepts **multiple API keys**. Under each key, a new section opens where the user adds **multiple actors by name + actor ID** (name field + actor ID field, add-another). **No pre-adding:** do NOT hardcode or pre-fill any actor IDs — the user adds them via the UI. Each actor becomes a search/fetch leg in `web.py` (search API leg, page-scraper fetch leg, Reddit leg) with an on/off toggle in `/admin`, config in `search.json`, token from the vault (never in chat, never in code).
- **How the IDs work:** actor IDs are public — `GET https://api.apify.com/v2/acts/agentxpro~<slug>` returns the actor object with its `id`, no token needed. The `username~slug` form also works directly in run calls. The token is only needed to *run* actors (`POST /v2/acts/{actorId}/runs`, `Authorization: Bearer <token>`), and results come back from the run's dataset. **Gap found during research:** the API returned empty input schemas for these actors (version 0.0) — read the exact input JSON per actor from its apify.com page at build time.
- **Effort:** medium (gateway settings + web.py legs + UI). Needs the user's Apify API token via the secure vault flow before building.

#### 11a. Search backend selector (settings option)
- **What:** a search settings option in `/admin` (and the search settings in `search.json`) letting the user pick which search backend `web.py research` uses: **Monid**, **Bing combination**, or **Apify**. One backend at a time, user-settable, default stays Monid.
- **Apify flow:** when Apify is selected, the Apify section appears: user first selects one of their saved Apify API keys, then selects the actor by name (from the name + actor ID pairs they added under that key). Key → actor name → runs. Same pattern for every Apify leg.
- **Reddit search option:** add a Reddit search option alongside web search — backed by the user's Reddit Scraper actor (Apify). It appears as its own leg/toggle in search settings and in `web.py` output (source label `reddit`).
- **Apify option visibility:** the Apify key/actor selectors only show when Apify is the selected backend (or when an Apify leg is enabled). Otherwise the settings stay clean with just Monid/Bing.
- **Fetch rule:** fetch **always uses Monid** for web research. If Monid fetch gets blocked/fails, fall back to the **Camoufox browser** (Instabox, port 9377) to fetch the page. The web research search leg itself is the 34-source Apify actor when Apify is the selected backend; otherwise Monid/Bing as today.
- **Skill:** update the web-research skill (`~/workspace/custom-skills/web-research/SKILL.md`, repo `skills/web-research.md`, box prompt) with the backend selector, the Apify key → actor flow, the Reddit leg, and the fetch Monid → Camoufox fallback — so the agent (and the user reading the skill) understands how it all works. No hardcoded actor IDs or keys in the skill.
- **Effort:** medium-large (gateway settings + search.json schema + web.py backend switch + Reddit leg + fetch fallback + /admin UI + skill). Build after the actor endpoint details are confirmed from the actor pages.
- **Completion rule (user, 2026-11-22):** once all the task.md items for this work are done, update GitHub with ALL the files — code, settings, skill, and the guide too (README / setup docs updated to explain the new backend selector, Apify key → actor setup, Reddit leg, and fetch fallback). Nothing stays local-only.

**USER GO-AHEAD (2026-11-23 ~15:52 IST): do ALL the buildable items now, in order.** Queue: (1) Apify 11/11a + multi-key 12 — already running in session search-apify2, finish first. (2) Feature toggles + capabilities skill (below). (3) Items 1–10 in priority order. (4) Rust conversions, one at a time: browser-control.py, zs.py, tailnet-relay.py, watcher.py, search/recall scripts. (5) Carried-over fixes (gateway non-blocking, browser Stop/Start, Camoufox auto-resize, /browser-vnc gate, recall-auto restart). Blocked items (Zhihu leg, Keenable leg, Apify live test) wait on the user's keys/tokens via the vault. Push everything to GitHub including the guide as each batch lands.

### 13. Feature toggles + capabilities skill (user request 2026-11-22)
- **Toggles:** on/off switches in /admin (settings section, where the search worker URLs live) for each feature: prompt chaining, advisor, lifecycle hooks, prompt-to-model mapping, status signals, built-in memory, and each search leg (Monid, Bing, X, Reddit, Apify). A feature that is off does not run and its settings hide.
- **Capabilities skill:** a file the agent reads at session start (a prompt/skill) listing what it has — features on/off, tools, MCP servers (browser + vault, TUI-only note), search legs and which backend is selected, vault/browser access. So the agent knows its own capabilities instead of guessing.
- **Effort:** low–medium (settings schema + /admin UI + one prompt file).

### 14. Gateway resilience: retry on provider 503 + verify fallback triggers (user request 2026-11-23 — fix on the live site, push to GitHub)
- **Root cause (verified 2026-11-23 ~20:13 IST):** 100milionseek returns 503 `upstream_busy` when its single upstream is at full capacity (live status: 566 in flight, 252,665 retries, 127k failed). The repo and config were correct — base URL, model, key all match the docs. The docs say 503 means "service at full capacity, retry after the Retry-After seconds" (20s).
- **Fix:** when a provider returns 503 (or 429/502), retry the request after the Retry-After seconds instead of failing the turn and dying. Verify the gateway fallback actually triggers — `fallback_enabled = true` and `fallback_order` are set in config.toml, but the session died on a 503 instead of falling back. Find why and fix it so a 503 moves to the next provider in the order.
- **Deploy + push:** this is a live-site fix — rebuild and restart the gateway so the live site has it, and push the code to GitHub with the guide. Same rule as the rest of the queue: every batch goes live AND to GitHub.
- **Effort:** small–medium (gateway retry + fallback path).

### 12. Multi-key rotation + key health per provider (advanced, user request 2026-11-23)
- **What:** allow adding MULTIPLE API keys for the same provider/base URL (e.g., several 100milionseek keys for DeepSeek). The gateway rotates requests across the keys round-robin, so load spreads and responses are faster, and one key hitting a rate limit does not block the others.
- **Key health / circuit breaker:** track each key's health. If a key fails (auth error, 429, 5xx, timeout), STOP sending requests to it — take it out of rotation immediately. Retry it after 2 hours; if it fails again on retry, flag it as bad (visible "flagged" state in /admin, excluded until the user re-enables or replaces it). A key that recovers on retry goes back into rotation.
- **UI:** /admin provider card shows each key (masked, write-only as today) with its health state (active / cooling down until <time> / flagged). Add-another-key per provider.
- **Verify first:** the model system already claims "multiple keys per provider with round-robin rotation" — verify it actually works end-to-end (add a second key for the same base URL, confirm rotation happens and a failing key is skipped), because the user reports it does not work today. Fix it if it is broken, do not just re-document it.
- **Effort:** medium (gateway key store + rotation + health state + 2h retry timer + /admin UI). Builds on the same gateway/model code as the Apify work (item 11/11a). When done, push everything to GitHub including the guide, per the completion rule.

## SKIP / DEFER

### Multimodal (images/PDFs) — NOT in our binary, defer
- **Evidence:** `strings` on the binary finds no `multimodal` references; `--help` has no attach/image flags. Upstream has `/add` + a `multimodal`/`pdf` Cargo feature, but our build doesn't include it.
- **Why defer:** we'd need a rebuild with `--features multimodal,pdf`, and we rarely attach images/PDFs to agent messages — the browser already returns screenshots as files. Revisit if a real need appears, and verify `/add` works after any rebuild.

### Ralph Wiggum loops — already have it, no action
- **Evidence:** no code literally called "Ralph" (nickname only). The `loop` feature is in the binary: `/loop` in TUI, `--loop --loop-max N` headless.

### Code Mode vs MCP (Fred tweet) — researched 2026-11-22, verdict: DON'T switch, partially adopt
- What it is: Cloudflare's "Code Mode" (blog 2026-02-20, "give agents an entire API in 1,000 tokens"). Agent gets 2 tools: `search` (writes code to filter the API spec, only matches come back) and `execute` (writes code calling those functions, runs in a sandbox, only the final result returns). Cloudflare's 2,500-endpoint API as MCP tools = 1.17M definition tokens; Code Mode = ~1,000. Real end-to-end savings on normal tasks are ~30–80%, not 99.9%. And Code Mode does NOT replace MCP — Cloudflare's Code Mode server IS an MCP server with 2 tools.
- Why not switch for us: (1) in our main mode (ACP/gateway) MCP doesn't run at all, so there's nothing to switch from; (2) our tool catalogs are tiny (vault ~5 tools, browser ~10 commands) — definition tokens are ~2–5k, code mode's search + code-writing overhead could cost MORE, especially with our small/cheap models; (3) naive code mode breaks the vault's core promise (agent-written code handling the token); (4) our "sandbox" is a root shell on the box holding every secret — far weaker than Cloudflare's isolate, and even that had escapes (Check Point, Aug 2026, workerd bugs).
- What IS worth doing: a small `vault` CLI on the Altare box wrapping the existing `service_request` API (server-side key injection, token never in context) + a `vault search` discovery command — gives /pi sessions vault access they currently lack entirely. Browser path is already code-mode-shaped (Rust CLI), nothing to switch there. Keep MCP for TUI. Also fix the literal token in `config.toml` regardless — it undermines both designs.

### AVA repo — researched 2026-11-22, verdict: NOTHING to adopt except one idea
- AVA (llmsresearch/AVA, TMLR 2026) is a Python research framework for spending a fixed token budget on short-answer Q&A (GSM8K/MATH/HotpotQA/HumanEval), not a coding agent. No tools, no file editing, no sessions, no memory, no MCP, no subagents, no server, no UI. Backends are Ollama or Azure OpenAI only — we use neither. Its budget accounting counts output tokens only (input context is our dominant cost), and its vote mechanism needs an extractable answer signature, which open-ended coding work doesn't have.
- **One portable idea:** "verify only when uncertain" — run the second review/Goldfish pass only on risky or uncertain changes, not on everything. Prompt-level, ~free. Fold into item 9.
- Do not port AVA code, do not add it as a dependency, do not replace any component with it. Token-budget enforcement would need gateway observability of Zerostack's per-session token use first (ACP doesn't report it reliably) — a prerequisite project, not an AVA adoption.

## CARRIED OVER — pre-existing open flaws (not from the 10 features)
- Gateway is single-threaded: one stuck task blocks `/health` and all pages. Real non-blocking fix still open.
- `recall-auto` restart blocked until the updater distinguishes user-deleted sessions from lost ones (else it deletes surviving digests).
- Browser control Stop/Start buttons call `stop-browser.sh` / `browser-stack.sh`, which don't exist on the box.
- Camoufox window still opens tiny/off-screen; durable auto-resize fix open.
- Direct noVNC path `/browser-vnc/vnc.html` is not password-gated.
