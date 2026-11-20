# cf-search — Bing search bridge Worker (search-only)

Runs on the Akamithunsingh77 Cloudflare account (`c60343103ca6c9cdd2acdeff73ae3a72`).
Workers.dev subdomain: `akamithun-search`.
Live URL: https://cf-search.akamithun-search.workers.dev
Deployed 2026-11-19.

## What it does
One endpoint, returning plain text only (no JSON, no format parameter).
One line per result: `N. Title - URL`. This is the lowest-token format
(measured ~23 tokens/result vs ~106 for full JSON).

- `/search` — Bing web / images / videos / news, all Bing filters.

## Response (single format, always)
```
Search: bitcoin (web, 3 results)
1. Bitcoin price today - https://coinmarketcap.com/currencies/bitcoin/
2. BTC USD — Bitcoin Price and Chart — TradingView - https://www.tradingview.com/symbols/BTCUSD/
3. Bitcoin - Open source P2P money - https://bitcoin.org/
```
If the upstream fails, a second line says `Error: upstream_failed` (or
`upstream_503` etc.) and there are no result lines.

## Filters
- `/search`: q (required), vertical (web|images|videos|news), count (1-50,
  default 10), page, offset/first, setlang/lang, cc/region/country, mkt,
  safesearch (off|moderate|strict), time/time_range (day|week|month|year).
  Images add size, color, imgtype, face, license. Videos add duration,
  resolution. News adds sort.

Full documentation of every filter, result counts, pagination, country-wise
filtering and the token research: `llms.txt` and `llm.txt` in this folder.

## How many results
- Web/news (Bing RSS): at most 10 per query. count=50 still gives 10.
- Pagination does NOT work for web/news: Bing ignores the `first` offset
  in every form tested (RSS, HTML, FORM, FPIG, cookies, count=50).
  Page=2 returns the identical results as page=1 (verified 2026-11-19).
  Use different country/language/time filters to get genuinely
  different result sets instead.
- Country-wise filtering WORKS: the same query with setlang=en-IN&cc=IN vs
  setlang=en-US&cc=US gives genuinely different results (verified with
  `cricket` 2026-11-19).
- Images/videos: up to `count`, and pagination works better there.

## Deploy
```text
cd ~/workspace/cf-search-worker
eval $(python3 ~/workspace/custom-connectors/custom.cloudflare-search/bin/mint_cf_search.py)
npx wrangler deploy
```

## Why this exists
Public SearxNG instances rate-limit and bot-block this VM (0/100 and 0/100 in
two load tests). TinyFish via Monid works but is a third-party API with daily
caps. The vault Worker proved Cloudflare's network can reach Bing directly
(30/30 at 15/min, 28/30 with results, ~440ms). This dedicated Worker turns
that into a clean search API for Pi, with every Bing filter.

## What does NOT work from Cloudflare's network
- Google News: a /gnews endpoint was built and then removed 2026-11-19.
  Google News RSS returns 503 to Cloudflare egress IPs (it works from a
  normal VM directly). Bing News via /search vertical=news is the working
  news source.
- DuckDuckGo: tested twice 2026-11-19. First via the vault Worker (lite,
  html and main endpoints all upstream_failed after ~20s each), then via a
  live /ddg endpoint in this Worker (html.duckduckgo.com returned HTTP 202,
  a bot-challenge page, 0 results). The endpoint was removed. GitHub repos
  that scrape DuckDuckGo from Workers exist, but from this Worker's
  Cloudflare IPs DuckDuckGo serves a bot-challenge, not results.
- Google and SearxNG also fail from the Worker. Bing is the only working
  search source here.
