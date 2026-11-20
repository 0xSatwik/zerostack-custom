// cf-search — Bing search bridge Worker (search-only).
// Runs on the Akamithunsingh77 Cloudflare account. No keys, no vault, no auth.
// Endpoints: GET/POST /search -> plain text, one line per result.
//            GET /llm.txt (or /llms.txt) -> this full manual.
//            GET /health -> ok cf-search web images videos news
// Single format, always: "N. Title - URL". No JSON, no format parameter.
// This is the lowest-token format (measured ~23 tokens/result vs ~106 for
// full JSON, ~63 for text with snippets). Snippets are dropped on purpose:
// the title + URL is enough for an agent to pick which page to fetch, and
// fetching the page gives the real content anyway.
//
// Search filters (query params on GET, or JSON body on POST):
//   q            (required) search query
//   vertical     web | images | videos | news        (default web)
//   count        1..50                              (default 10)
//   page         page number 1,2,3... (default 1). Sets offset = (page-1)*count.
//                The easy way to paginate: page=1 gives results 1-10,
//                page=2 gives 11-20, page=3 gives 21-30, and so on.
//   offset|first pagination offset (default 0). Use page instead when you
//                just want the next page; offset is for exact control.
//   setlang|lang UI language e.g. en-US en-IN hi-IN es-ES de-DE (default en-US)
//   cc|region|country country code e.g. IN US GB  (default from setlang)
//                Change the country to get different, region-specific results.
//   mkt          market e.g. en-IN                  (default = setlang)
//   safesearch   off | moderate | strict            (default moderate)
//   time|time_range day | week | month | year      (recency filter)
//   -- images only --
//   size         small | medium | large | wallpaper
//   color        color | bw
//   imgtype      photo | clipart | line | animated
//   face         face | portrait
//   license      any | free | free-share | free-modify | public
//   -- videos only --
//   duration     short | medium | long
//   resolution   hd | sd
//   -- news only --
//   sort         date | relevance
//
// Response (always plain text):
//   Search: bitcoin (web, 3 results)
//   1. Bitcoin price today - https://coinmarketcap.com/currencies/bitcoin/
//   2. BTC USD — Bitcoin Price and Chart — TradingView - https://www.tradingview.com/symbols/BTCUSD/
//   3. Bitcoin - Open source P2P money - https://bitcoin.org/

const UA = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36';

const TIME_QFT = {
  day: '+filterui:age-lt1440',
  week: '+filterui:age-lt10080',
  month: '+filterui:age-lt43200',
  year: '+filterui:age-lt525600',
};
const IMG_SIZE = { small: 'imagesize-small', medium: 'imagesize-medium', large: 'imagesize-large', wallpaper: 'imagesize-wallpaper' };
const IMG_COLOR = { color: 'color-color', bw: 'color-bw' };
const IMG_TYPE = { photo: 'image-type-photo', clipart: 'image-type-clipart', line: 'image-type-line', animated: 'image-type-animatedgif' };
const IMG_FACE = { face: 'face-face', portrait: 'face-portrait' };
const IMG_LICENSE = { free: 'license-L1', 'free-share': 'license-L2', 'free-modify': 'license-L3', public: 'license-L4' };
const VID_DURATION = { short: 'duration-short', medium: 'duration-medium', long: 'duration-long' };
const VID_RES = { hd: 'resolution-hd', sd: 'resolution-sd' };

function xmlUnescape(s) {
  return s.replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&apos;/g, "'");
}
function stripTags(s) {
  return xmlUnescape(s.replace(/<[^>]+>/g, '')).replace(/\s+/g, ' ').trim();
}

// Parse Bing RSS (<item> blocks) into results.
function parseRss(xml) {
  const out = [];
  const items = xml.match(/<item>[\s\S]*?<\/item>/g) || [];
  for (const it of items) {
    const title = (it.match(/<title>([\s\S]*?)<\/title>/) || [])[1] || '';
    const link = (it.match(/<link>([\s\S]*?)<\/link>/) || [])[1] || '';
    if (!link) continue;
    out.push({ title: stripTags(title), url: xmlUnescape(link.trim()) });
  }
  return out;
}

// Parse Bing images HTML. Bing embeds results as JSON with "murl" (media URL)
// and "t" (title), but the field order varies, so extract each field
// independently around each murl instead of one strict pattern.
function parseImages(html) {
  const out = [];
  const seen = new Set();
  const re = /"murl":"(https?:[^"]+)"/g;
  let m;
  while ((m = re.exec(html)) !== null && out.length < 60) {
    const url = m[1].replace(/\\u002f/g, '/');
    if (seen.has(url)) continue;
    seen.add(url);
    const window = html.slice(m.index, m.index + 1200);
    const title = (window.match(/"t":"((?:[^"\\]|\\.)*)"/) || [])[1] || '';
    out.push({ title: title.replace(/\\"/g, '"').replace(/\\u0026/g, '&').trim() || 'Image result', url });
  }
  // Fallback: plain <img> tags with alt text inside the results area.
  if (out.length === 0) {
    const re2 = /<img[^>]+src="(https?:[^"]+)"[^>]*alt="([^"]{3,120})"/g;
    while ((m = re2.exec(html)) !== null && out.length < 60) {
      const url = m[1];
      if (seen.has(url)) continue;
      seen.add(url);
      out.push({ title: m[2].trim(), url });
    }
  }
  return out;
}

// Parse Bing videos HTML: look for video result links/titles.
function parseVideos(html) {
  const out = [];
  const seen = new Set();
  const re = /"surl":"(https?:[^"]+)"[^}]{0,500}?"t":"((?:[^"\\]|\\.)*)"/g;
  let m;
  while ((m = re.exec(html)) !== null && out.length < 60) {
    const url = m[1].replace(/\\u002f/g, '/');
    const title = m[2].replace(/\\"/g, '"').replace(/\\u0026/g, '&');
    if (seen.has(url) || !title.trim()) continue;
    seen.add(url);
    out.push({ title: title.trim(), url });
  }
  // Fallback: anchors to known video hosts inside the results.
  if (out.length === 0) {
    const re2 = /href="(https?:\/\/(?:www\.)?(?:youtube\.com\/watch\?[^"]+|youtu\.be\/[^"]+|vimeo\.com\/\d+|dailymotion\.com\/video\/[^"]+))"[^>]*>([^<]{5,120})</g;
    while ((m = re2.exec(html)) !== null && out.length < 60) {
      const url = m[1];
      if (seen.has(url)) continue;
      seen.add(url);
      out.push({ title: stripTags(m[2]), url });
    }
  }
  return out;
}

// Parse Bing HTML web/news search results.
// Results are <h2><a href="URL">Title</a></h2> inside each <li class="b_algo">
// block (a site-icon/cite block comes before the h2, so the h2 is NOT right
// after the <li>). Unlike Bing RSS, the HTML search DOES respect the first=
// offset, so it is the only way to paginate web/news to page 2+. The href is
// a Bing redirect (bing.com/ck/a?...&u=<base64>); decode the u param to the
// real destination.
function parseBingHtml(html) {
  const out = [];
  const seen = new Set();
  const re = /<h2><a[^>]+href="([^"]+)"[^>]*>([\s\S]*?)<\/a><\/h2>/g;
  let m;
  while ((m = re.exec(html)) !== null && out.length < 60) {
    // Unescape &amp; first: the redirect href uses &amp;u=, and the u param
    // regex needs a real & to find it.
    let href = xmlUnescape(m[1]);
    const u = href.match(/[?&]u=([A-Za-z0-9+/=_-]+)/);
    if (u) {
      try {
        // Bing prefixes the base64 with "a1" (a version marker); strip it,
        // then restore URL-safe chars and padding before decoding.
        let b64 = u[1].replace(/^a1/, '').replace(/-/g, '+').replace(/_/g, '/');
        while (b64.length % 4) b64 += '=';
        href = atob(b64);
      } catch { /* keep raw href */ }
    }
    if (!/^https?:\/\//i.test(href)) continue;
    // Skip Bing's own internal links.
    if (/^https?:\/\/(www\.)?bing\.com\//i.test(href)) continue;
    if (seen.has(href)) continue;
    seen.add(href);
    const title = stripTags(m[2]);
    if (!title) continue;
    out.push({ title, url: href });
  }
  return out;
}

function buildQft(p) {
  const parts = [];
  if (p.time && TIME_QFT[p.time]) parts.push(TIME_QFT[p.time]);
  if (p.vertical === 'images') {
    if (p.size && IMG_SIZE[p.size]) parts.push('+filterui:' + IMG_SIZE[p.size]);
    if (p.color && IMG_COLOR[p.color]) parts.push('+filterui:' + IMG_COLOR[p.color]);
    if (p.imgtype && IMG_TYPE[p.imgtype]) parts.push('+filterui:' + IMG_TYPE[p.imgtype]);
    if (p.face && IMG_FACE[p.face]) parts.push('+filterui:' + IMG_FACE[p.face]);
    if (p.license && IMG_LICENSE[p.license]) parts.push('+filterui:' + IMG_LICENSE[p.license]);
  }
  if (p.vertical === 'videos') {
    if (p.duration && VID_DURATION[p.duration]) parts.push('+filterui:' + VID_DURATION[p.duration]);
    if (p.resolution && VID_RES[p.resolution]) parts.push('+filterui:' + VID_RES[p.resolution]);
  }
  return parts.join('');
}

async function bingFetch(bingUrl) {
  const res = await fetch(bingUrl, {
    headers: { 'User-Agent': UA, 'Accept-Language': 'en-US,en;q=0.9', 'Accept': 'text/html,application/rss+xml,application/xml;q=0.9,*/*;q=0.8' },
    redirect: 'manual',
    signal: AbortSignal.timeout(20000),
  });
  const text = await res.text();
  return { status: res.status, text };
}

const LLMS_TXT = "# cf-search \u2014 llms.txt\n\nA search-only Cloudflare Worker that returns Bing web/image/video/news\nresults as clean plain text. Built for AI agents (Pi, Muse) that need\nfast, free, filterable search without an API key.\n\n- Live URL: https://cf-search.akamithun-search.workers.dev\n- Health: GET /health returns one line: ok cf-search web images videos news\n- Docs: GET /llms.txt (this file) or GET /llm.txt (identical).\n- No auth, no key, no sign-in. Free tier Worker. Source: cf-search-worker/\n  (src/index.js). One endpoint: /search (Bing). Accepts GET query params\n  or POST with a JSON body.\n\nTHIS FILE IS THE MANUAL. An AI agent that reads this file knows\neverything needed to use this Worker. No other doc is required.\n\n================================================================\nFOR AI AGENTS - START HERE\n================================================================\n\nTHE 3 CORE RECIPES:\n  1. Web search: GET /search?q=<terms>&count=10\n     Returns plain text, one line per result: \"N. Title - URL\".\n  2. Filtered search: GET /search?q=<terms>&vertical=news&time=week&setlang=en-IN&cc=IN\n     Every Bing filter works \u2014 vertical, count, language, country,\n     market, safe search, time range, image/video/news filters.\n  3. Country-wise search: run the SAME query twice with different\n     setlang+cc (e.g. en-IN/IN vs en-US/US) to get genuinely different,\n     region-specific result sets.\n\nNo User-Agent header is required. No API key. CORS is open\n(Access-Control-Allow-Origin: *), so browser and agent clients can\ncall it directly. Results are not cached (Cache-Control: no-store) \u2014\nevery call fetches fresh from Bing.\n\n================================================================\nSECTION A - RESPONSE FORMAT (single format, always)\n================================================================\n\nEvery search returns plain text only. No JSON, no format parameter,\nno separate endpoint for the original results. One line per result:\n\n```\nSearch: bitcoin (web, 3 results)\n1. Bitcoin price today - https://coinmarketcap.com/currencies/bitcoin/\n2. BTC USD \u2014 Bitcoin Price and Chart \u2014 TradingView - https://www.tradingview.com/symbols/BTCUSD/\n3. Bitcoin - Open source P2P money - https://bitcoin.org/\n```\n\nThe first line is a header: the query, the vertical, and the real\nnumber of results returned. When page > 1 it also shows the page, e.g.\n`Search: bitcoin (web, 5 results, page 2)`. Then one numbered line per\nresult: title, a space-dash-space, then the URL. If the upstream fails,\na second line says \"Error: upstream_failed\" (or \"upstream_403\" etc.)\nand there are no result lines. If the query is missing or too long,\nthe Worker answers HTTP 400 with `error: q is required (1..500 chars)`.\n\nA1. WHY THIS FORMAT (token research, 2026-11-19)\n\nMeasured on the same 3 real bitcoin results, approximate tokens (chars/4):\n\n- Full JSON (query + filters + results + metadata): 317 tokens, 106 per result\n- JSON results array only: 259 tokens, 86 per result\n- Text with title + URL + snippet + published date: 209 tokens, 70 per result\n- Text with title + URL + snippet: 190 tokens, 63 per result\n- Markdown links + snippet: 191 tokens, 64 per result\n- Pipe-separated title|url|snippet: 174 tokens, 58 per result\n- Title + URL only (this format): 68 tokens, 23 per result\n\nTitle + URL only is the lowest by a wide margin: about 4.7x fewer\ntokens than full JSON and about 2.8x fewer than text with snippets.\nSnippets are dropped on purpose. The title + URL is enough for an\nagent to pick which page to fetch, and fetching the page gives the\nreal content anyway. This format is used for every call, always.\n\nA2. URLS ARE CLEAN. Bing redirect URLs are decoded (the `u` param is\nbase64 with an `a1` prefix) to clean destination URLs. What you get is\nthe real page URL, not a Bing redirect link. Verified live 2026-11-19.\n\n================================================================\nSECTION B - ENDPOINT: /search (Bing)\n================================================================\n\nGET /search?q=... or POST /search with a JSON body. Returns Bing results.\nPOST body params override query params when both are given.\n\nB1. FILTERS \u2014 COMMON (every vertical)\n\n- `q` (required, 1-500 chars): the search query.\n- `vertical`: web | images | videos | news. Default web.\n- `count`: 1-50. Default 10. Note: web/news via Bing RSS returns at\n  most 10 per query (see Result counts below), so count above 10 has\n  no effect there. Images/videos can return more.\n- `page`: page number 1, 2, 3... Default 1. Sets offset = (page-1) x\n  count. See Pagination below \u2014 for web/news it does NOT return new\n  results.\n- `offset` or `first`: exact result offset. Default 0. `page` wins if\n  given.\n- `setlang` or `lang`: UI language, e.g. en-US, en-IN, hi-IN, es-ES,\n  de-DE, fr-FR, pt-BR, ja-JP, zh-CN. Default en-US.\n- `cc` or `region` or `country`: country code, e.g. IN, US, GB, DE, JP.\n  Default: derived from setlang. Change it to get country-specific\n  results.\n- `mkt`: market, e.g. en-IN, en-US. Default: same as setlang.\n- `safesearch`: off | moderate | strict. Default moderate.\n- `time` or `time_range`: day | week | month | year. Recency filter\n  (past 24h, past week, past month, past year). Default: no time filter.\n\nB2. FILTERS \u2014 IMAGES ONLY (vertical=images)\n\n- `size`: small | medium | large | wallpaper.\n- `color`: color | bw (black and white).\n- `imgtype`: photo | clipart | line | animated.\n- `face`: face | portrait.\n- `license`: free | free-share | free-modify | public.\n\nB3. FILTERS \u2014 VIDEOS ONLY (vertical=videos)\n\n- `duration`: short | medium | long.\n- `resolution`: hd | sd.\n\nB4. FILTERS \u2014 NEWS ONLY (vertical=news, Bing News)\n\n- `sort`: date | relevance.\n\nB5. EXAMPLES (/search)\n\n- Web, 10 results, English India:\n  GET /search?q=electric+cars&count=10&setlang=en-IN\n- Web, past week, strict safe search:\n  GET /search?q=cricket&time=week&safesearch=strict\n- Images, large black-and-white, 20 results:\n  GET /search?q=mountain&vertical=images&size=large&color=bw&count=20\n- Videos, short, HD:\n  GET /search?q=yoga&vertical=videos&duration=short&resolution=hd&count=10\n- Bing News, sorted by date:\n  GET /search?q=spacex&vertical=news&sort=date&count=10\n- POST with JSON body:\n  POST /search {\"q\":\"quantum computing\",\"count\":5}\n- Country-wise, same query, India vs USA (different results):\n  GET /search?q=cricket&count=5&setlang=en-IN&cc=IN\n  GET /search?q=cricket&count=5&setlang=en-US&cc=US\n\n================================================================\nSECTION C - PAGINATION\n================================================================\n\n- `page`: page number 1, 2, 3, and so on. Default 1. It sets\n  offset = (page-1) x count, and the header line shows the page when\n  it is above 1, e.g. `Search: bitcoin (web, 5 results, page 2)`.\n- IMPORTANT (verified 2026-11-19, tested 7 ways): pagination does NOT\n  work for Bing web/news from a programmatic fetch. Bing ignores the\n  `first` offset in every form tested: RSS `first`, HTML `first`,\n  `FORM=PERE`, the `FPIG` session token, cookies, cookies+FPIG+FORM\n  together, and `count=50`. Every one returns the same top 10 results.\n  Bing only paginates for a real browser session. So page=2 returns\n  the IDENTICAL results as page=1, in the same order. Do not call\n  page=2 expecting more web/news results \u2014 it gives the same list again.\n- For images/videos, pagination works better because Bing HTML returns\n  more results and respects the offset.\n- To get more web/news results for one query, use different filters\n  instead (change the country, the language, or add a time filter) \u2014\n  those return genuinely different result sets.\n\n================================================================\nSECTION D - COUNTRY-WISE (get different results per country)\n================================================================\n\n- Bing lets you change the country and language to get different,\n  region-specific results for the same query. Change these three together:\n- `cc` (also `region` or `country`): country code, e.g. IN, US, GB, DE, JP.\n  Default: derived from setlang.\n- `setlang` or `lang`: UI language, e.g. en-US, en-IN, hi-IN, es-ES, de-DE.\n  Default en-US.\n- `mkt`: market, e.g. en-IN, en-US. Default: same as setlang.\n- VERIFIED 2026-11-19: the same query `cricket` with setlang=en-IN&cc=IN\n  gives India-focused results (Cricinfo, Cricbuzz/IPL 2026, CREX),\n  while setlang=en-US&cc=US gives US-focused results (Cricket\n  Wireless, the mobile carrier, on top). Only 3 of 5 results overlap.\n  Country-wise filtering works and gives genuinely different results\n  per country.\n\n================================================================\nSECTION E - RESULT COUNTS (verified)\n================================================================\n\n- Web/news, count=10: 10 results. count=50: still 10 \u2014 Bing RSS gives\n  at most 10 per query, so a larger count has no effect on web/news.\n- Images, count=5: 5 results (verified with a cats search).\n- The `page` parameter does not increase the web/news total: page=2\n  returns the same 10 (or 5) results as page=1.\n\n================================================================\nSECTION F - BEHAVIOUR AND LIMITS\n================================================================\n\n- Fetches Bing from Cloudflare's network. Typical response 100-700ms.\n  Verified 30/30 Bing searches at 15 per minute, 28/30 with results,\n  about 440ms average.\n- Web and news use Bing HTML search (not RSS), parsed from the result\n  `<h2>` links. Bing redirect URLs are decoded (the `u` param is\n  base64 with an `a1` prefix) to clean destination URLs. RSS is kept\n  as a fallback if HTML parsing returns nothing. Images and videos\n  parse Bing's HTML embedded JSON; if Bing changes its page markup\n  those two verticals can return fewer results.\n- The Worker sets a 20 second fetch timeout to Bing. A slow or failed\n  upstream returns a header plus \"Error: upstream_failed\" (or\n  \"upstream_403\" etc.) and no result lines.\n- Free tier Worker: about 30 seconds max per request. No rate limit is\n  set on the Worker itself; Bing may throttle very aggressive use.\n- Results are not cached (Cache-Control: no-store). Every call fetches\n  fresh.\n- CORS is open (Access-Control-Allow-Origin: *), so browser and agent\n  clients can call it directly.\n- Errors: missing/invalid `q` \u2192 HTTP 400. Upstream failure \u2192 header +\n  `Error: upstream_failed` (or upstream_403 etc.), HTTP 200 with no\n  result lines. Unknown path \u2192 HTTP 404 `not found: use /search?q=...`.\n\n================================================================\nSECTION G - WHAT IT IS NOT\n================================================================\n\n- It is not a general web proxy. It only searches Bing (/search).\n- It does not fetch page content, only search result listings\n  (title + URL). Fetch the page yourself for content.\n- Google News does NOT work from Cloudflare's network. A /gnews\n  endpoint was built and then removed 2026-11-19: Google News RSS\n  returns 503 to Cloudflare egress IPs (it works from a normal VM\n  directly). Bing News via /search vertical=news is the working news\n  source.\n- DuckDuckGo does NOT work from Cloudflare's network. Tested twice\n  2026-11-19: first via the vault Worker (lite, html and main\n  endpoints all returned upstream_failed after about 20 seconds each),\n  then via a live /ddg endpoint built into this Worker\n  (html.duckduckgo.com returned HTTP 202, a bot-challenge page, 0\n  results). The endpoint was removed. GitHub repos that scrape\n  DuckDuckGo from Workers exist, but from this Worker's Cloudflare IPs\n  DuckDuckGo serves a bot-challenge, not results. It may work\n  intermittently or from other IPs; it does not work here.\n- Google and SearxNG also fail from the Worker. Bing is the only\n  working search source here.\n\n================================================================\nSECTION H - AGENT COOKBOOK (copy-paste)\n================================================================\n\nH1. Fastest web answer: GET /search?q=<terms>&count=10\nH2. News for the past day: GET /search?q=<terms>&vertical=news&time=day&sort=date\nH3. Same query, two countries: GET /search?q=<terms>&setlang=en-IN&cc=IN\n    then GET /search?q=<terms>&setlang=en-US&cc=US \u2014 merge, dedupe.\nH4. Images for a mood board: GET /search?q=<terms>&vertical=images&size=large&count=20\nH5. Videos, short and HD: GET /search?q=<terms>&vertical=videos&duration=short&resolution=hd\nH6. Combine with another engine: run TinyFish and this Worker on the\n    same query, merge the URLs, dedupe, then fetch the top pages.\n    That is what web.py `research` does (see the muse-pi-setup repo).\nH7. When /search returns an error line, retry once after a few seconds.\n    If it fails again, use the other engine \u2014 do not retry-loop.\nH8. When you need more than 10 web results, do NOT call page=2 (same\n    results). Change the country, language, or time filter instead.\n\n================================================================\nSECTION I - DEPLOY YOUR OWN\n================================================================\n\nThe Worker needs no secrets, no KV, no D1 \u2014 it is a plain\nfetch-and-parse bridge. Deploy it with your own Cloudflare account:\n\n  1. Create a Worker in the Cloudflare dashboard (or `npx wrangler deploy`\n     from the cf-search-worker/ source in the muse-pi-setup repo).\n  2. Paste src/index.js, deploy. No environment variables needed.\n  3. Your URL is https://<your-worker>.<your-subdomain>.workers.dev\n  4. Set CF_SEARCH_URL to your URL (env export) so web.py uses it.\n  5. Test: GET /health \u2192 \"ok cf-search web images videos news\"\n     GET /search?q=bitcoin&count=3 \u2192 3 results, plain text.\n\nThe Worker URL is NEVER hardcoded in web.py or the skill. Muse ASKS\nthe user for their Worker URL during setup and sets CF_SEARCH_URL.\n";

export default {
  async fetch(request) {
    const url = new URL(request.url);
    const cors = { 'Access-Control-Allow-Origin': '*', 'Access-Control-Allow-Methods': 'GET, POST, OPTIONS', 'Access-Control-Allow-Headers': 'Content-Type' };
    if (request.method === 'OPTIONS') return new Response(null, { headers: cors });
    if (url.pathname === '/' || url.pathname === '/health') {
      return new Response('ok cf-search web images videos news\n', { headers: { ...cors, 'Content-Type': 'text/plain; charset=utf-8' } });
    }
    if (url.pathname === '/llm.txt' || url.pathname === '/llms.txt') {
      return new Response(LLMS_TXT, { headers: { ...cors, 'Content-Type': 'text/plain; charset=utf-8', 'Cache-Control': 'public, max-age=3600' } });
    }
    if (url.pathname !== '/search') {
      return new Response('not found: use /search?q=...\n', { status: 404, headers: { ...cors, 'Content-Type': 'text/plain; charset=utf-8' } });
    }

    // Params: GET query, or POST JSON body (body wins).
    let p = {};
    for (const [k, v] of url.searchParams.entries()) p[k] = v;
    if (request.method === 'POST') {
      try { const b = await request.json(); if (b && typeof b === 'object') p = { ...p, ...b }; } catch { /* ignore bad body */ }
    }
    const q = (p.q || '').toString().trim();
    if (!q || q.length > 500) {
      return new Response('error: q is required (1..500 chars)\n', { status: 400, headers: { ...cors, 'Content-Type': 'text/plain; charset=utf-8' } });
    }
    const vertical = ['web', 'images', 'videos', 'news'].includes((p.vertical || 'web').toLowerCase()) ? (p.vertical || 'web').toLowerCase() : 'web';
    const count = Math.min(50, Math.max(1, parseInt(p.count || '10', 10) || 10));
    const page = Math.max(1, parseInt(p.page || '1', 10) || 1);
    const offset = p.page ? (page - 1) * count : Math.max(0, parseInt(p.offset || p.first || '0', 10) || 0);
    const setlang = (p.setlang || p.lang || 'en-US').toString();
    const cc = (p.cc || p.region || p.country || '').toString();
    const mkt = (p.mkt || setlang).toString();
    const safesearch = ['off', 'moderate', 'strict'].includes((p.safesearch || 'moderate').toLowerCase()) ? (p.safesearch || 'moderate').toLowerCase() : 'moderate';
    const time = ['day', 'week', 'month', 'year'].includes((p.time || p.time_range || '').toLowerCase()) ? (p.time || p.time_range || '').toLowerCase() : '';
    p = { ...p, vertical, count, offset, setlang, cc, mkt, safesearch, time };

    const qft = buildQft(p);
    const enc = encodeURIComponent(q);
    let bingUrl, parse;
    if (vertical === 'web') {
      // HTML search, not RSS: RSS ignores the first= offset, so page=2
      // returns the same results as page=1. HTML respects first=, so
      // pagination actually works.
      bingUrl = `https://www.bing.com/search?q=${enc}&count=${count}&first=${offset}&setlang=${setlang}&mkt=${mkt}&safesearch=${safesearch}${cc ? '&cc=' + encodeURIComponent(cc) : ''}${qft ? '&qft=' + encodeURIComponent(qft) : ''}`;
      parse = parseBingHtml;
    } else if (vertical === 'news') {
      bingUrl = `https://www.bing.com/news/search?q=${enc}&count=${count}&first=${offset}&setlang=${setlang}&mkt=${mkt}&safesearch=${safesearch}${cc ? '&cc=' + encodeURIComponent(cc) : ''}${qft ? '&qft=' + encodeURIComponent(qft) : ''}${p.sort ? '&sortby=' + encodeURIComponent(p.sort) : ''}`;
      parse = parseBingHtml;
    } else if (vertical === 'images') {
      bingUrl = `https://www.bing.com/images/search?q=${enc}&count=${count}&first=${offset}&setlang=${setlang}&mkt=${mkt}&safesearch=${safesearch}${cc ? '&cc=' + encodeURIComponent(cc) : ''}${qft ? '&qft=' + encodeURIComponent(qft) : ''}`;
      parse = parseImages;
    } else {
      bingUrl = `https://www.bing.com/videos/search?q=${enc}&count=${count}&first=${offset}&setlang=${setlang}&mkt=${mkt}&safesearch=${safesearch}${cc ? '&cc=' + encodeURIComponent(cc) : ''}${qft ? '&qft=' + encodeURIComponent(qft) : ''}`;
      parse = parseVideos;
    }

    let results = [], upstream = 0, error = null;
    try {
      if (vertical === 'web' || vertical === 'news') {
        // HTML search respects first=, so one fetch with count and first
        // gives the right page. Fall back to RSS (page 1 only, no offset)
        // if HTML parsing returns nothing, in case Bing changes markup.
        const r = await bingFetch(bingUrl);
        upstream = r.status;
        if (r.status === 200) {
          results = parse(r.text).slice(0, count);
        } else {
          error = 'upstream_' + r.status;
        }
        if (results.length === 0 && !error) {
          const rssUrl = bingUrl.replace('bing.com/search?', 'bing.com/search?format=rss&').replace('bing.com/news/search?', 'bing.com/news/search?format=rss&');
          const rr = await bingFetch(rssUrl);
          if (rr.status === 200) results = parseRss(rr.text).slice(0, count);
        }
      } else {
        const r = await bingFetch(bingUrl);
        upstream = r.status;
        if (r.status === 200) {
          results = parse(r.text).slice(0, count);
        } else {
          error = 'upstream_' + r.status;
        }
      }
    } catch (e) {
      error = 'upstream_failed';
    }

    // Single format, always: plain text, one line per result.
    const lines = [`Search: ${q} (${vertical}, ${results.length} result${results.length === 1 ? '' : 's'}${page > 1 ? `, page ${page}` : ''})`];
    if (error) lines.push(`Error: ${error}`);
    results.forEach((r, i) => lines.push(`${i + 1}. ${r.title} - ${r.url}`));
    return new Response(lines.join('\n') + '\n', { headers: { ...cors, 'Content-Type': 'text/plain; charset=utf-8', 'Cache-Control': 'no-store' } });
  },
};
