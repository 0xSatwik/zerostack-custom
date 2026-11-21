<!-- Installed for Zerostack on the Altare box, 2026-11-20. -->
<!-- Zerostack has no skills directory: this file IS the skill, as a named prompt. -->
<!-- Load it with `/prompt browser`, `.browser <msg>`, or `zerostack --load-prompt browser`. -->
# browser

> The headed browser on the Instabox box (Camoufox Firefox, driven by Playwright MCP). Use it for any live web page the agent must open, click, fill in, or watch — logins, dashboards, forms, pages that need JavaScript. Do NOT use it for search or for fetching static pages; that is the web-research skill (`/prompt web-research`).

## What the browser is

- **Camoufox** — a headed (visible) anti-detect Firefox, v158.0.1-beta.41, running on the Instabox machine `muse-browser` (Debian 13, 4 vCPU / 8 GiB). It is headed, not headless: it draws on the box's desktop (DISPLAY=:1, Xvnc on :5901), so the user can watch it live.
- **Playwright MCP server** — `@playwright/mcp` **0.0.78, pinned** — on the same box, port 8931, connected to Camoufox's Playwright server (ws port 9223).

## How you drive it — the CLI, not MCP tools

**You do NOT have browser MCP tools in a gateway session.** Zerostack's ACP mode (the mode the zs-gateway uses) sets `mcp_manager: None` (`src/extras/acp/mod.rs`) — MCP servers from the config are never loaded there. The `[mcp_servers.browser]` entry in the config works in the **Zerostack TUI only**. In a gateway session, drive the browser with the **`browser` CLI (Rust) via bash** (bash is available in ACP mode). The CLI talks to the same Playwright MCP server over HTTP. (Rust binary at /root/pi-bridge/browser; the Python browser.py remains at /root/pi-bridge/browser.py as a fallback.)

```bash
/root/pi-bridge/browser navigate <url>
/root/pi-bridge/browser snapshot
/root/pi-bridge/browser click <ref>
/root/pi-bridge/browser type <ref> <text> [--submit]
/root/pi-bridge/browser eval "<js>"
/root/pi-bridge/browser wait <text>
/root/pi-bridge/browser back
/root/pi-bridge/browser close
/root/pi-bridge/browser status
```

- **Wiring** — the CLI connects to `http://127.0.0.1:8931/mcp`. That is a local relay on the Altare box (`/root/pi-bridge/tailnet-relay.py`, tmux `tailnet-relay`) that forwards through the Tailscale SOCKS5 proxy to the Instabox box (100.105.110.103). Altare runs Tailscale in userspace-networking mode and cannot dial Tailscale IPs directly — never point a tool at the Tailscale IP from Altare; always use the `127.0.0.1:8931` relay (the CLI already does).
- The CLI keeps one MCP session (id in `/tmp/browser-mcp-session.json`) so consecutive calls share one browser/page. If the session expired, the CLI re-initialises automatically — but the page is then blank, so you must **re-navigate and re-snapshot** before acting.

## How to drive a page (the correct sequence)

1. **Navigate** — `browser navigate <url>`. It prints the page title. Always the first step.
2. **Snapshot** — `browser snapshot`. Read the refs (e.g. `textbox [ref=e2]`, `button "Go" [ref=e3]`). This is the page's ground truth; do not guess selectors.
3. **Act, back-to-back** — `browser type <ref> <text>` / `browser click <ref>` using the refs from the snapshot you just took. Do the actions **in quick succession**; after each action that changes the page, **snapshot again** before the next action — refs go stale after a navigation, a re-render, or a pause.
4. **Verify** — snapshot (or `eval "document.title"`) to confirm the result (a confirmation message, the right page, the filed form). Report what the page actually shows, not what you expected.
5. **Close** — `browser close` when done, so the next task starts clean.

Rules:
- One browser tab at a time. Do not open a second page while one is mid-task; the CLI drives a single browser.
- **If a ref is "not found"**: the snapshot is stale or the session expired. Re-navigate to the URL, re-snapshot, then retry with the fresh refs. Do not retry the same stale ref.
- Never fill in a password, OTP, or payment detail unless the user explicitly asked for that exact action on that exact site. Sign-ins go through the user (they can take over the viewer) or through a saved login the user approved.
- Never submit a purchase, a booking, or any irreversible action without the user's explicit yes for that action.
- If a page shows a bot check / CAPTCHA, stop and tell the user — do not try to solve it by scripting around it. The user can take over the viewer and solve it themself.
- If the browser is stopped (the viewer shows nothing, or the CLI errors "browser closed"), do NOT restart it yourself. Tell the user; they can press Start on the viewer, or ask you to restart it.

## The viewer — the user watches and takes over

- **Viewer link:** https://hardeepwalia.pitunnel.dpdns.org/browser — the headed desktop, live, with a red **Stop** and a green **Start** button. Same tunnel login as /pi.
- The user can click and type in the viewer at any time (takeover). When they do, stop driving the browser and let them finish; resume only if they ask.
- **Stop** kills the Camoufox + MCP stack (the browser closes). **Start** restarts it. If the user presses Stop, your next browser tool call will fail — that is expected; wait for them to press Start or to tell you to restart it.

## Browser vs web-research

- **Use the browser** when the task needs a live, interactive page: logging in, a dashboard, a form, a checkout, a page that only renders with JavaScript, or anything the user wants to watch happen.
- **Use web-research** (`/prompt web-research`, `python3 /root/pi-bridge/search.py research "..."`) when the task is finding information: a search, a news check, fetching a static article or docs page. The browser is slower and heavier; never use it as a search engine.
- If a page is a static article or docs, fetch it with web-research instead of opening it in the browser.

## Technical notes (do not change these)

- **Playwright version pin:** `@playwright/mcp` must stay at **0.0.78** (playwright-core 1.62.0-alpha). Version 0.0.86 (playwright-core 1.66 alpha) is rejected by the Camoufox server (Playwright 1.62.0, 428 version mismatch). Never upgrade the MCP server without checking the Camoufox Playwright version first.
- **Camoufox ws endpoint** binds IPv6 `[::1]:9223` only — never normalise it to 127.0.0.1 (ECONNREFUSED). The MCP server is started with the endpoint read from `/data/workspace/camoufox-endpoint.txt` on the Instabox box.
- **The CLI and the MCP config are two paths to the same browser.** `[mcp_servers.browser]` in the Zerostack config is for the **TUI** (where MCP loads). Gateway/ACP sessions use the **`browser` CLI (Rust)** via bash. Do not "fix" the config to make MCP appear in ACP — ACP sets `mcp_manager: None` by design; the CLI is the intended path there.
- **Restarting the stack:** on the Instabox box, `/data/workspace/browser-stack.sh` restarts Camoufox + MCP; `/data/workspace/restart-mcp.sh` restarts only the MCP server. Both kill by port PID — never `pkill -f "playwright-mcp"` from an SSH command line (it matches the invoking shell and kills your own session).
- **Instabox machine:** `muse-browser`, id `m_agq7thurpy4svui43m5r735hri`. It auto-pauses after 30 min idle; `ib.py start` / `resume` wakes it. SSH token expires 2026-12-05 — re-issue with `ib.py ssh-token` before then if still needed.
- **Instabox API key:** managed from the Admin page (`/admin`, "Instabox API key" card) — stored in `/root/pi-bridge/provider-keys.env` as `INSTABOX_API_KEY` (chmod 600). The key is used by Muse's Instabox connector to create/start/stop machines; Zerostack does not call the Instabox API directly.

## Do not

- Do not use the browser to search the web (use web-research).
- Do not drive two pages at once, and do not assume a snapshot ref is still valid after a navigation — re-snapshot.
- Do not restart the browser stack, the relay, or the tunnel to "fix" a stuck page. Snapshot first; if the page is genuinely stuck, tell the user.
- Do not paste any API key, token, or password into a page, a prompt, or a tool call. Keys live in the gateway store / Secure Vault, never in chat or in a browser field unless the user explicitly asked for that exact login.
