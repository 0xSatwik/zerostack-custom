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

X / social (FxEmbed, no key, UA mandatory):
  base https://fixtweet.twapiworker.workers.dev (clone fixtweet-2...)
  /ai/2/ask?q=&pages=N (max 8) — one merged Markdown pack, best for X search
  /api/2/search?q=&feed=latest|top|media&count=&cursor= — JSON, cursor.bottom walks
  /ai/2/status/{id} /thread/{id} /profile/{handle} /profile/{handle}/statuses
  /ai/2/post?url=<any X/Bsky/TikTok/IG/Threads/Mastodon post or profile URL>
  500 during a walk = retry same cursor after ~30s; 404 = exhausted.
  Full manual: https://fixtweet.twapiworker.workers.dev/llms.txt

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
FX = "https://fixtweet.twapiworker.workers.dev"
FX2 = "https://fixtweet-2.twapiworker.workers.dev"
CF_SEARCH_URL = os.environ.get("CF_SEARCH_URL", "")
# The cf-search Worker URL is NEVER hardcoded here — it is the user's own
# Worker, deployed from docs/SEARCH-WORKER.md. Muse ASKS the user for their
# Worker URL during setup and sets CF_SEARCH_URL (env export). If it is
# empty, the cf-search leg is skipped and TinyFish + X still work.
UA = {"User-Agent": "Muse/1.0"}


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


# ---- X / social via FxEmbed (no key, UA mandatory) ----

def fx_get(path, base=FX):
    req = urllib.request.Request(base + path, headers=UA)
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        # flaky guest answers: retry once on the other base
        if base == FX:
            return fx_get(path, base=FX2)
        raise


def x_search(query, pages=2, feed="latest"):
    """One-shot X pack: /ai/2/ask walks N pages server-side, returns Markdown."""
    q = urllib.parse.quote(query)
    return fx_get(f"/ai/2/ask?q={q}&pages={max(1, min(8, pages))}")


def x_search_json(query, count=30, feed="latest", max_pages=3):
    """JSON walk: follow cursor.bottom until exhausted / max_pages. 500 = retry same cursor."""
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


def combined_search(query, purpose=None):
    """Web (TinyFish + cf-search Worker, deduped) + X. Returns dict of all sources."""
    urls, seen = [], set()
    tf = tinyfish_search(query, purpose) or []
    for r in tf:
        u = r.get("url")
        if u and u not in seen:
            seen.add(u); urls.append({"title": r.get("title"), "url": u, "source": "tinyfish", "snippet": r.get("snippet")})
    cf = cfsearch_search(query) or []
    for r in cf:
        u = r.get("url")
        if u and u not in seen:
            seen.add(u); urls.append({"title": r.get("title"), "url": u, "source": "cf-search"})
    return {"web": urls, "tinyfish_count": len(tf), "cfsearch_count": len(cf)}


def research(query, pages=2, fetch_top=5):
    """Maximum data: web search (three engines) + X search, then fetch top URLs."""
    combo = combined_search(query)
    try:
        x = x_search(query, pages=pages)
    except Exception as e:
        print(f"[research] X search failed ({e}); continuing with web only", file=sys.stderr)
        x = None
    fetched = []
    top = [r["url"] for r in combo["web"][:fetch_top] if r["url"].startswith("http")]
    if top:
        fetched = tinyfish_fetch(top) or []
        if not fetched:
            fetched = [{"url": u, "text": curl_fetch(u)[:8000]} for u in top]
    return {"query": query, "web_results": combo["web"],
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
    r = sub.add_parser("research"); r.add_argument("query"); r.add_argument("--pages", type=int, default=2); r.add_argument("--fetch-top", type=int, default=5)
    args = p.parse_args()
    if args.cmd == "search":
        res = tinyfish_search(args.query, args.purpose)
        if res:
            print(json.dumps(res, indent=2)); return
        print("[tinyfish] no result, falling back to cf-search Worker", file=sys.stderr)
        res = cfsearch_search(args.query)
        print(json.dumps(res or [], indent=2))
    elif args.cmd == "search-cf":
        res = cfsearch_search(args.query, vertical=args.vertical, count=args.count,
                              setlang=args.setlang, cc=args.cc, time_range=args.time)
        print(json.dumps(res or [], indent=2))
    elif args.cmd == "fetch":
        res = tinyfish_fetch(args.urls)
        if res:
            print(json.dumps(res, indent=2)); return
        print("[tinyfish] no result, falling back to curl", file=sys.stderr)
        for u in args.urls:
            print(f"## {u}\n{curl_fetch(u)[:8000]}")
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
