#!/usr/bin/env python3
"""web.py — fast search/fetch: TinyFish free via Monid + cf-search Worker (Bing) + X.

Order (fastest / cheapest first):
  search: 1) TinyFish /free/search via Monid API ($0, 30/min, 500/hr)
          2) cf-search Worker — Bing web/images/videos/news, no key, free
             (your own Worker URL — Muse asks you for it, plain text)
  fetch:  1) TinyFish /free/fetch via Monid API ($0, 150 URLs/min, 1000/day)
          2) plain curl (browser UA)

DuckDuckGo was REMOVED 2026-11-19 — it returns nothing from the Pi
sandbox (bot-challenge, []) and from Cloudflare's network (HTTP 202).
It only works from a normal VM. Do not add it back.

Monid key: read from Monid's own credential store
~/.config/monid/credentials.yaml (label "main", chmod 600) or
$MONID_API_KEY. The literal key is NEVER in this file, in the
workspace, or in memory — see ~/workspace/search/SEARCH.md.

cf-search Worker (Bing, no key):
  GET /search?q=...&vertical=web|images|videos|news&count=1-50
  &setlang=en-IN&cc=IN&time=day|week|month|year&safesearch=moderate
  Returns plain text, one line per result: "N. Title - URL".
  The Worker URL is NEVER hardcoded — it is the user's own Worker,
  deployed from docs/SEARCH-WORKER.md. Muse ASKS the user for their
  Worker URL during setup and sets CF_SEARCH_URL (env export).
  Limits (verified 2026-11-19): web/news max 10 per query; pagination
  does NOT work (Bing ignores first= in every form tested — page=2
  returns the same results as page=1); country-wise filtering DOES work
  (setlang+cc change the results). Google News and DuckDuckGo do NOT
  work from Cloudflare's network (503 / 202 bot-challenge). Bing only.
  Full docs: search/cf-search-llms.txt (in this repo).

X / social (fixtweet worker, no key, UA mandatory):
  The worker URL is NEVER hardcoded — the user sets it in /admin
  (Web search settings) and it is stored in search.json next to this
  file (or $FXEMBED_WORKER_URL). If it is empty, the X leg is skipped.
  /ai/2/ask?q=&pages=N (max 8) — one merged Markdown pack, best for X search
  /api/2/search?q=&feed=latest|top|media&count=&cursor= — JSON, cursor.bottom walks
  /ai/2/status/{id} /thread/{id} /profile/{handle} /profile/{handle}/statuses
  /ai/2/post?url=<any X/Bsky/TikTok/IG/Threads/Mastodon post or profile URL>
  /ai/2/trends — X trends
  500 during a walk = retry same cursor after ~30s; 404 = exhausted.
  Two bases exist (fixtweet + fixtweet-2, identical clones) — set the
  second URL in settings too for failover. Full manual: the worker's
  /llms.txt (e.g. https://fixtweet.twapiworker.workers.dev/llms.txt).

Combined research (maximum URLs + data):
  research = TinyFish search + cf-search Worker (dedup URLs)
             + X search (/ai/2/ask) + TinyFish fetch of top URLs.
  Always use `research` for any real research question — never just
  one source. See ~/workspace/custom-skills/web-research/SKILL.md.

Usage:
  python3 web.py search "Pi coding agent" [--purpose "..."]
  python3 web.py search-cf "query" [--vertical web] [--count 10] [--cc IN] [--setlang en-IN]
  python3 web.py fetch https://example.com [https://...]
  python3 web.py xsearch "Pi coding agent" [--pages 3]
  python3 web.py xpost https://x.com/jack/status/20
  python3 web.py xprofile <handle>
  python3 web.py research "Pi coding agent" [--pages 2]  # combined, best
"""
import argparse, json, os, re, subprocess, sys, time, urllib.parse, urllib.request

MONID_API = "https://api.monid.ai"
CONFIG = os.path.expanduser("~/.config/monid/credentials.yaml")

# ---- Worker URLs: NEVER hardcoded. The user sets them in /admin
# (Web search settings); they are stored in search.json next to this
# file. Env vars override the file. Empty = that leg is skipped. ----
def _load_search_settings():
    settings = {}
    for path in (os.path.join(os.path.dirname(os.path.abspath(__file__)), "search.json"),
                 "/root/pi-bridge/search.json"):
        try:
            with open(path) as f:
                settings = json.load(f)
            break
        except Exception:
            continue
    return settings if isinstance(settings, dict) else {}

_SETTINGS = _load_search_settings()
# fixtweet worker (X/social). Primary + optional second base (failover).
FX = os.environ.get("FXEMBED_WORKER_URL") or _SETTINGS.get("fxembed_url", "")
FX2 = os.environ.get("FXEMBED_WORKER_URL_2") or _SETTINGS.get("fxembed_url_2", "")
# cf-search Worker (Bing). The user's own Worker URL.
CF_SEARCH_URL = os.environ.get("CF_SEARCH_URL") or _SETTINGS.get("cf_search_url", "")
# If a leg's URL is empty, that leg is skipped and TinyFish + the other
# legs still work. No URL is ever written literally in this file.
UA = {"User-Agent": "Muse/1.0"}

# ---- search backend: monid (default) | bing | apify ---------------------
# One backend at a time, user-settable in /admin. Apify legs need a key the
# user added there (token lives in provider-keys.env, never in this file) and
# an actor they added by name + actor ID. Nothing here is pre-filled.
BACKEND = (_SETTINGS.get("backend") or "monid").lower()
if BACKEND not in ("monid", "bing", "apify"):
    BACKEND = "monid"
REDDIT_ON = bool(_SETTINGS.get("reddit", False))
APIFY_API = "https://api.apify.com/v2"
KEYS_ENV = os.path.join(os.path.dirname(os.path.abspath(__file__)), "provider-keys.env")


def _apify_token(env_name):
    """Token for one Apify key slot: env var first, then provider-keys.env.
    Never printed, never returned to a caller."""
    if not env_name:
        return None
    v = os.environ.get(env_name)
    if v:
        return v.strip()
    try:
        for line in open(KEYS_ENV):
            line = line.strip()
            if line.startswith(env_name + "="):
                return line.split("=", 1)[1].strip().strip('"').strip("'")
    except Exception:
        pass
    return None


def _apify_leg(leg):
    """Resolve one Apify leg (search|reddit) -> (token, actor_id) or None.
    Selection is key id + actor name, both chosen by the user in /admin."""
    key_id = _SETTINGS.get("apify_%s_key" % leg) or 0
    actor_name = _SETTINGS.get("apify_%s_actor" % leg) or ""
    if not key_id or not actor_name:
        return None
    for k in _SETTINGS.get("apify_keys") or []:
        if int(k.get("id") or 0) != int(key_id):
            continue
        for a in k.get("actors") or []:
            if a.get("name") == actor_name:
                token = _apify_token(k.get("env"))
                if not token:
                    print("[apify] %s: key slot %s has no token" % (leg, k.get("env")), file=sys.stderr)
                    return None
                return token, a.get("actor_id")
    print("[apify] %s: no actor named %r under that key" % (leg, actor_name), file=sys.stderr)
    return None


def apify_run(actor_id, token, run_input, timeout=180, limit=40):
    """Run one of the user's actors and return its dataset items.
    Actor IDs are public; the token is only needed to run."""
    q = urllib.parse.urlencode({"token": token})
    url = "%s/acts/%s/runs?%s" % (APIFY_API, urllib.parse.quote(actor_id, safe="~"), q)
    req = urllib.request.Request(
        url, data=json.dumps(run_input).encode(),
        headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=40) as r:
            run = json.load(r).get("data", {})
    except Exception as e:
        print("[apify] start failed: %s" % e, file=sys.stderr)
        return None
    rid, ds = run.get("id"), run.get("defaultDatasetId")
    deadline = time.time() + timeout
    while time.time() < deadline:
        st = run.get("status")
        if st in ("SUCCEEDED", "FAILED", "ABORTED", "TIMED-OUT"):
            break
        time.sleep(3)
        try:
            with urllib.request.urlopen("%s/actor-runs/%s?%s" % (APIFY_API, rid, q), timeout=30) as r:
                run = json.load(r).get("data", {})
        except Exception as e:
            print("[apify] poll failed: %s" % e, file=sys.stderr)
            return None
    if run.get("status") != "SUCCEEDED":
        print("[apify] run ended %s" % run.get("status"), file=sys.stderr)
        return None
    ds = ds or run.get("defaultDatasetId")
    try:
        with urllib.request.urlopen(
                "%s/datasets/%s/items?%s&limit=%d" % (APIFY_API, ds, q, limit), timeout=60) as r:
            return json.load(r)
    except Exception as e:
        print("[apify] dataset fetch failed: %s" % e, file=sys.stderr)
        return None


def _first(d, *names):
    for n in names:
        v = d.get(n)
        if isinstance(v, str) and v.strip():
            return v.strip()
    return ""


def apify_search(query, count=20):
    """Web search through the user's own Apify search actor. Input keys differ
    per actor, so a small union is sent — actors ignore what they don't use."""
    leg = _apify_leg("search")
    if not leg:
        return None
    token, actor_id = leg
    items = apify_run(actor_id, token, {"query": query, "q": query, "searchQuery": query,
                                        "keyword": query, "maxItems": count, "limit": count,
                                        "maxResults": count})
    if items is None:
        return None
    out = []
    for it in items:
        if not isinstance(it, dict):
            continue
        url = _first(it, "url", "link", "href", "organicResults")
        title = _first(it, "title", "name", "heading")
        snippet = _first(it, "description", "snippet", "text", "content")
        if url.startswith("http"):
            out.append({"title": title, "url": url, "source": "apify", "snippet": snippet[:400]})
    return out


def apify_reddit(query, count=20):
    """Reddit search through the user's own Reddit scraper actor."""
    leg = _apify_leg("reddit")
    if not leg:
        return None
    token, actor_id = leg
    items = apify_run(actor_id, token, {"query": query, "q": query, "searchQuery": query,
                                        "searchTerm": query, "keyword": query,
                                        "maxItems": count, "limit": count, "maxPosts": count,
                                        "sort": "relevance"})
    if items is None:
        return None
    out = []
    for it in items:
        if not isinstance(it, dict):
            continue
        url = _first(it, "url", "postUrl", "link", "permalink")
        if url.startswith("/r/"):
            url = "https://www.reddit.com" + url
        title = _first(it, "title", "name")
        body = _first(it, "body", "selfText", "text", "description")
        if url.startswith("http"):
            out.append({"title": title, "url": url, "source": "reddit",
                        "snippet": body[:400], "subreddit": _first(it, "subreddit", "communityName")})
    return out


def camoufox_fetch(url):
    """Fetch through the headed Camoufox browser (Instabox) when Monid fetch is
    blocked or fails. Uses the Rust browser CLI; returns text or ''."""
    cli = os.path.join(os.path.dirname(os.path.abspath(__file__)), "browser")
    if not os.path.exists(cli):
        return ""
    try:
        subprocess.run([cli, "navigate", url], capture_output=True, text=True, timeout=90)
        r = subprocess.run([cli, "eval", "document.body.innerText"],
                           capture_output=True, text=True, timeout=60)
        return (r.stdout or "")[:8000] if r.returncode == 0 else ""
    except Exception as e:
        print("[camoufox] fetch failed: %s" % e, file=sys.stderr)
        return ""


def monid_key():
    if os.environ.get("MONID_API_KEY"):
        return os.environ["MONID_API_KEY"]
    try:
        txt = open(CONFIG).read()
        m = re.search(r'key:\s*"([^"]+)"', txt)
        return m.group(1) if m else None
    except Exception:
        return None


def monid_run(provider, endpoint, inp, timeout=120):
    key = monid_key()
    if not key:
        return None
    body = {"provider": provider, "endpoint": endpoint}
    if inp:
        body["input"] = inp
    req = urllib.request.Request(
        MONID_API + "/v1/run", data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        method="POST")
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            run = json.load(r)
    except Exception as e:
        print(f"[monid] run failed: {e}", file=sys.stderr)
        return None
    run_id = run.get("runId")
    for _ in range(int(timeout / 5)):
        if run.get("status") == "COMPLETED":
            return run.get("output")
        if run.get("status") in ("FAILED", "CANCELLED"):
            print(f"[monid] run {run.get('status')}: {run}", file=sys.stderr)
            return None
        time.sleep(5)
        req = urllib.request.Request(
            MONID_API + f"/v1/runs/{run_id}",
            headers={"Authorization": f"Bearer {key}"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                run = json.load(r)
        except Exception as e:
            print(f"[monid] poll failed: {e}", file=sys.stderr)
            return None
    return None


def tinyfish_search(query, purpose=None):
    qp = {"query": query}
    if purpose:
        qp["purpose"] = purpose
    out = monid_run("tinyfish", "/free/search", {"queryParams": qp})
    return out.get("results") if out else None


def tinyfish_fetch(urls):
    out = monid_run("tinyfish", "/free/fetch", {"body": {"urls": urls, "format": "markdown"}})
    return out.get("results") if out else None


def fetch_urls(urls):
    """Fetch rule: Monid (TinyFish free) first; if that fails or comes back
    empty, fall back to the Camoufox browser, then plain curl."""
    res = tinyfish_fetch(urls)
    if res:
        return res, "monid"
    print("[fetch] Monid fetch failed — falling back to the Camoufox browser", file=sys.stderr)
    via_browser = []
    for u in urls:
        t = camoufox_fetch(u)
        if t:
            via_browser.append({"url": u, "text": t})
    if via_browser:
        return via_browser, "camoufox"
    print("[fetch] Camoufox failed too — falling back to curl", file=sys.stderr)
    return [{"url": u, "text": curl_fetch(u)[:8000]} for u in urls], "curl"


def cfsearch_search(query, vertical="web", count=10, setlang=None, cc=None, time_range=None):
    """cf-search Worker (Bing). No key. Returns [{title, url}]. Plain text in, parsed out.
    Skipped (returns None) when CF_SEARCH_URL is not set — the user's own Worker URL."""
    if not CF_SEARCH_URL:
        print("[cf-search] skipped: CF_SEARCH_URL not set (ask the user for their Worker URL)", file=sys.stderr)
        return None
    params = {"q": query, "vertical": vertical, "count": str(count)}
    if setlang:
        params["setlang"] = setlang
    if cc:
        params["cc"] = cc
    if time_range:
        params["time"] = time_range
    url = CF_SEARCH_URL.rstrip("/") + "/search?" + urllib.parse.urlencode(params)
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36"})
        with urllib.request.urlopen(req, timeout=30) as r:
            text = r.read().decode(errors="replace")
    except Exception as e:
        print(f"[cf-search] fetch failed: {e}", file=sys.stderr)
        return None
    out = []
    for line in text.splitlines():
        m = re.match(r"^\d+\.\s+(.+?)\s+-\s+(https?://\S+)\s*$", line)
        if m:
            out.append({"title": m.group(1).strip(), "url": m.group(2).strip()})
    return out


def curl_fetch(url):
    r = subprocess.run(["curl", "-s", "-m", "30", "-A", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36", url],
                       capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else ""


# ---- X / social via fixtweet worker (no key, UA mandatory) ----

def fx_get(path, base=None):
    """GET a path on the fixtweet worker. Returns text or None.
    Skipped (None) when no worker URL is configured. On a flaky
    empty/404, retries once on the second base if one is set."""
    if base is None:
        base = FX
    if not base:
        return None
    req = urllib.request.Request(base.rstrip("/") + path, headers=UA)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        # flaky guest answers: retry once on the other base
        if base == FX and FX2 and FX2 != FX:
            return fx_get(path, base=FX2)
        raise


def x_search(query, pages=2, feed="latest"):
    """One-shot X pack: /ai/2/ask walks N pages server-side, returns Markdown.
    Skipped (None) when no fixtweet worker URL is configured."""
    if not FX:
        print("[x] skipped: no fixtweet worker URL set (set it in /admin Web search settings)", file=sys.stderr)
        return None
    q = urllib.parse.quote(query)
    return fx_get(f"/ai/2/ask?q={q}&pages={max(1, min(8, pages))}")


def x_search_json(query, count=30, feed="latest", max_pages=3):
    """JSON walk: follow cursor.bottom until exhausted / max_pages. 500 = retry same cursor.
    Skipped ([]) when no fixtweet worker URL is configured."""
    if not FX:
        return []
    out, cursor, seen = [], None, set()
    for _ in range(max_pages):
        qs = f"/api/2/search?q={urllib.parse.quote(query)}&feed={feed}&count={count}"
        if cursor:
            qs += f"&cursor={urllib.parse.quote(cursor)}"
        raw = fx_get(qs)
        if raw is None:
            break
        data = json.loads(raw)
        for r in data.get("results", []):
            if r.get("id") not in seen:
                seen.add(r.get("id"))
                out.append(r)
        cursor = (data.get("cursor") or {}).get("bottom")
        if not cursor:
            break
        time.sleep(1)
    return out


def x_post(url_or_id):
    if url_or_id.startswith("http"):
        return fx_get(f"/ai/2/post?url={urllib.parse.quote(url_or_id)}")
    return fx_get(f"/ai/2/status/{url_or_id}")


def x_profile(handle):
    return fx_get(f"/ai/2/profile/{handle.lstrip('@')}")


def x_trends():
    raw = fx_get("/api/2/trends")
    return json.loads(raw) if raw else None


def _leg_on(name):
    """Per-leg toggle from search.json — a leg that is off does not run."""
    return bool(_SETTINGS.get(name, True))


def combined_search(query, purpose=None):
    """Web search on the SELECTED backend (monid | bing | apify), deduped,
    plus the Reddit leg when it is on and Apify is the backend."""
    urls, seen = [], set()
    counts = {}
    def add(rows, label):
        n = 0
        for r in rows or []:
            u = r.get("url")
            if u and u.startswith("http") and u not in seen:
                seen.add(u)
                urls.append({"title": r.get("title"), "url": u, "source": label,
                             "snippet": r.get("snippet")})
                n += 1
        counts[label] = len(rows or [])
        return n
    if BACKEND == "apify":
        rows = apify_search(query)
        if rows is None:
            print("[apify] search leg unavailable — falling back to Monid", file=sys.stderr)
            add(tinyfish_search(query, purpose) if _leg_on("tinyfish") else None, "tinyfish")
        else:
            add(rows, "apify")
    elif BACKEND == "bing":
        add(cfsearch_search(query) if _leg_on("bing") else None, "cf-search")
    else:
        add(tinyfish_search(query, purpose) if _leg_on("tinyfish") else None, "tinyfish")
        add(cfsearch_search(query) if _leg_on("bing") else None, "cf-search")
    reddit = []
    if REDDIT_ON:
        reddit = apify_reddit(query) or []
        add(reddit, "reddit")
    return {"web": urls, "counts": counts, "reddit_count": len(reddit), "backend": BACKEND}


def research(query, pages=2, fetch_top=5):
    """Maximum data: web search on the selected backend (+ Reddit when on) and
    X search, then fetch the top URLs (Monid, then Camoufox, then curl)."""
    combo = combined_search(query)
    x = None
    if _leg_on("x"):
        try:
            x = x_search(query, pages=pages)
        except Exception as e:
            print(f"[research] X search failed ({e}); continuing with web only", file=sys.stderr)
            x = None
    fetched, via = [], ""
    top = [r["url"] for r in combo["web"][:fetch_top] if r["url"].startswith("http")]
    if top:
        fetched, via = fetch_urls(top)
    return {"query": query, "backend": BACKEND, "web_results": combo["web"],
            "counts": combo["counts"], "fetch_via": via,
            "x_pack_markdown": x, "fetched": fetched}


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("search"); s.add_argument("query"); s.add_argument("--purpose")
    c = sub.add_parser("search-cf"); c.add_argument("query")
    c.add_argument("--vertical", default="web", choices=["web", "images", "videos", "news"])
    c.add_argument("--count", type=int, default=10)
    c.add_argument("--setlang"); c.add_argument("--cc"); c.add_argument("--time")
    f = sub.add_parser("fetch"); f.add_argument("urls", nargs="+")
    x = sub.add_parser("xsearch"); x.add_argument("query"); x.add_argument("--pages", type=int, default=2)
    xp = sub.add_parser("xpost"); xp.add_argument("url_or_id")
    xpr = sub.add_parser("xprofile"); xpr.add_argument("handle")
    sub.add_parser("xtrends")
    rd = sub.add_parser("reddit"); rd.add_argument("query"); rd.add_argument("--count", type=int, default=20)
    r = sub.add_parser("research"); r.add_argument("query"); r.add_argument("--pages", type=int, default=2); r.add_argument("--fetch-top", type=int, default=5)
    args = p.parse_args()
    if args.cmd == "search":
        if BACKEND == "apify":
            res = apify_search(args.query)
            if res is None:
                print("[apify] no search leg configured — falling back to Monid", file=sys.stderr)
                res = tinyfish_search(args.query, args.purpose)
        elif BACKEND == "bing":
            res = cfsearch_search(args.query)
        else:
            res = tinyfish_search(args.query, args.purpose)
            if not res:
                print("[tinyfish] no result, falling back to cf-search Worker", file=sys.stderr)
                res = cfsearch_search(args.query)
        print(json.dumps(res or [], indent=2))
    elif args.cmd == "reddit":
        print(json.dumps(apify_reddit(args.query, count=args.count) or [], indent=2))
    elif args.cmd == "search-cf":
        res = cfsearch_search(args.query, vertical=args.vertical, count=args.count,
                              setlang=args.setlang, cc=args.cc, time_range=args.time)
        print(json.dumps(res or [], indent=2))
    elif args.cmd == "fetch":
        res, via = fetch_urls(args.urls)
        print(json.dumps({"via": via, "results": res}, indent=2))
    elif args.cmd == "xsearch":
        print(x_search(args.query, pages=args.pages) or "no X results")
    elif args.cmd == "xpost":
        print(x_post(args.url_or_id) or "not found")
    elif args.cmd == "xprofile":
        print(x_profile(args.handle) or "not found")
    elif args.cmd == "xtrends":
        print(json.dumps(x_trends(), indent=2))
    elif args.cmd == "research":
        print(json.dumps(research(args.query, pages=args.pages, fetch_top=args.fetch_top), indent=2))


if __name__ == "__main__":
    main()
