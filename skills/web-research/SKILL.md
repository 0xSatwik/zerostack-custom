---
name: web-research
description: Best search and fetch system for Muse and Zerostack — backend selector in /admin (Monid / Bing / Apify), TinyFish free via Monid + cf-search Worker (Bing), your own Apify actors (34-source web search, Reddit), TinyFish fetch with Camoufox browser fallback, and X/Bluesky/TikTok/Instagram/Threads/Mastodon via the fixtweet worker. Use for ANY web research, X search, Reddit search, or fetching pages. Always use the combined `research` command, never a single source.
---

# Web Research — the only search/fetch system to use

Muse and Zerostack must ALWAYS use this system for web research. Never use a
single search engine, never curl a search page by hand, never guess.

## The tool

Muse: `python3 ~/workspace/search/web.py`
Zerostack (on Altare sandbox): `python3 /root/pi-bridge/search.py` (same file)

## Worker URLs — set by the user in /admin, never hardcoded

Both search workers are the user's own deployments. The user sets their
URLs in the **/admin page → Keys & secrets → Web search — worker URLs**.
They are stored in `search.json` next to `web.py` (chmod 600) — no URL
is ever written literally in code, in a skill, or in the repo.

- **fixtweet worker URL** (X / social) + optional second URL (failover,
  e.g. the `-2` clone). If empty, the X leg is skipped.
- **cf-search worker URL** (Bing). If empty, the Bing leg is skipped.
- **Leg toggles**: TinyFish, Bing, X — on/off each.
- **Search backend**: set in /admin → Web search settings — `Monid`, `Bing combination`, or `Apify`. One backend at a time; default Monid. Stored in `search.json` as `backend`.
- **Monid key** (TinyFish): same /admin page, write-only, stored in
  Monid's own credential store (`~/.config/monid/credentials.yaml`).
- **Apify API keys** (only used when the backend is Apify): multiple
  keys, same card pattern as Monid/Instabox. Under each key, add
  multiple actors by **name + actor ID** (no IDs are ever pre-filled
  or hardcoded anywhere). In /admin you pick a key, then pick the
  actor by name. The token lives in the vault and reaches `web.py`
  at runtime; the agent never sees it.

`web.py` reads `search.json` on every run (env vars `FXEMBED_WORKER_URL`,
`FXEMBED_WORKER_URL_2`, `CF_SEARCH_URL` override it). If a URL is not
set, that leg prints a skip note to stderr and the other legs still work.

Full manuals: the fixtweet worker's `/llms.txt` and the cf-search
worker's `/llm.txt` (fetch either URL + `/llms.txt` or `/llm.txt`).

## Combined research — default for every research question

```
python3 web.py research "your question" --pages 2 --fetch-top 5
```

What it does, in one call (code-mode style — 1 tool call, not 6-8):
1. **Web search, per the selected backend** — Monid: TinyFish `/free/search`
   via Monid ($0) + cf-search Worker (Bing, no key), deduped. Bing
   combination: the same two engines. Apify: your Web Search API actor
   (34 sources, SearXNG meta-search) via run-sync.
2. **Reddit search** (when enabled) — your Reddit Scraper actor via
   run-sync. Reddit is its own leg; the X worker does not cover it.
3. **X search** — fixtweet worker `/ai/2/ask?q=&pages=2`, merged Markdown pack.
4. **Fetch top 5 URLs** — TinyFish `/free/fetch` ($0, clean Markdown).
   If Monid fetch is blocked or fails, fetch via the Camoufox browser
   on Instabox (navigate + read rendered text). Never a plain curl
   scrape as first choice.

Use `research` for anything that needs real data. Use the single
commands below only when you need just one piece.

## Web search — two engines, combined

```
python3 web.py search "query" [--purpose "what the results are for"]
python3 web.py search-cf "query" [--vertical web|images|videos|news] [--count 10] [--cc IN] [--setlang en-IN] [--time week]
python3 web.py fetch https://example.com [https://...]   # up to 10 URLs
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
  file, in chat, or in memory. On the Altare box it lives in the same
  file (`/root/.config/monid/credentials.yaml`); the /admin page's
  Monid card writes it there.

### cf-search Worker (Bing, no key) — second leg

- `GET /search?q=...` at the user's Worker URL (set in /admin, see
  above). Plain text, one line per result: `N. Title - URL`. No JSON,
  no format parameter. Health: `GET /health` returns
  `ok cf-search web images videos news`.
- Full manual: `search/cf-search-llms.txt` in the repo (also live at
  `<your-worker-url>/llm.txt` once deployed).

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

## Apify — your own actors (backend: Apify)

Only active when the search backend is Apify (see above). Keys and
actor IDs are set by the user in /admin — never hardcoded here, in
`web.py`, or in the repo. The token comes from the vault at runtime;
the agent never sees it.

### Running an actor (the only calls `web.py` uses)

- Run synchronously, get dataset items back directly:
  `POST https://api.apify.com/v2/actors/<username>~<slug>/run-sync-get-dataset-items?token=<TOKEN>`
  Body = input JSON, `Content-Type: application/json`. Use this — it
  waits and returns the items, no polling.
- Async (only if a run is long): `POST
  https://api.apify.com/v2/actors/<username>~<slug>/runs?token=<TOKEN>`,
  then `GET https://api.apify.com/v2/datasets/{datasetId}/items`.
- In API paths the actor is `<username>~<slug>` (tilde). In the Store
  it is `<username>/<slug>` (slash).
- Actor IDs are public: `GET https://api.apify.com/v2/acts/<username>~<slug>`
  (no token) returns the actor object with its `id`.

### Web Search API actor — 34 sources

SearXNG meta-search over Google, Bing, Brave, DuckDuckGo, Yandex,
Yahoo, Baidu, Sogou, Mojeek, Qwant, Startpage + Wikipedia, GitHub,
arXiv, Stack Overflow, OpenStreetMap, Reddit, YouTube, Wolfram Alpha.
No browser/proxy by default. Dedupes by normalized URL across the
whole run; scores title +2, content +1, URL/host +0.5, plus a
freshness bonus. Up to 1,000 queries per run, 1–100 results per query,
up to 10,000 results per run.

Input (all optional; `queries` is the effective main input):
- `queries`: array of strings, 1–1,000 questions. Batch many in one run.
- `mode`: `search` (default) | `images` | `videos` | `news` | `maps` |
  `science` | `files` | `read_page` (fetch full text of `urls`).
- `engines`: array, empty = all sources. E.g.
  `["wikipedia","github","arxiv","bing","duckduckgo"]`.
- `category`: `general` (default) | `files` | `images` | `it` | `map` |
  `music` | `news` | `science` | `social media` | `videos`.
- `language`: default `en`. `region`: default `us`. `safeSearch`: 0 off,
  1 moderate (default), 2 strict. `timeRange`: `day` | `week` | `month` |
  `year` (empty = none).
- `maxResults`: default 5 (1–100 per query). `maxConcurrency`: default 8.
  `maxChars`: default 4000 (max text per result). `chunkSize`: 0 = no chunking.
- `groupByQuery`: false = one dataset item per result; true = one item
  per query with a `results` array.
- `includeRawContent`: false — fetch full text of every result page
  (billed per page read). `pageFormat`: `text` | `markdown` | `html`.
- `urls`: array of URLs, used with `mode: read_page`.
  `agentContext`: false — adds clean LLM-ready text per result (billed
  separately). `strictEngines`: false (a failing engine does not fail
  the run). `debug`: false. `proxyConfiguration`:
  `{"useApifyProxy": false}` by default.

Example: `{"queries":["What is the capital of France?"],"maxResults":5}`.
With full text: add `"includeRawContent":true`. Read a page:
`{"mode":"read_page","urls":["https://en.wikipedia.org/wiki/Paris"],"maxChars":4000}`.

Output row (default): `query`, `engine`, `position`, `title`, `url`,
`content` (snippet, capped 400 chars), `score`, `publishedDate`. With
raw content: + `rawContent`, `rawContentLength`, `fetchedAt`,
`htmlLength`, `extractionMode`. With groupByQuery: `query`, `count`,
`results[]`.

Pricing (pay per event; the owner pays only platform compute for their
own runs from the $5/month free credits — the per-event prices are what
other users pay): search results $1.50/1,000; read pages $1.00/1,000;
tokens $0.15/1,000; agentContext $0.50/1,000; images/videos $1.00/1,000.
Caps: 5,000 results/day, 10,000/run.

### Reddit Scraper actor

Scrapes Reddit's public JSON endpoints — no login, cookies, proxy or
browser. One subreddit per run, one username per run for user modes.
Only public subreddits/posts. NSFW excluded by default. Pagination is
automatic up to `resultsLimit`.

Input (all optional):
- `mode`: `subredditPosts` (default) | `postDetails` | `duplicates` |
  `userPosts` | `userComments` | `userOverview` | `subredditDetails` |
  `subredditAutocomplete` | `directory`.
- `subredditName`: e.g. `technology` (no `r/`). `postUrl`: full post URL
  (postDetails). `userName`: no `u/` (user modes). `query`: text for
  subredditAutocomplete.
- `directory`: `popular` (default) | `all` | `top` | `new` | `rising` |
  `random` (mode `directory`).
- `feedSource`: `front` (default) | `hot` | `new` | `top` | `rising` |
  `controversial`. `sort`: `hot` (default) | `new` | `top` | `rising` |
  `controversial`. `time`: `all` (default) | `hour` | `day` | `week` |
  `month` | `year` (for top/controversial).
- `resultsLimit`: default 100 (1–1,000). `includeNsfw`: false.
  `includeComments`: false. `includeRules`: false (subredditDetails).
  `agentContext`: false.

Output post row: `subreddit`, `author`, `id`, `title`, `selftext`,
`url`, `permalink`, `domain`, `score`, `upvoteRatio`, `numComments`,
`createdUtc`, `created`, `isVideo`, `isSelf`, `over18`, `spoiler`,
`stickied`, `locked`, `flairText`, `thumbnail`, `media`, `preview`,
`awards`, `body`, `selftextHtml`, `source`, `agentContext` (when
enabled). User rows: `username`, `name`, `id`, `description`,
`createdUtc`, `karma`, `commentKarma`, `linkKarma`, `isEmployee`,
`isGold`, `isMod`, `verified`, `iconImg`, `bannerImg`, `subreddit`
(profile subreddit object). Subreddit rows: `id`, `name`,
`displayName`, `title`, `description`, `subscribers`,
`activeUserCount`, `createdUtc`, `over18`, `lang`, `rules` (when
includeRules).

Pricing: results $0.90/1,000. Caps: 5,000 results/day, 10,000/run,
1,000 per run.

### Commands

```
python3 web.py research "question"            # uses the selected backend + Reddit leg if enabled
python3 web.py reddit "technology" [--sort hot|new|top] [--time week] [--limit 20]   # Reddit Scraper actor
python3 web.py reddit-post <post-url> [--comments]   # single post details
```

## Fetch — always Monid, Camoufox fallback

Fetch always uses TinyFish `/free/fetch` ($0, clean Markdown). If
Monid fetch is blocked or fails, fall back to the Camoufox browser on
Instabox (port 9377): navigate to the URL and read the rendered text
(use the `browser` Rust CLI or the camofox-browser API). The browser
renders JavaScript, so it also covers pages plain fetchers miss. Never
use a plain curl scrape as the first choice, and never use a paid
fetch endpoint.

### DuckDuckGo — REMOVED (2026-11-19)

DuckDuckGo was the third leg and is now REMOVED from web.py, the skill,
and the repo. It returns nothing from the Altare sandbox (bot-challenge,
`[]`) and from Cloudflare's network (HTTP 202). It only works from a
normal VM. Do not add it back. If a search needs a third source, use
a different query/country/time filter on the Bing Worker instead.

## X / social search — fixtweet worker, no key

The worker URL is set by the user in /admin (see "Worker URLs" above)
— it is NEVER hardcoded in code or in this skill. Two bases exist
(the worker and its identical `-2` clone) — set both for failover.
**User-Agent header is MANDATORY** on every `/api` and `/ai` call
(e.g. `Muse/1.0`); missing UA = HTTP 401. Full manual: the worker's
`/llms.txt` (fetch `<worker-url>/llms.txt`).

Networks: X/Twitter, Bluesky, TikTok, Instagram, Threads, any
Mastodon/ActivityPub instance. NOT supported: Reddit, Facebook,
YouTube, TikTok comments/likers/search.

```
python3 web.py xsearch "Zerostack agent" [--pages 3]   # /ai/2/ask, merged pack
python3 web.py xpost https://x.com/jack/status/20  # any post URL, or just the id
python3 web.py xprofile <handle>                        # X profile
python3 web.py xtrends                                   # X trends
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
   from the sandbox or Cloudflare.
8. **Backend selector**: use the backend set in /admin (`search.json`
   `backend`). Monid = TinyFish + Bing; Apify = your Apify actors.
   Never mix backends in one call unless the setting says so.
9. **Apify**: actor IDs and keys are never hardcoded — they come from
   /admin (key → actor name). Use run-sync for results. The agent
   never sees the token.
10. **Fetch is Monid, fallback Camoufox** — if Monid fetch is blocked,
    use the Camoufox browser, not a curl scrape.
11. **Reddit is its own leg** — use the Reddit Scraper actor for
    Reddit; the X worker does not cover Reddit.
