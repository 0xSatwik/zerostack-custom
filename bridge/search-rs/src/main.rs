//! web/search — fast search/fetch: TinyFish free via Monid + cf-search
//! Worker (Bing) + X. Rust port of /root/pi-bridge/search.py (B4).
//!
//! Order (fastest / cheapest first):
//!   search: 1) TinyFish /free/search via Monid API ($0)
//!           2) cf-search Worker — Bing web/images/videos/news, no key, free
//!   fetch:  1) TinyFish /free/fetch via Monid API ($0)
//!           2) Camoufox browser (Instabox), 3) plain curl
//!
//! HTTPS is done via the `curl` binary (no TLS crate) — curl handles TLS,
//! and the Monid Bearer token is passed through a 0600 temp config file,
//! never on a command line (it would show in `ps`).
//!
//! Worker URLs are NEVER hardcoded: they come from search.json next to the
//! binary (or /root/pi-bridge/search.json) or env vars. Empty = that leg
//! is skipped. The Monid key comes from $MONID_API_KEY or
//! ~/.config/monid/credentials.yaml (label "main").
//!
//! Usage:
//!   search search "Pi coding agent" [--purpose "..."]
//!   search search-cf "query" [--vertical web] [--count 10] [--cc IN] [--setlang en-IN] [--time day]
//!   search fetch https://example.com [https://...]
//!   search xsearch "Pi coding agent" [--pages 3]
//!   search xpost https://x.com/jack/status/20
//!   search xprofile <handle>
//!   search xtrends
//!   search reddit "query" [--count 20]
//!   search research "Pi coding agent" [--pages 2] [--fetch-top 5]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MONID_API: &str = "https://api.monid.ai";
const APIFY_API: &str = "https://api.apify.com/v2";
const BROWSER_UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";
const WORKER_UA: &str = "Muse/1.0";

// ---------------------------------------------------------------- config ---

fn settings_path() -> Option<PathBuf> {
    // Next to the binary first, then the box's canonical location.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("search.json");
            if p.exists() {
                return Some(p);
            }
        }
    }
    let p = PathBuf::from("/root/pi-bridge/search.json");
    if p.exists() {
        return Some(p);
    }
    None
}

fn load_settings() -> serde_json::Value {
    if let Some(p) = settings_path() {
        if let Ok(t) = fs::read_to_string(&p) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                if v.is_object() {
                    return v;
                }
            }
        }
    }
    serde_json::json!({})
}

fn setting(settings: &serde_json::Value, key: &str) -> String {
    settings.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn env_or(settings: &serde_json::Value, env: &str, key: &str) -> String {
    if let Ok(v) = std::env::var(env) {
        if !v.is_empty() {
            return v;
        }
    }
    setting(settings, key)
}

fn leg_on(settings: &serde_json::Value, name: &str) -> bool {
    settings.get(name).and_then(|v| v.as_bool()).unwrap_or(true)
}

// ----------------------------------------------------------------- http ---

/// Run curl. `headers` are written to a 0600 temp config file (so a Bearer
/// token never appears in `ps`). Returns (http_code, body). On a curl
/// failure returns (0, "").
fn curl_run(url: &str, headers: &[String], body: Option<&str>, timeout_secs: u64) -> (u16, String) {
    let pid = std::process::id();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let hdr_path = format!("/tmp/search-hdr-{}-{}.txt", pid, nonce);
    let body_path = format!("/tmp/search-body-{}-{}.json", pid, nonce);
    let mut cfg = String::new();
    for h in headers {
        // Escape backslashes and quotes for the curl config file.
        let esc = h.replace('\\', "\\\\").replace('"', "\\\"");
        cfg.push_str(&format!("header = \"{}\"\n", esc));
    }
    let _ = fs::write(&hdr_path, cfg);
    // Restrict the header file (it may hold a Bearer token).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&hdr_path, fs::Permissions::from_mode(0o600));
    }
    let mut cmd = Command::new("curl");
    cmd.arg("-s")
        .arg("-m")
        .arg(timeout_secs.to_string())
        .arg("-K")
        .arg(&hdr_path)
        .arg("-w")
        .arg("\n__HTTP_CODE__:%{http_code}");
    if let Some(b) = body {
        let _ = fs::write(&body_path, b);
        cmd.arg("--data-binary").arg(format!("@{}", body_path));
    }
    cmd.arg(url).stdout(Stdio::piped()).stderr(Stdio::null());
    let out = cmd.output();
    let _ = fs::remove_file(&hdr_path);
    let _ = fs::remove_file(&body_path);
    match out {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            // Split the trailing __HTTP_CODE__:NNN marker.
            if let Some(idx) = text.rfind("\n__HTTP_CODE__:") {
                let code: u16 = text[idx + 15..].trim().parse().unwrap_or(0);
                (code, text[..idx].to_string())
            } else {
                (0, text)
            }
        }
        Err(_) => (0, String::new()),
    }
}

fn http_get(url: &str, ua: &str, timeout_secs: u64) -> (u16, String) {
    curl_run(url, &[format!("User-Agent: {}", ua)], None, timeout_secs)
}

fn http_get_auth(url: &str, token: &str, timeout_secs: u64) -> (u16, String) {
    curl_run(
        url,
        &[format!("Authorization: Bearer {}", token)],
        None,
        timeout_secs,
    )
}

fn http_post_json(url: &str, token: Option<&str>, body: &str, timeout_secs: u64) -> (u16, String) {
    let mut headers = vec!["Content-Type: application/json".to_string()];
    if let Some(t) = token {
        headers.push(format!("Authorization: Bearer {}", t));
    }
    curl_run(url, &headers, Some(body), timeout_secs)
}

fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// ---------------------------------------------------------------- monid ---

fn monid_key() -> Option<String> {
    if let Ok(k) = std::env::var("MONID_API_KEY") {
        if !k.is_empty() {
            return Some(k);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let cfg = format!("{}/.config/monid/credentials.yaml", home);
    if let Ok(txt) = fs::read_to_string(&cfg) {
        // key: "...." (label "main")
        for line in txt.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("key:") {
                let v = rest.trim().trim_matches('"').trim_matches('\'');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// POST /v1/run then poll /v1/runs/{id} until COMPLETED/FAILED/CANCELLED.
fn monid_run(provider: &str, endpoint: &str, input: serde_json::Value, timeout_secs: u64) -> Option<serde_json::Value> {
    let key = monid_key()?;
    let body = serde_json::json!({"provider": provider, "endpoint": endpoint, "input": input});
    let (code, text) = http_post_json(
        &format!("{}/v1/run", MONID_API),
        Some(&key),
        &serde_json::to_string(&body).unwrap_or_default(),
        30,
    );
    if code != 200 {
        eprintln!("[monid] run failed: HTTP {}", code);
        return None;
    }
    let mut run: serde_json::Value = serde_json::from_str(&text).ok()?;
    let run_id = run.get("runId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let status = run.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "COMPLETED" {
            return run.get("output").cloned();
        }
        if status == "FAILED" || status == "CANCELLED" {
            eprintln!("[monid] run {}: {}", status, serde_json::to_string(&run).unwrap_or_default());
            return None;
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_secs(5));
        let (code, text) = http_get_auth(&format!("{}/v1/runs/{}", MONID_API, run_id), &key, 30);
        if code != 200 {
            eprintln!("[monid] poll failed: HTTP {}", code);
            return None;
        }
        run = serde_json::from_str(&text).ok()?;
    }
}

fn tinyfish_search(query: &str, purpose: Option<&str>) -> Option<Vec<serde_json::Value>> {
    let mut qp = serde_json::json!({"query": query});
    if let Some(p) = purpose {
        qp["purpose"] = serde_json::json!(p);
    }
    let out = monid_run("tinyfish", "/free/search", serde_json::json!({"queryParams": qp}), 120)?;
    out.get("results").and_then(|r| r.as_array()).cloned()
}

fn tinyfish_fetch(urls: &[String]) -> Option<Vec<serde_json::Value>> {
    let out = monid_run(
        "tinyfish",
        "/free/fetch",
        serde_json::json!({"body": {"urls": urls, "format": "markdown"}}),
        120,
    )?;
    out.get("results").and_then(|r| r.as_array()).cloned()
}

// ----------------------------------------------------------------- fetch ---

fn curl_fetch(url: &str) -> String {
    let (code, text) = http_get(url, BROWSER_UA, 30);
    if code == 200 {
        text
    } else {
        String::new()
    }
}

fn camoufox_fetch(url: &str, search_dir: &std::path::Path) -> String {
    let cli = search_dir.join("browser");
    if !cli.exists() {
        return String::new();
    }
    let nav = Command::new(&cli)
        .arg("navigate")
        .arg(url)
        .output();
    if nav.map(|o| o.status.success()).unwrap_or(false) {
        if let Ok(o) = Command::new(&cli).arg("eval").arg("document.body.innerText").output() {
            if o.status.success() {
                let t = String::from_utf8_lossy(&o.stdout).into_owned();
                return t.chars().take(8000).collect();
            }
        }
    }
    String::new()
}

/// Fetch rule: Monid (TinyFish free) first; then Camoufox browser, then curl.
fn fetch_urls(urls: &[String], search_dir: &std::path::Path) -> (Vec<serde_json::Value>, String) {
    if let Some(res) = tinyfish_fetch(urls) {
        if !res.is_empty() {
            return (res, "monid".to_string());
        }
    }
    eprintln!("[fetch] Monid fetch failed — falling back to the Camoufox browser");
    let mut via_browser: Vec<serde_json::Value> = Vec::new();
    for u in urls {
        let t = camoufox_fetch(u, search_dir);
        if !t.is_empty() {
            via_browser.push(serde_json::json!({"url": u, "text": t}));
        }
    }
    if !via_browser.is_empty() {
        return (via_browser, "camoufox".to_string());
    }
    eprintln!("[fetch] Camoufox failed too — falling back to curl");
    let res: Vec<serde_json::Value> = urls
        .iter()
        .map(|u| serde_json::json!({"url": u, "text": curl_fetch(u).chars().take(8000).collect::<String>()}))
        .collect();
    (res, "curl".to_string())
}

// -------------------------------------------------------------- cf-search ---

fn cfsearch_search(settings: &serde_json::Value, query: &str, vertical: &str, count: u32, setlang: Option<&str>, cc: Option<&str>, time_range: Option<&str>) -> Option<Vec<serde_json::Value>> {
    let base = env_or(settings, "CF_SEARCH_URL", "cf_search_url");
    if base.is_empty() {
        eprintln!("[cf-search] skipped: CF_SEARCH_URL not set (ask the user for their Worker URL)");
        return None;
    }
    let mut params = format!("q={}&vertical={}&count={}", url_encode(query), url_encode(vertical), count);
    if let Some(v) = setlang {
        params.push_str(&format!("&setlang={}", url_encode(v)));
    }
    if let Some(v) = cc {
        params.push_str(&format!("&cc={}", url_encode(v)));
    }
    if let Some(v) = time_range {
        params.push_str(&format!("&time={}", url_encode(v)));
    }
    let url = format!("{}/search?{}", base.trim_end_matches('/'), params);
    let (code, text) = http_get(&url, BROWSER_UA, 30);
    if code != 200 {
        eprintln!("[cf-search] fetch failed: HTTP {}", code);
        return None;
    }
    let mut out: Vec<serde_json::Value> = Vec::new();
    for line in text.lines() {
        // "N. Title - URL"
        if let Some((title, url)) = parse_cf_line(line) {
            out.push(serde_json::json!({"title": title, "url": url}));
        }
    }
    Some(out)
}

fn parse_cf_line(line: &str) -> Option<(String, String)> {
    // ^\d+\.\s+(.+?)\s+-\s+(https?://\S+)\s*$
    let line = line.trim();
    let mut rest = line;
    // Strip the leading "N. "
    let mut i = 0;
    while i < rest.len() && rest.as_bytes()[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || !rest[i..].starts_with('.') {
        return None;
    }
    rest = rest[i + 1..].trim_start();
    // Find " - http" separator (last occurrence wins for titles with dashes).
    let sep = rest.rfind(" - http")?;
    let title = rest[..sep].trim().to_string();
    let url = rest[sep + 3..].trim().to_string();
    if url.starts_with("http") && !title.is_empty() {
        Some((title, url))
    } else {
        None
    }
}

// ------------------------------------------------------- X / fixtweet ---

fn fx_get(settings: &serde_json::Value, path: &str) -> Option<String> {
    let fx = env_or(settings, "FXEMBED_WORKER_URL", "fxembed_url");
    let fx2 = env_or(settings, "FXEMBED_WORKER_URL_2", "fxembed_url_2");
    fx_get_base(&fx, &fx2, path)
}

fn fx_get_base(fx: &str, fx2: &str, path: &str) -> Option<String> {
    if fx.is_empty() {
        return None;
    }
    let url = format!("{}{}", fx.trim_end_matches('/'), path);
    let (code, text) = http_get(&url, WORKER_UA, 60);
    if code == 200 {
        return Some(text);
    }
    if code == 404 {
        return None;
    }
    // Flaky guest answers: retry once on the other base.
    if !fx2.is_empty() && fx2 != fx {
        let url2 = format!("{}{}", fx2.trim_end_matches('/'), path);
        let (code2, text2) = http_get(&url2, WORKER_UA, 60);
        if code2 == 200 {
            return Some(text2);
        }
        if code2 == 404 {
            return None;
        }
    }
    None
}

fn x_search(settings: &serde_json::Value, query: &str, pages: u32) -> Option<String> {
    let fx = env_or(settings, "FXEMBED_WORKER_URL", "fxembed_url");
    if fx.is_empty() {
        eprintln!("[x] skipped: no fixtweet worker URL set (set it in /admin Web search settings)");
        return None;
    }
    let p = pages.clamp(1, 8);
    fx_get(settings, &format!("/ai/2/ask?q={}&pages={}", url_encode(query), p))
}

fn x_search_json(settings: &serde_json::Value, query: &str, count: u32, feed: &str, max_pages: u32) -> Vec<serde_json::Value> {
    let fx = env_or(settings, "FXEMBED_WORKER_URL", "fxembed_url");
    if fx.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..max_pages {
        let mut qs = format!("/api/2/search?q={}&feed={}&count={}", url_encode(query), url_encode(feed), count);
        if let Some(c) = &cursor {
            qs.push_str(&format!("&cursor={}", url_encode(c)));
        }
        let raw = match fx_get(settings, &qs) {
            Some(r) => r,
            None => break,
        };
        let data: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => break,
        };
        if let Some(results) = data.get("results").and_then(|r| r.as_array()) {
            for r in results {
                let id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !seen.contains(&id) {
                    seen.push(id);
                    out.push(r.clone());
                }
            }
        }
        cursor = data
            .get("cursor")
            .and_then(|c| c.get("bottom"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        if cursor.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    out
}

fn x_post(settings: &serde_json::Value, url_or_id: &str) -> Option<String> {
    if url_or_id.starts_with("http") {
        fx_get(settings, &format!("/ai/2/post?url={}", url_encode(url_or_id)))
    } else {
        fx_get(settings, &format!("/ai/2/status/{}", url_or_id))
    }
}

fn x_profile(settings: &serde_json::Value, handle: &str) -> Option<String> {
    fx_get(settings, &format!("/ai/2/profile/{}", handle.trim_start_matches('@')))
}

fn x_trends(settings: &serde_json::Value) -> Option<serde_json::Value> {
    let raw = fx_get(settings, "/api/2/trends")?;
    serde_json::from_str(&raw).ok()
}

// ----------------------------------------------------------------- apify ---

fn apify_token(settings: &serde_json::Value, env_name: &str) -> Option<String> {
    if env_name.is_empty() {
        return None;
    }
    if let Ok(v) = std::env::var(env_name) {
        if !v.is_empty() {
            return Some(v.trim().to_string());
        }
    }
    // provider-keys.env next to the binary (or /root/pi-bridge).
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("provider-keys.env"));
        }
    }
    candidates.push(PathBuf::from("/root/pi-bridge/provider-keys.env"));
    let _ = settings;
    for path in candidates {
        if let Ok(txt) = fs::read_to_string(&path) {
            for line in txt.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix(&format!("{}=", env_name)) {
                    let v = rest.trim().trim_matches('"').trim_matches('\'');
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

fn apify_leg(settings: &serde_json::Value, leg: &str) -> Option<(String, String)> {
    let key_id = settings.get(&format!("apify_{}_key", leg)).and_then(|v| v.as_i64()).unwrap_or(0);
    let actor_name = settings.get(&format!("apify_{}_actor", leg)).and_then(|v| v.as_str()).unwrap_or("");
    if key_id == 0 || actor_name.is_empty() {
        return None;
    }
    let keys = settings.get("apify_keys").and_then(|v| v.as_array())?;
    for k in keys {
        if k.get("id").and_then(|v| v.as_i64()).unwrap_or(0) != key_id {
            continue;
        }
        let env_name = k.get("env").and_then(|v| v.as_str()).unwrap_or("");
        for a in k.get("actors").and_then(|v| v.as_array()).map(|v| v.as_slice()).unwrap_or(&[]) {
            if a.get("name").and_then(|v| v.as_str()) == Some(actor_name) {
                let actor_id = a.get("actor_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let token = apify_token(settings, env_name);
                if token.is_none() {
                    eprintln!("[apify] {}: key slot {} has no token", leg, env_name);
                    return None;
                }
                return Some((token.unwrap(), actor_id));
            }
        }
    }
    eprintln!("[apify] {}: no actor named {:?} under that key", leg, actor_name);
    None
}

fn apify_run(actor_id: &str, token: &str, run_input: serde_json::Value, timeout_secs: u64, limit: u32) -> Option<Vec<serde_json::Value>> {
    let q = format!("token={}", url_encode(token));
    let url = format!("{}/acts/{}/runs?{}", APIFY_API, url_encode(actor_id), q);
    let body = serde_json::to_string(&run_input).unwrap_or_default();
    let (code, text) = http_post_json(&url, None, &body, 40);
    if code != 200 && code != 201 {
        eprintln!("[apify] start failed: HTTP {}", code);
        return None;
    }
    let mut run: serde_json::Value = serde_json::from_str(&text).ok()?;
    let data = run.get("data").cloned().unwrap_or(serde_json::json!({}));
    let rid = data.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut ds = data.get("defaultDatasetId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let status = run.get("data").and_then(|d| d.get("status")).and_then(|v| v.as_str()).unwrap_or("");
        if ["SUCCEEDED", "FAILED", "ABORTED", "TIMED-OUT"].contains(&status) {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_secs(3));
        let (code, text) = http_get(&format!("{}/actor-runs/{}?{}", APIFY_API, rid, q), WORKER_UA, 30);
        if code != 200 {
            eprintln!("[apify] poll failed: HTTP {}", code);
            return None;
        }
        run = serde_json::from_str(&text).ok()?;
    }
    let status = run.get("data").and_then(|d| d.get("status")).and_then(|v| v.as_str()).unwrap_or("");
    if status != "SUCCEEDED" {
        eprintln!("[apify] run ended {}", status);
        return None;
    }
    if ds.is_empty() {
        ds = run.get("data").and_then(|d| d.get("defaultDatasetId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    }
    let (code, text) = http_get(&format!("{}/datasets/{}/items?{}&limit={}", APIFY_API, ds, q, limit), WORKER_UA, 60);
    if code != 200 {
        eprintln!("[apify] dataset fetch failed: HTTP {}", code);
        return None;
    }
    serde_json::from_str(&text).ok()
}

fn first_str(d: &serde_json::Value, names: &[&str]) -> String {
    for n in names {
        if let Some(v) = d.get(*n).and_then(|v| v.as_str()) {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
    }
    String::new()
}

fn apify_search(settings: &serde_json::Value, query: &str, count: u32) -> Option<Vec<serde_json::Value>> {
    let (token, actor_id) = apify_leg(settings, "search")?;
    let items = apify_run(
        &actor_id,
        &token,
        serde_json::json!({"query": query, "q": query, "searchQuery": query, "keyword": query, "maxItems": count, "limit": count, "maxResults": count}),
        180,
        40,
    )?;
    let mut out: Vec<serde_json::Value> = Vec::new();
    for it in items {
        if !it.is_object() {
            continue;
        }
        let url = first_str(&it, &["url", "link", "href"]);
        let title = first_str(&it, &["title", "name", "heading"]);
        let snippet = first_str(&it, &["description", "snippet", "text", "content"]);
        if url.starts_with("http") {
            out.push(serde_json::json!({"title": title, "url": url, "source": "apify", "snippet": snippet.chars().take(400).collect::<String>()}));
        }
    }
    Some(out)
}

fn apify_reddit(settings: &serde_json::Value, query: &str, count: u32) -> Option<Vec<serde_json::Value>> {
    let (token, actor_id) = apify_leg(settings, "reddit")?;
    let items = apify_run(
        &actor_id,
        &token,
        serde_json::json!({"query": query, "q": query, "searchQuery": query, "searchTerm": query, "keyword": query, "maxItems": count, "limit": count, "maxPosts": count, "sort": "relevance"}),
        180,
        40,
    )?;
    let mut out: Vec<serde_json::Value> = Vec::new();
    for it in items {
        if !it.is_object() {
            continue;
        }
        let mut url = first_str(&it, &["url", "postUrl", "link", "permalink"]);
        if url.starts_with("/r/") {
            url = format!("https://www.reddit.com{}", url);
        }
        let title = first_str(&it, &["title", "name"]);
        let body = first_str(&it, &["body", "selfText", "text", "description"]);
        if url.starts_with("http") {
            out.push(serde_json::json!({"title": title, "url": url, "source": "reddit", "snippet": body.chars().take(400).collect::<String>(), "subreddit": first_str(&it, &["subreddit", "communityName"])}));
        }
    }
    Some(out)
}

// ------------------------------------------------------- combined / research ---

fn backend(settings: &serde_json::Value) -> String {
    let b = setting(settings, "backend").to_lowercase();
    if ["monid", "bing", "apify"].contains(&b.as_str()) {
        b
    } else {
        "monid".to_string()
    }
}

fn reddit_on(settings: &serde_json::Value) -> bool {
    settings.get("reddit").and_then(|v| v.as_bool()).unwrap_or(false)
}

fn combined_search(settings: &serde_json::Value, query: &str, purpose: Option<&str>) -> serde_json::Value {
    let mut urls: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut counts = serde_json::Map::new();
    let be = backend(settings);
    let mut add = |rows: Option<Vec<serde_json::Value>>, label: &str, urls: &mut Vec<serde_json::Value>, seen: &mut Vec<String>, counts: &mut serde_json::Map<String, serde_json::Value>| {
        let n = rows.as_ref().map(|r| r.len()).unwrap_or(0);
        if let Some(rows) = rows {
            for r in rows {
                if let Some(u) = r.get("url").and_then(|v| v.as_str()) {
                    if u.starts_with("http") && !seen.contains(&u.to_string()) {
                        seen.push(u.to_string());
                        urls.push(serde_json::json!({"title": r.get("title"), "url": u, "source": label, "snippet": r.get("snippet")}));
                    }
                }
            }
        }
        counts.insert(label.to_string(), serde_json::json!(n));
    };
    if be == "apify" {
        let rows = apify_search(settings, query, 20);
        if rows.is_none() {
            eprintln!("[apify] search leg unavailable — falling back to Monid");
            let r = if leg_on(settings, "tinyfish") { tinyfish_search(query, purpose) } else { None };
            add(r, "tinyfish", &mut urls, &mut seen, &mut counts);
        } else {
            add(rows, "apify", &mut urls, &mut seen, &mut counts);
        }
    } else if be == "bing" {
        let r = if leg_on(settings, "bing") { cfsearch_search(settings, query, "web", 10, None, None, None) } else { None };
        add(r, "cf-search", &mut urls, &mut seen, &mut counts);
    } else {
        let r = if leg_on(settings, "tinyfish") { tinyfish_search(query, purpose) } else { None };
        add(r, "tinyfish", &mut urls, &mut seen, &mut counts);
        let r = if leg_on(settings, "bing") { cfsearch_search(settings, query, "web", 10, None, None, None) } else { None };
        add(r, "cf-search", &mut urls, &mut seen, &mut counts);
    }
    let mut reddit_count = 0usize;
    if reddit_on(settings) {
        let reddit = apify_reddit(settings, query, 20).unwrap_or_default();
        reddit_count = reddit.len();
        add(Some(reddit), "reddit", &mut urls, &mut seen, &mut counts);
    }
    serde_json::json!({"web": urls, "counts": counts, "reddit_count": reddit_count, "backend": be})
}

fn research(settings: &serde_json::Value, query: &str, pages: u32, fetch_top: usize, search_dir: &std::path::Path) -> serde_json::Value {
    let combo = combined_search(settings, query, None);
    let mut x: Option<String> = None;
    if leg_on(settings, "x") {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| x_search(settings, query, pages))) {
            Ok(v) => x = v,
            Err(_) => eprintln!("[research] X search failed; continuing with web only"),
        }
    }
    let web = combo.get("web").and_then(|w| w.as_array()).cloned().unwrap_or_default();
    let top: Vec<String> = web
        .iter()
        .take(fetch_top)
        .filter_map(|r| r.get("url").and_then(|u| u.as_str()).map(|s| s.to_string()))
        .filter(|u| u.starts_with("http"))
        .collect();
    let (fetched, via) = if top.is_empty() {
        (Vec::new(), String::new())
    } else {
        fetch_urls(&top, search_dir)
    };
    serde_json::json!({
        "query": query,
        "backend": combo.get("backend"),
        "web_results": web,
        "counts": combo.get("counts"),
        "fetch_via": via,
        "x_pack_markdown": x,
        "fetched": fetched,
    })
}

// ------------------------------------------------------------------ cli ---

fn usage() -> ! {
    eprintln!("usage:\n  search search \"query\" [--purpose \"...\"]\n  search search-cf \"query\" [--vertical web] [--count 10] [--cc IN] [--setlang en-IN] [--time day]\n  search fetch https://example.com [https://...]\n  search xsearch \"query\" [--pages 2]\n  search xpost https://x.com/.../status/...\n  search xprofile <handle>\n  search xtrends\n  search reddit \"query\" [--count 20]\n  search research \"query\" [--pages 2] [--fetch-top 5]");
    std::process::exit(2);
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        if args[i] == name && i + 1 < args.len() {
            return Some(args[i + 1].clone());
        }
        if let Some(v) = args[i].strip_prefix(&format!("{}=", name)) {
            return Some(v.to_string());
        }
        i += 1;
    }
    None
}

fn flag_int(args: &[String], name: &str, default: u64) -> u64 {
    flag(args, name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn search_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            return dir.to_path_buf();
        }
    }
    PathBuf::from("/root/pi-bridge")
}

fn main() {
    let settings = load_settings();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
    }
    let cmd = argv[0].as_str();
    let rest = &argv[1..];
    let dir = search_dir();

    match cmd {
        "search" => {
            let query = rest.first().cloned().unwrap_or_else(|| usage());
            let purpose = flag(rest, "--purpose");
            let be = backend(&settings);
            let res: Option<Vec<serde_json::Value>> = if be == "apify" {
                let r = apify_search(&settings, &query, 20);
                if r.is_none() {
                    eprintln!("[apify] no search leg configured — falling back to Monid");
                    tinyfish_search(&query, purpose.as_deref())
                } else {
                    r
                }
            } else if be == "bing" {
                cfsearch_search(&settings, &query, "web", 10, None, None, None)
            } else {
                let r = tinyfish_search(&query, purpose.as_deref());
                if r.is_none() || r.as_ref().map(|v| v.is_empty()).unwrap_or(false) {
                    eprintln!("[tinyfish] no result, falling back to cf-search Worker");
                    cfsearch_search(&settings, &query, "web", 10, None, None, None)
                } else {
                    r
                }
            };
            println!("{}", serde_json::to_string_pretty(&res.unwrap_or_default()).unwrap_or_default());
        }
        "search-cf" => {
            let query = rest.first().cloned().unwrap_or_else(|| usage());
            let vertical = flag(rest, "--vertical").unwrap_or_else(|| "web".to_string());
            let count = flag_int(rest, "--count", 10) as u32;
            let setlang = flag(rest, "--setlang");
            let cc = flag(rest, "--cc");
            let time_range = flag(rest, "--time");
            let res = cfsearch_search(&settings, &query, &vertical, count, setlang.as_deref(), cc.as_deref(), time_range.as_deref());
            println!("{}", serde_json::to_string_pretty(&res.unwrap_or_default()).unwrap_or_default());
        }
        "fetch" => {
            if rest.is_empty() {
                usage();
            }
            let urls: Vec<String> = rest.to_vec();
            let (res, via) = fetch_urls(&urls, &dir);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"via": via, "results": res})).unwrap_or_default());
        }
        "xsearch" => {
            let query = rest.first().cloned().unwrap_or_else(|| usage());
            let pages = flag_int(rest, "--pages", 2) as u32;
            let res = x_search(&settings, &query, pages);
            println!("{}", res.unwrap_or_else(|| "no X results".to_string()));
        }
        "xpost" => {
            let u = rest.first().cloned().unwrap_or_else(|| usage());
            let res = x_post(&settings, &u);
            println!("{}", res.unwrap_or_else(|| "not found".to_string()));
        }
        "xprofile" => {
            let h = rest.first().cloned().unwrap_or_else(|| usage());
            let res = x_profile(&settings, &h);
            println!("{}", res.unwrap_or_else(|| "not found".to_string()));
        }
        "xtrends" => {
            let res = x_trends(&settings);
            println!("{}", serde_json::to_string_pretty(&res.unwrap_or(serde_json::Value::Null)).unwrap_or_default());
        }
        "reddit" => {
            let query = rest.first().cloned().unwrap_or_else(|| usage());
            let count = flag_int(rest, "--count", 20) as u32;
            let res = apify_reddit(&settings, &query, count);
            println!("{}", serde_json::to_string_pretty(&res.unwrap_or_default()).unwrap_or_default());
        }
        "research" => {
            let query = rest.first().cloned().unwrap_or_else(|| usage());
            let pages = flag_int(rest, "--pages", 2) as u32;
            let fetch_top = flag_int(rest, "--fetch-top", 5) as usize;
            let res = research(&settings, &query, pages, fetch_top, &dir);
            println!("{}", serde_json::to_string_pretty(&res).unwrap_or_default());
        }
        _ => usage(),
    }
}
