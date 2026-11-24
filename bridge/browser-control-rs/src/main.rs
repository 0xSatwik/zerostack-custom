//! Browser takeover control page + Stop/Start for the Instabox browser box.
//! Rust port of bridge/browser-control.py (B4). Serves on 0.0.0.0:6081.
//! Password-protected: the login accepts the gateway token OR the opening
//! password (the same two that unlock /pi and /admin). A session cookie keeps
//! the user logged in. The VNC iframe only renders after login.
//!
//! Stop/Start run /data/workspace/stop-browser.sh and
//! /data/workspace/browser-stack.sh (both must exist and be executable —
//! they were missing, which is why the buttons silently did nothing).
mod sha256;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VNC_URL: &str = "/browser-vnc/vnc.html?autoconnect=true&resize=remote";
const PORT: u16 = 6081;
const TOKEN_FILE: &str = "/data/workspace/browser-auth.txt";
const PASS_HASH_FILE: &str = "/data/workspace/browser-pass-hash.txt";
const STOP_SCRIPT: &str = "/data/workspace/stop-browser.sh";
const START_SCRIPT: &str = "/data/workspace/browser-stack.sh";
const BROWSER_PORT: u16 = 9377;

// ---------------------------------------------------------------- auth ---

fn read_trimmed(path: &str) -> String {
    std::fs::read_to_string(path).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Constant-time comparison (same as hmac.compare_digest).
fn ct_eq(a: &str, b: &str) -> bool {
    let ab = a.as_bytes();
    let bb = b.as_bytes();
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..ab.len() {
        diff |= ab[i] ^ bb[i];
    }
    diff == 0
}

fn check_password(pw: &str) -> bool {
    // Accepts the gateway token directly, OR the opening password
    // (verified by comparing its SHA-256 to the stored hash).
    let tok = read_trimmed(TOKEN_FILE);
    if !tok.is_empty() && ct_eq(pw, &tok) {
        return true;
    }
    let ph = read_trimmed(PASS_HASH_FILE);
    if !ph.is_empty() {
        let pw_hash = sha256::digest_hex(pw.as_bytes());
        if ct_eq(&pw_hash, &ph) {
            return true;
        }
    }
    false
}

type Sessions = Arc<Mutex<HashSet<String>>>;

fn random_token() -> String {
    // 16 random bytes from /dev/urandom, hex-encoded (same as secrets.token_hex(16)).
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    } else {
        // Fallback: time + pid mixed (never the primary path on Linux).
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, x) in b.iter_mut().enumerate() {
            *x = ((t >> (i % 8 * 8)) & 0xff) as u8 ^ (std::process::id() as u8);
        }
    }
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

fn authed(req: &Request, sessions: &Sessions) -> bool {
    // Cookie session, or an Authorization: Bearer <token> header (for API).
    if let Some(cookie) = req.header("cookie") {
        for part in cookie.split(';') {
            let part = part.trim();
            if let Some(tok) = part.strip_prefix("bsess=") {
                if sessions.lock().map(|s| s.contains(tok)).unwrap_or(false) {
                    return true;
                }
            }
        }
    }
    if let Some(auth) = req.header("authorization") {
        if let Some(tok) = auth.strip_prefix("Bearer ") {
            return check_password(tok.trim());
        }
    }
    false
}

// -------------------------------------------------------------- http ---

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }
    /// Form-encoded body (the login form) or JSON, as a flat map.
    fn form(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        let text = String::from_utf8_lossy(&self.body);
        // Try JSON first.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let serde_json::Value::Object(o) = v {
                for (k, v) in o {
                    if let Some(s) = v.as_str() {
                        m.insert(k, s.to_string());
                    }
                }
                return m;
            }
        }
        // Fall back to application/x-www-form-urlencoded.
        for pair in text.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            m.insert(percent_decode(k), percent_decode(v));
        }
        m
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hi = (b[i + 1] as char).to_digit(16);
                let lo = (b[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

struct Response {
    status: u16,
    ctype: &'static str,
    body: Vec<u8>,
    extra: Vec<(String, String)>,
}

impl Response {
    fn json(status: u16, v: &serde_json::Value) -> Response {
        Response {
            status,
            ctype: "application/json",
            body: serde_json::to_vec(v).unwrap_or_default(),
            extra: vec![],
        }
    }
    fn html(body: String) -> Response {
        Response { status: 200, ctype: "text/html; charset=utf-8", body: body.into_bytes(), extra: vec![] }
    }
    fn redirect(location: &str, set_cookie: &str) -> Response {
        Response {
            status: 302,
            ctype: "text/html; charset=utf-8",
            body: Vec::new(),
            extra: vec![
                ("Location".to_string(), location.to_string()),
                ("Set-Cookie".to_string(), set_cookie.to_string()),
            ],
        }
    }
}

fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    loop {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) if line.trim().is_empty() => continue,
            Ok(_) => break,
            Err(_) => return None,
        }
    }
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut h = String::new();
        match r.read_line(&mut h) {
            Ok(0) => break,
            Ok(_) => {
                let t = h.trim_end();
                if t.is_empty() {
                    break;
                }
                if let Some((k, v)) = t.split_once(':') {
                    headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
            Err(_) => break,
        }
    }
    let n: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let cap = n.min(4 * 1024 * 1024);
    let mut body = vec![0u8; cap];
    if cap > 0 && r.read_exact(&mut body).is_err() {
        return None;
    }
    let path = match target.split_once('?') {
        Some((p, _)) => p.to_string(),
        None => target,
    };
    Some(Request { method, path, headers, body })
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "OK",
    }
}

fn serve(addr: &str, sessions: Sessions) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("browser-control listening on {}", addr);
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        let sessions = sessions.clone();
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
            let _ = stream.set_nodelay(true);
            let read_half = match stream.try_clone() {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut out = stream;
            let mut r = BufReader::new(read_half);
            loop {
                let req = match read_request(&mut r) {
                    Some(q) => q,
                    None => break,
                };
                let keep = req
                    .header("connection")
                    .map(|c| !c.eq_ignore_ascii_case("close"))
                    .unwrap_or(true);
                let resp = handle(&req, &sessions);
                let mut head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: {}\r\n",
                    resp.status,
                    status_text(resp.status),
                    resp.ctype,
                    resp.body.len(),
                    if keep { "keep-alive" } else { "close" }
                );
                for (k, v) in &resp.extra {
                    head.push_str(&format!("{}: {}\r\n", k, v));
                }
                head.push_str("\r\n");
                if out.write_all(head.as_bytes()).is_err() {
                    break;
                }
                if out.write_all(&resp.body).is_err() {
                    break;
                }
                let _ = out.flush();
                if !keep {
                    break;
                }
            }
            let _ = out.shutdown(std::net::Shutdown::Both);
        });
    }
    Ok(())
}

// -------------------------------------------------------------- pages ---

const LOGIN_PAGE: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Browser — login</title>
<style>
body{margin:0;font-family:system-ui;background:#0b0d10;color:#e7eaee;display:flex;align-items:center;justify-content:center;min-height:100vh}
.card{background:#171a1f;border:1px solid #262b33;border-radius:12px;padding:24px;width:min(360px,90vw)}
h1{font-size:18px;margin:0 0 6px}.mut{color:#9aa3ad;font-size:13px;margin:0 0 16px}
input{width:100%;box-sizing:border-box;background:#0b0d10;color:#e7eaee;border:1px solid #262b33;border-radius:8px;padding:10px;font-size:15px}
button{width:100%;margin-top:12px;background:#2563eb;color:#fff;border:0;border-radius:8px;padding:10px;font-size:15px;cursor:pointer}
.err{color:#f87171;font-size:13px;min-height:18px;margin-top:8px}
</style></head><body><div class="card">
<h1>Browser login</h1>
<p class="mut">Enter the same gateway token (or opening password) you use for Chat and Admin.</p>
<form method="post" action="/browser/login"><input type="password" name="password" placeholder="Gateway token" autofocus>
<button type="submit">Unlock browser</button></form>
<div class="err">__ERR__</div>
</div></body></html>"#;

fn control_page() -> String {
    format!(r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Browser - muse-browser</title>
<style>
body{{margin:0;font-family:system-ui;background:#111;color:#eee}}
.bar{{display:flex;gap:8px;align-items:center;padding:8px 12px;background:#1c1c1c}}
.bar b{{font-size:15px}}
.bar span{{color:#999;font-size:13px}}
button{{padding:8px 16px;border:0;border-radius:8px;font-size:15px;cursor:pointer}}
#stop{{background:#c0392b;color:#fff}}
#start{{background:#27ae60;color:#fff}}
#status{{margin-left:auto;font-size:13px}}
iframe{{border:0;width:100vw;height:calc(100vh - 52px);display:block;background:#000}}
</style></head><body>
<div class="bar"><b>Browser</b><span>muse-browser &middot; Camoufox &middot; click / type to take over</span>
<button id="stop" onclick="act('stop')">Stop</button>
<button id="start" onclick="act('start')">Start</button>
<span id="status"></span></div>
<iframe id="v" src="{vnc}" allow="clipboard-read; clipboard-write"></iframe>
<script>
async function act(a){{
  document.getElementById('status').textContent = a + '...';
  try{{
    const r = await fetch('/browser/'+a,{{method:'POST'}});
    const j = await r.json();
    document.getElementById('status').textContent = j.ok ? (a+' ok') : ('error: '+(j.error||''));
    if(a==='stop'){{ document.getElementById('v').src='about:blank'; }}
    if(a==='start'){{ setTimeout(()=>{{ document.getElementById('v').src='{vnc}'; }}, 8000); }}
  }}catch(e){{ document.getElementById('status').textContent = 'error: '+e; }}
}}
</script></body></html>"#, vnc = VNC_URL)
}

// ------------------------------------------------------------ handler ---

fn strip_browser_prefix(path: &str) -> String {
    // The tunnel forwards the FULL path, so via /browser the paths arrive
    // as /browser, /browser/status, ... — strip that prefix so the same
    // handlers work both via the tunnel and direct.
    if let Some(rest) = path.strip_prefix("/browser") {
        if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        }
    } else {
        path.to_string()
    }
}

fn browser_port_up() -> bool {
    TcpStream::connect(("127.0.0.1", BROWSER_PORT)).is_ok()
}

fn handle(req: &Request, sessions: &Sessions) -> Response {
    let path = strip_browser_prefix(&req.path);
    let method = req.method.as_str();

    if method == "GET" && (path == "/" || path == "/index.html") {
        if !authed(req, sessions) {
            return Response::html(LOGIN_PAGE.replace("__ERR__", ""));
        }
        return Response::html(control_page());
    }
    if method == "GET" && path == "/status" {
        if !authed(req, sessions) {
            return Response::json(401, &serde_json::json!({"ok": false, "error": "unauthorized"}));
        }
        return Response::json(
            200,
            &serde_json::json!({"ok": true, "stack_ports_up": if browser_port_up() { 1 } else { 0 }}),
        );
    }
    if method == "POST" && path == "/login" {
        let body = req.form();
        let pw = body.get("password").map(|s| s.trim().to_string()).unwrap_or_default();
        if check_password(&pw) {
            let tok = random_token();
            if let Ok(mut s) = sessions.lock() {
                s.insert(tok.clone());
            }
            return Response::redirect(
                "/browser",
                &format!("bsess={}; Path=/; HttpOnly; SameSite=Lax", tok),
            );
        }
        return Response::html(LOGIN_PAGE.replace("__ERR__", "Wrong token — try again."));
    }
    if method == "POST" && (path == "/stop" || path == "/start") {
        if !authed(req, sessions) {
            return Response::json(401, &serde_json::json!({"ok": false, "error": "unauthorized"}));
        }
        let script = if path == "/stop" { STOP_SCRIPT } else { START_SCRIPT };
        let out = Command::new("bash").arg(script).output();
        match out {
            Ok(o) if o.status.success() => {
                return Response::json(200, &serde_json::json!({"ok": true}));
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
                return Response::json(
                    200,
                    &serde_json::json!({"ok": false, "error": if err.is_empty() { "script failed" } else { &err }}),
                );
            }
            Err(e) => {
                return Response::json(200, &serde_json::json!({"ok": false, "error": e.to_string()}));
            }
        }
    }
    Response::json(404, &serde_json::json!({"ok": false, "error": "not found"}))
}

fn main() {
    let sessions: Sessions = Arc::new(Mutex::new(HashSet::new()));
    let addr = format!("0.0.0.0:{}", PORT);
    if let Err(e) = serve(&addr, sessions) {
        eprintln!("browser-control: {}", e);
        std::process::exit(1);
    }
}
