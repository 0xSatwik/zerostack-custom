# Search & Fetch System — Muse + Pi (standing, 2026-11-17)

Muse and Pi ALWAYS use this system. No other search/fetch path.

## Stack (in order, combined — maximum URLs + data)
1. **TinyFish free search** via Monid API — `tinyfish /free/search`
   ($0, 12,000/day, 30/min). Primary web search.
2. **cf-search Worker (Bing)** — no key, free. Second web leg (added
   2026-11-19). `https://cf-search.akamithun-search.workers.dev/search`
   (override with `CF_SEARCH_URL`). Plain text, one line per result.
   Country-wise filtering works; pagination does NOT work for web/news.
   Google News and DuckDuckGo do NOT work from Cloudflare's network.
   Full docs: `docs/SEARCH-WORKER.md`.
3. **TinyFish free fetch** via Monid — `tinyfish /free/fetch`
   ($0, 1,000 URLs/day, 150/min). Primary page fetch, clean Markdown.
   Fallback: plain curl.
4. **X / social via FxEmbed** — no key, UA mandatory.
   Base `https://fixtweet.twapiworker.workers.dev`
   (clone `fixtweet-2.twapiworker.workers.dev`).
   X search: `/ai/2/ask?q=&pages=N` (max 8). Also Bluesky, TikTok,
   Instagram, Threads, Mastodon via `/ai/2/post?url=`.
   Full manual: `https://fixtweet.twapiworker.workers.dev/llms.txt`

DuckDuckGo was REMOVED 2026-11-19 — it returns nothing from the Pi
sandbox and from Cloudflare's network. Do not add it back.

## Tool
- Muse: `python3 ~/workspace/search/web.py`
- Pi: `python3 /root/pi-bridge/search.py` (same file, on the sandbox)
- Skill: `~/workspace/custom-skills/web-research/SKILL.md`
  (Pi copy: `/root/pi-bridge/SKILL.md` + `/root/work/AGENTS.md`)

## Commands
```
python3 web.py research "question" --pages 2 --fetch-top 5   # DEFAULT, combined
python3 web.py search "query" [--purpose "..."]
python3 web.py search-cf "query" [--vertical web|images|videos|news] [--count 10] [--cc IN] [--setlang en-IN] [--time week]
python3 web.py fetch <url> [url...]
python3 web.py xsearch "query" [--pages 3]
python3 web.py xpost <x-url-or-id>
python3 web.py xprofile <handle>
python3 web.py xtrends
```

## Keys (never literal in any file/chat/memory)
- Monid/TinyFish key: Muse — `~/.config/monid/credentials.yaml`
  (label `main`, chmod 600) or `$MONID_API_KEY`. Monid balance $1,
  free calls cost $0. Pi — env export `$MONID_API_KEY` in
  `/root/.profile` on the sandbox, same as the other model API keys.
- FxEmbed: no key at all. UA header mandatory.

## Rules
- Always `research` for a research question — one combined call.
- Merge + dedupe URLs from TinyFish, cf-search Worker and X; fetch top 5-10.
- Never use paid `/search` / `/fetch` (non-free) Monid endpoints.
- Bing pagination does not work — do not call `page=2` expecting new results.
- DuckDuckGo is removed — do not add it back.
- Cite every source URL.
