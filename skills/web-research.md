<!-- Installed for Zerostack on the Altare box, 2026-11-20, from Pi's pi-bridge/SKILL.md. -->
<!-- Zerostack has no skills directory: this file IS the skill, as a named prompt. -->
<!-- Load it with `/prompt web-research`, `.web-research <msg>`, or `zerostack --load-prompt web-research`. -->
# web-research

> Best search and fetch system for Muse and Pi — TinyFish free via Monid + cf-search Worker (Bing) combined for maximum URLs, TinyFish free fetch, and X/Bluesky/TikTok/Instagram/Threads/Mastodon via FxEmbed. Use for ANY web research, X search, or fetching pages. Always use the combined `research` command, never a single source.

# Web Research — the only search/fetch system to use

Muse and Pi must ALWAYS use this system for web research. Never use a
single search engine, never curl a search page by hand, never guess.

## The tool

Muse: `python3 ~/workspace/search/web.py`
Zerostack and Pi (both on the Altare box): `python3 /root/pi-bridge/search.py`

## Combined research — default for every research question

```
python3 /root/pi-bridge/search.py research "your question" --pages 2 --fetch-top 5
```

What it does, in one call (code-mode style — 1 tool call, not 6-8):
1. **Web search, two engines, deduped** — TinyFish `/free/search`
   via Monid ($0) + cf-search Worker (Bing, no key). Maximum URLs.
2. **X search** — FxEmbed `/ai/2/ask?q=&pages=2`, merged Markdown pack.
3. **Fetch top 5 URLs** — TinyFish `/free/fetch` ($0, clean Markdown),
   fallback plain curl.

Use `research` for anything that needs real data. Use the single
commands below only when you need just one piece.

## Web search — two engines, combined

```
python3 /root/pi-bridge/search.py search "query" [--purpose "what the results are for"]
python3 /root/pi-bridge/search.py search-cf "query" [--vertical web|images|videos|news] [--count 10] [--cc IN] [--setlang en-IN] [--time week]
python3 /root/pi-bridge/search.py fetch https://example.com [https://...]   # up to 10 URLs
```

### TinyFish (primary, via Monid)

- Free: search 12,000/day, 30/min, 500/hr; fetch 1,000 URLs/day,
  150 URLs/min. Via Monid API `POST /v1/run` provider `tinyfish`,
  endpoints `/free/search` (queryParams: query, purpose, location,
  language, domain_type web|news|research_paper, recency_minutes)
  and `/free/fetch` (body: urls[1-10], format markdown|html|json).
- Paid endpoints `/search` / `/fetch` exist — NEVER use them, free
  ones are $0. Monid balance is $1; free calls cost $0.
- Monid key: from `~/.config/monid/credentials.yaml` (label `main`,
  chmod 600) or `$MONID_API_KEY`. NEVER write the literal key in a
  file, in chat, or in memory. On the Pi sandbox it lives as an env
  export in `/root/.profile`, same as the other API keys.

### cf-search Worker (Bing, no key) — second leg

- `GET /search?q=...` at YOUR Worker URL (see below). Plain text, one
  line per result: `N. Title - URL`. No JSON, no format parameter.
  Health: `GET /health` returns `ok cf-search web images videos news`.
- **The Worker URL is NEVER hardcoded.** It is the user's own Worker,
  deployed from `docs/SEARCH-WORKER.md`. **Muse ASKS the user for
  their Worker URL during setup** and sets `CF_SEARCH_URL` (env export).
  If it is empty, the cf-search leg is skipped and TinyFish + X still work.
- Full manual: `search/cf-search-llms.txt` in this repo (also live at
  `<your-worker-url>/llms.txt` once deployed).

**Filters (all Bing filters):**

Common (every vertical):
- `q` (required, 1-500 chars): the search query.
- `vertical`: web | images | videos | news. Default web.
- `count`: 1-50. Default 10. Web/news max 10 per query (Bing RSS
  gives at most 10), so count above 10 has no effect there.
- `page`: 1, 2, 3... Default 1. Sets offset = (page-1) x count.
  See Pagination below — for web/news it does NOT return new results.
- `offset` or `first`: exact result offset. Default 0. `page` wins.
- `setlang` or `lang`: UI language, e.g. en-US, en-IN, hi-IN, es-ES,
  de-DE, fr-FR, pt-BR, ja-JP, zh-CN. Default en-US.
- `cc` or `region` or `country`: country code, e.g. IN, US, GB, DE, JP.
  Default: derived from setlang. Change it for country-specific results.
- `mkt`: market, e.g. en-IN, en-US. Default: same as setlang.
- `safesearch`: off | moderate | strict. Default moderate.
- `time` or `time_range`: day | week | month | year. Recency filter.

Images only (vertical=images):
- `size`: small | medium | large | wallpaper.
- `color`: color | bw (black and white).
- `imgtype`: photo | clipart | line | animated.
- `face`: face | portrait.
- `license`: free | free-share | free-modify | public.

Videos only (vertical=videos):
- `duration`: short | medium | long.
- `resolution`: hd | sd.

News only (vertical=news, Bing News):
- `sort`: date | relevance.

**Examples:**
- Web, 10 results, English India: `/search?q=electric+cars&count=10&setlang=en-IN`
- Web, past week, strict: `/search?q=cricket&time=week&safesearch=strict`
- Images, large black-and-white, 20: `/search?q=mountain&vertical=images&size=large&color=bw&count=20`
- Videos, short, HD: `/search?q=yoga&vertical=videos&duration=short&resolution=hd&count=10`
- News, sorted by date: `/search?q=spacex&vertical=news&sort=date&count=10`
- Country-wise (different results): `/search?q=cricket&count=5&setlang=en-IN&cc=IN` vs `...&setlang=en-US&cc=US`
- POST with JSON body: `POST /search {"q":"quantum computing","count":5}`

**Limits (verified 2026-11-19):**
- Web/news max 10 per query; count=50 still gives 10.
- **Pagination does NOT work for web/news.** Bing ignores `first=`
  in every form tested (RSS, HTML, FORM=PERE, FPIG session token,
  cookies, cookies+FPIG+FORM, count=50). Page 2 returns the identical
  results as page 1. To get more results, vary the query, country,
  language, or time filter instead.
- **Country-wise filtering DOES work.** Same query with
  `setlang=en-IN&cc=IN` vs `setlang=en-US&cc=US` gives genuinely
  different results (verified with `cricket`).
- Title + URL only is the lowest-token format (~23 tokens/result vs
  ~106 for full JSON). Snippets are dropped on purpose — fetch the page
  for real content.
- Google News and DuckDuckGo do NOT work from Cloudflare's network
  (503 / 202 bot-challenge). Bing is the only working source there.

### DuckDuckGo — REMOVED (2026-11-19)

DuckDuckGo was the third leg and is now REMOVED from web.py, the skill,
and the repo. It returns nothing from the Pi sandbox (bot-challenge,
`[]`) and from Cloudflare's network (HTTP 202). It only works from a
normal VM. Do not add it back. If a search needs a third source, use
a different query/country/time filter on the Bing Worker instead.

## X / social search — FxEmbed, no key

Base: `https://fixtweet.twapiworker.workers.dev`
(clone: `https://fixtweet-2.twapiworker.workers.dev` — fail over on flake)
**User-Agent header is MANDATORY** on every `/api` and `/ai` call
(e.g. `Muse/1.0`); missing UA = HTTP 401. Full manual (229 lines):
`search/fxembed-llms.txt` in this repo (also live at
`https://fixtweet.twapiworker.workers.dev/llms.txt`).

Networks: X/Twitter, Bluesky, TikTok, Instagram, Threads, any
Mastodon/ActivityPub instance. NOT supported: Reddit, Facebook,
YouTube, TikTok comments/likers/search.

```
python3 /root/pi-bridge/search.py xsearch "Pi coding agent" [--pages 3]   # /ai/2/ask, merged pack
python3 /root/pi-bridge/search.py xpost https://x.com/jack/status/20  # any post URL, or just the id
python3 /root/pi-bridge/search.py xprofile <handle>                        # X profile
python3 /root/pi-bridge/search.py xtrends                                   # X trends
```

### The 3 core recipes

1. **Single post:** `GET /api/2/status/{id}` — numeric id from
   `x.com/.../status/{id}`. Or Markdown: `/ai/2/status/{id}`.
2. **Deep keyword search:** `GET /api/2/search?q=<terms>&feed=latest&count=50&fast=1`,
   then follow `cursor.bottom` from each response (pass it back as
   `&cursor=<value>`) and repeat until the walk ends. The continuation
   token is ALWAYS `cursor.bottom` — never any other key.
3. **One-shot pack:** `GET /ai/2/ask?q=<terms>&pages=N` (N max 8) —
   the worker walks N search pages server-side and returns ONE merged
   Markdown pack. The recommended deep route.

### X endpoints (JSON /api/2/ + Markdown /ai/2/ twins)

- Single post: `/api/2/status/{id}` | `/ai/2/status/{id}`
- Thread (author chain): `/api/2/thread/{id}` | `/ai/2/thread/{id}`
- Conversation (post + ancestors + replies): `/api/2/conversation/{id}?ranking_mode=likes|recency`
  — effectively page 1 only (guest path dead, fallback serves first page).
- Profile: `/api/2/profile/{handle}` (handle OR `id:123`) | `/ai/2/profile/{handle}`
- Profile timeline: `/api/2/profile/{handle}/{statuses|media|articles|followers|following}?count=1-100&cursor=&with_replies=1&groupthreads=1&since=&lang=`
- Profile search: `/api/2/profile/{handle}/search?q=`
- Quotes/reposts: `/api/2/status/{id}/{quotes|reposts}?count=&cursor=`
- Search JSON: `/api/2/search?q=&feed=latest|top|media&count=50&fast=1`
- Trends/typeahead: `/api/2/trends` | `/api/2/typeahead?q=`
- Universal post/profile: `GET /ai/2/post?url=<any supported URL>` —
  X, Bluesky, TikTok, Instagram, Threads, Mastodon, any of them.

### Search walk rules

- **500 mid-walk = retry the SAME request after ~30s.** The walk
  resumes at the failed window, nothing is skipped.
- **404 = genuinely exhausted.** The walk is over.
- `fast=1` returns RSS-level stubs (text/author/time only, no stats
  or media) without hydration.
- `feed=media` appends `filter:media`. `feed=top` ranks a hydrated
  pool by engagement.
- X operators pass through: `from:`, `filter:`, `lang:`, `since:`,
  `until:`, `within_time:`, `min_faves:`.
- Deep walks return ~500 unique posts per common keyword.
- `?lang=` is accepted but translation is a NO-OP — text comes back
  in the original language, unmarked. Do not rely on it.
- Single-post answers can flake per request — retry once or try the
  other base (fixtweet-2).
- Poll timelines with `?since=<unix-sec>`: 204 = nothing new.

### Bluesky / TikTok / Instagram / Threads / Mastodon

- Bluesky: `/ai/2/bsky/profile/{handle}` | `/ai/2/bsky/profile/{handle}/statuses` |
  `/ai/2/bsky/status/{handle}/{rkey}` | `/ai/2/bsky/thread/{handle}/{rkey}` |
  `/ai/2/bsky/search?q=`. Rkey recipe: `statuses?count=1` → copy `.../post/{rkey}`
  from the `[open]` link → use it in status/thread paths.
- TikTok: `/ai/2/tiktok/status/{numeric-video-id}` | `/ai/2/tiktok/profile/{handle}`.
  Only numeric video ids work. No comments/likers/search. Unsigned calls
  are IP-limited (a handful, then a long 429 cooldown) — space out calls.
- Instagram: `/ai/2/instagram/profile/{username}`. Logged-out Instagram
  is heavily blocked upstream — 500s are normal.
- Threads: `/ai/2/threads/profile/{username}`.
- Mastodon: `/ai/2/mastodon/{instance-domain}/profile/{handle}`.
  Status search returns empty (needs auth); user search works.
- Universal: `/ai/2/post?url=<any post/profile URL>` handles all of them.

### RSS feeds (polling)

- `/twitter/{handle}/feed.xml` (RSS) | `/twitter/{handle}/feed.atom.xml` (Atom).
  Poll with `?since=<unix-sec>`; 204 = nothing new.
- Media feeds (`media.xml`) return a bare HTTP 500 — known issue, use
  the timeline feeds.

## Rules

1. **Always `research` first** for a research question. One combined
   call beats many single-source calls, in tokens and in coverage.
2. **Maximum URLs**: merge TinyFish + cf-search Worker + X links,
   dedupe, then fetch the top 5-10. Never stop at the first 3 results.
3. **Never use paid Monid/TinyFish endpoints** (`/search`, `/fetch`
   without `/free/`). Free tiers cover everything here.
4. **Never paste API keys** in chat, files, or model output. Keys live
   in their own credential stores only.
5. **Cite sources**: every claim from research gets its URL.
6. **Bing pagination does not work** — do not call `page=2` expecting
   new results. Vary the query, country, language, or time filter instead.
7. **DuckDuckGo is removed** — do not add it back. It does not work
   from Pi or Cloudflare.


## Zerostack notes

Run the tool through the `bash` tool. There is no separate search tool here, and
Zerostack's built-in Exa search is disabled (`enable-exa-mcp = false`), so this
script is the only search path. `research` prints Markdown to stdout; quote the
query so the shell does not split it. The Monid key and `CF_SEARCH_URL` come from
the box environment (`/root/.profile`) — never pass or print a key.
