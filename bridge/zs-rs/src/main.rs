//! zs — tiny client for the live Zerostack gateway (127.0.0.1:8766).
//! Rust port of bridge/zs.py (B4).
//!
//! The gateway owns the transcript store and replays a digest into a fresh
//! zerostack process when one dies, so talking to the gateway (not to a raw
//! `zerostack` process) is what makes sessions recoverable.
//!
//! Auth: the gateway token is read from /root/pi-bridge/gateway.env at
//! runtime. It is never printed, never written anywhere else, never passed
//! on a command line (it would show up in `ps`).
//!
//! Usage:
//!   zs list
//!   zs new <name> [--cwd DIR] [--model M] [--brief TEXT] [--wait SECS]
//!   zs ask <session-id|name> "message" [--wait SECS] [--cwd DIR] [--model M]
//!   zs wait <session-id|name> [--secs N]
//!   zs show <session-id|name> [--tail N]
//!   zs kill <session-id|name>
//!   zs status <session-id|name>

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const BASE_HOST: &str = "127.0.0.1";
const BASE_PORT: u16 = 8766;
const ENVF: &str = "/root/pi-bridge/gateway.env";

// ---------------------------------------------------------------- token ---

fn token() -> String {
    let content = std::fs::read_to_string(ENVF).unwrap_or_else(|e| {
        eprintln!("cannot read gateway env {}: {}", ENVF, e);
        std::process::exit(1);
    });
    for line in content.lines() {
        if let Some(v) = line.strip_prefix("PI_GATEWAY_TOKEN=") {
            return v.trim().to_string();
        }
    }
    eprintln!("no gateway token in {}", ENVF);
    std::process::exit(1);
}

// ------------------------------------------------------------ http client ---

struct HttpError {
    code: u16,
    body: String,
}

fn call(path: &str, method: &str, body: Option<&serde_json::Value>, timeout_secs: u64) -> Result<serde_json::Value, HttpError> {
    let tok = token();
    let body_bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default());
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nConnection: close\r\n",
        method, path, BASE_HOST, BASE_PORT, tok
    );
    if let Some(b) = &body_bytes {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");

    let mut stream = TcpStream::connect((BASE_HOST, BASE_PORT)).map_err(|e| HttpError {
        code: 0,
        body: format!("connect failed: {}", e),
    })?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(timeout_secs)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    stream.write_all(req.as_bytes()).map_err(|e| HttpError { code: 0, body: e.to_string() })?;
    if let Some(b) = &body_bytes {
        stream.write_all(b).map_err(|e| HttpError { code: 0, body: e.to_string() })?;
    }
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| HttpError { code: 0, body: e.to_string() })?;
    let text = String::from_utf8_lossy(&raw);
    // Split headers from body.
    let body_start = text.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let head = &text[..body_start.min(text.len())];
    let body_text = &text[body_start.min(text.len())..];
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(HttpError { code: status, body: body_text.chars().take(800).collect() });
    }
    match serde_json::from_str(body_text) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::json!({"_raw": body_text.chars().take(800).collect::<String>()})),
    }
}

fn get(path: &str) -> serde_json::Value {
    match call(path, "GET", None, 120) {
        Ok(v) => v,
        Err(e) => serde_json::json!({"_http_error": e.code, "body": e.body}),
    }
}

fn post(path: &str, body: &serde_json::Value) -> serde_json::Value {
    match call(path, "POST", Some(body), 120) {
        Ok(v) => v,
        Err(e) => serde_json::json!({"_http_error": e.code, "body": e.body}),
    }
}

// -------------------------------------------------------------- sessions ---

fn is_uuid(s: &str) -> bool {
    // [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lens = [8usize, 4, 4, 4, 12];
    parts.iter().zip(lens.iter()).all(|(p, l)| {
        p.len() == *l && p.chars().all(|c| c.is_ascii_hexdigit())
    })
}

fn resolve(r: &str) -> String {
    if is_uuid(r) {
        return r.to_string();
    }
    let list = get("/sessions");
    if let Some(arr) = list.as_array() {
        for sess in arr {
            if sess.get("name").and_then(|n| n.as_str()) == Some(r) {
                if let Some(id) = sess.get("id").and_then(|i| i.as_str()) {
                    return id.to_string();
                }
            }
        }
    }
    eprintln!("no session named {:?}", r);
    std::process::exit(1);
}

fn status(sid: &str) -> serde_json::Value {
    get(&format!("/sessions/{}/status", sid))
}

fn wait(sid: &str, secs: u64) -> serde_json::Value {
    let t0 = Instant::now();
    loop {
        let st = status(sid);
        if !st.get("running").and_then(|r| r.as_bool()).unwrap_or(false) {
            return st;
        }
        if t0.elapsed().as_secs() >= secs {
            let mut out = st;
            out["timeout"] = serde_json::json!(true);
            return out;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

// ---------------------------------------------------------------- render ---

fn render(content: &serde_json::Value) -> String {
    if let Some(s) = content.as_str() {
        return s.to_string();
    }
    if let Some(arr) = content.as_array() {
        let mut parts: Vec<String> = Vec::new();
        for block in arr {
            if !block.is_object() {
                continue;
            }
            let kind = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match kind {
                "text" => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        if !t.is_empty() {
                            parts.push(t.to_string());
                        }
                    }
                }
                "thinking" => {
                    let t = block
                        .get("thinking")
                        .or_else(|| block.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    parts.push(format!("[thinking] {}", t));
                }
                "toolCall" => {
                    let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let args = block.get("arguments").map(|a| {
                        let s = serde_json::to_string(a).unwrap_or_default();
                        s.chars().take(200).collect::<String>()
                    }).unwrap_or_default();
                    parts.push(format!("[tool {}] {}", name, args));
                }
                _ => {
                    parts.push(format!("[{}]", kind));
                }
            }
        }
        return parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n");
    }
    String::new()
}

// ------------------------------------------------------------------ cli ---

fn usage() -> ! {
    eprintln!(
        "usage:\n  zs list\n  zs new <name> [--cwd DIR] [--model M] [--brief TEXT] [--wait SECS]\n  zs ask <session-id|name> \"message\" [--wait SECS] [--cwd DIR] [--model M]\n  zs wait <session-id|name> [--secs N]\n  zs show <session-id|name> [--tail N]\n  zs kill <session-id|name>\n  zs status <session-id|name>"
    );
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

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
    }
    let cmd = argv[0].as_str();
    let rest = &argv[1..];

    match cmd {
        "list" => {
            let list = get("/sessions");
            if let Some(arr) = list.as_array() {
                for sess in arr {
                    let id = sess.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let name = sess.get("name").and_then(|v| v.as_str()).unwrap_or("-");
                    let msgs = sess.get("messages").map(|v| v.to_string()).unwrap_or_default();
                    let running = sess.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
                    let live = sess.get("live").and_then(|v| v.as_bool()).unwrap_or(false);
                    let cwd = sess.get("cwd").and_then(|v| v.as_str()).unwrap_or("");
                    println!("{}  {:<18} msgs={:<4} running={} live={} cwd={}", id, name, msgs, running, live, cwd);
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
            }
        }
        "new" => {
            let name = rest.first().cloned().unwrap_or_else(|| usage());
            let cwd = flag(rest, "--cwd").unwrap_or_else(|| "/root/work".to_string());
            let model = flag(rest, "--model").unwrap_or_default();
            let brief = flag(rest, "--brief").unwrap_or_default();
            let wait_secs = flag_int(rest, "--wait", 600);
            let mut body = serde_json::json!({
                "message": if brief.is_empty() { "Session opened. Reply with one line: ready.".to_string() } else { brief },
                "name": name,
                "cwd": cwd,
            });
            if !model.is_empty() {
                body["model"] = serde_json::json!(model);
            }
            let res = post("/prompt", &body);
            let sid = res.get("session_id").and_then(|v| v.as_str()).map(|s| s.to_string());
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"id": sid, "name": name, "cwd": cwd, "response": res})).unwrap_or_default());
            if wait_secs > 0 {
                if let Some(sid) = sid {
                    let st = wait(&sid, wait_secs);
                    let s = serde_json::to_string(&st).unwrap_or_default();
                    println!("status: {}", s.chars().take(400).collect::<String>());
                }
            }
        }
        "ask" => {
            let r = rest.first().cloned().unwrap_or_else(|| usage());
            let msg = rest.get(1).cloned().unwrap_or_else(|| usage());
            let wait_secs = flag_int(rest, "--wait", 900);
            let cwd = flag(rest, "--cwd");
            let model = flag(rest, "--model");
            let sid = resolve(&r);
            let body = serde_json::json!({"message": msg});
            let res = post(&format!("/sessions/{}/prompt", sid), &body);
            let _ = (cwd, model);
            if res.get("_http_error").is_some() {
                println!("{}", serde_json::to_string_pretty(&res).unwrap_or_default());
                std::process::exit(1);
            }
            let sid2 = res.get("session_id").and_then(|v| v.as_str()).unwrap_or(&sid).to_string();
            let mut st = serde_json::json!({});
            if wait_secs > 0 {
                st = wait(&sid2, wait_secs);
            }
            let completed = if st.is_object() && !st.as_object().unwrap().is_empty() {
                !st.get("running").and_then(|v| v.as_bool()).unwrap_or(true)
            } else {
                false
            };
            let reply = st.get("last_assistant_text").and_then(|v| v.as_str()).map(|s| s.to_string());
            let timed_out = st.get("timeout").and_then(|v| v.as_bool()).unwrap_or(false);
            let out = serde_json::json!({
                "session_id": sid2,
                "result": res,
                "completed": completed,
                "reply": reply,
                "timed_out": timed_out,
            });
            let s = serde_json::to_string_pretty(&out).unwrap_or_default();
            println!("{}", s.chars().take(200000).collect::<String>());
            if let Some(rep) = &reply {
                if rep.starts_with("[error:") {
                    std::process::exit(1);
                }
            }
            if timed_out {
                std::process::exit(2);
            }
        }
        "wait" => {
            let r = rest.first().cloned().unwrap_or_else(|| usage());
            let secs = flag_int(rest, "--secs", 600);
            let sid = resolve(&r);
            let st = wait(&sid, secs);
            let s = serde_json::to_string_pretty(&st).unwrap_or_default();
            println!("{}", s.chars().take(2000).collect::<String>());
        }
        "show" => {
            let r = rest.first().cloned().unwrap_or_else(|| usage());
            let tail = flag_int(rest, "--tail", 6) as usize;
            let sid = resolve(&r);
            let data = get(&format!("/sessions/{}", sid));
            let msgs = data
                .get("messages")
                .and_then(|m| m.get("data"))
                .and_then(|d| d.get("messages"))
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            let start = msgs.len().saturating_sub(tail);
            for msg in &msgs[start..] {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
                let content = render(msg.get("content").unwrap_or(&serde_json::Value::Null));
                let indented = content.replace('\n', "\n    ");
                let clipped: String = indented.chars().take(700).collect();
                println!("[{}] {}", role, clipped);
            }
        }
        "kill" => {
            let r = rest.first().cloned().unwrap_or_else(|| usage());
            let sid = resolve(&r);
            let res = post(&format!("/sessions/{}/stop", sid), &serde_json::json!({}));
            let s = serde_json::to_string_pretty(&res).unwrap_or_default();
            println!("{}", s.chars().take(800).collect::<String>());
        }
        "status" => {
            let r = rest.first().cloned().unwrap_or_else(|| usage());
            let sid = resolve(&r);
            let st = status(&sid);
            let s = serde_json::to_string_pretty(&st).unwrap_or_default();
            println!("{}", s.chars().take(2000).collect::<String>());
        }
        _ => usage(),
    }
}
