//! browser — CLI for the headed Camoufox browser on the Instabox box.
//!
//! Rust port of browser.py. Same commands, same arguments, same output.
//! Why a CLI and not MCP: Zerostack's ACP mode (the mode the zs-gateway
//! uses) sets `mcp_manager: None` — MCP servers from the config are NOT
//! loaded in ACP sessions, so the browser MCP tools never appear there.
//! This CLI talks to the Playwright MCP server over HTTP (via the localhost
//! relay on the Altare box) and is callable from Zerostack via bash, which
//! IS available in ACP mode. The browser skill (`/prompt browser`) teaches
//! Zerostack to use this CLI.
//!
//! The MCP session id is persisted in SESSION_FILE so consecutive CLI calls
//! share one browser/page. If the session expired, the CLI re-initialises
//! and retries.
//!
//! Usage:
//!   browser navigate <url>            open a URL, print title + url
//!   browser snapshot                  print the accessibility snapshot
//!   browser click <ref>               click element by snapshot ref
//!   browser type <ref> <text> [--submit]
//!                                     type into a field by ref
//!   browser screenshot [path]         save a screenshot (default /tmp/browser.png)
//!   browser eval <js>                 run JavaScript, print result
//!   browser wait <text>               wait for text to appear
//!   browser back                      go back
//!   browser close                     close the page/browser
//!   browser status                    show whether the MCP server answers

use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::exit;
use std::time::Duration;

fn mcp_url() -> String {
    env::var("BROWSER_MCP_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:8931/mcp".to_string())
}

fn session_file() -> String {
    env::var("BROWSER_SESSION_FILE")
        .unwrap_or_else(|_| "/tmp/browser-mcp-session.json".to_string())
}

fn prog() -> String {
    env::args()
        .next()
        .and_then(|a| {
            std::path::Path::new(&a)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "browser".to_string())
}

fn usage() -> String {
    let p = prog();
    format!(
        "browser — CLI for the headed Camoufox browser on the Instabox box.\n\
         \n\
         Usage:\n\
         \x20 {p} navigate <url>            open a URL, print title + url\n\
         \x20 {p} snapshot                  print the accessibility snapshot\n\
         \x20 {p} click <ref>               click element by snapshot ref\n\
         \x20 {p} type <ref> <text> [--submit]\n\
         \x20                                     type into a field by ref\n\
         \x20 {p} screenshot [path]         save a screenshot (default /tmp/browser.png)\n\
         \x20 {p} eval <js>                 run JavaScript, print result\n\
         \x20 {p} wait <text>               wait for text to appear\n\
         \x20 {p} back                      go back\n\
         \x20 {p} close                     close the page/browser\n\
         \x20 {p} status                    show whether the MCP server answers"
    )
}

// ---------- minimal HTTP client (std TcpStream) ----------

struct HttpResp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn http_post(url: &str, body: &str, session_id: Option<&str>) -> Result<HttpResp, String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs supported: {}", url))?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rfind(':') {
        Some(i) => (
            &hostport[..i],
            hostport[i + 1..]
                .parse::<u16>()
                .map_err(|_| format!("bad port in {}", url))?,
        ),
        None => (hostport, 80u16),
    };
    let stream = TcpStream::connect((host, port))
        .map_err(|e| format!("connect {}:{}: {}", host, port, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .ok();
    // NOTE: do NOT send "Connection: close". The Playwright MCP server
    // treats an explicit close as the client going away and deletes the
    // MCP session, so the next CLI call finds "Session not found" and
    // re-initialises to a blank page. Python's urllib doesn't send it
    // either. Instead we frame the body via Content-Length / chunked and
    // simply drop the stream after a complete response.
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        path,
        hostport,
        body.len()
    );
    if let Some(sid) = session_id {
        req.push_str(&format!("mcp-session-id: {}\r\n", sid));
    }
    req.push_str("\r\n");
    let mut reader = BufReader::new(stream);
    reader
        .get_mut()
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {}", e))?;
    reader
        .get_mut()
        .write_all(body.as_bytes())
        .map_err(|e| format!("write: {}", e))?;
    // Status line.
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| format!("read status: {}", e))?;
    let mut status = 0u16;
    if let Some(code) = line.split_whitespace().nth(1) {
        status = code.parse().unwrap_or(0);
    }
    // Headers, until the empty line.
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    loop {
        line.clear();
        reader
            .read_line(&mut line)
            .map_err(|e| format!("read header: {}", e))?;
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        if let Some((k, v)) = t.split_once(':') {
            let kl = k.trim().to_ascii_lowercase();
            let vv = v.trim().to_string();
            if kl == "content-length" {
                content_length = vv.parse().ok();
            }
            if kl == "transfer-encoding" && vv.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
            headers.push((kl, vv));
        }
    }
    // Body, framed — never read-to-EOF (keep-alive would block forever).
    let mut body_bytes = Vec::new();
    if chunked {
        loop {
            let mut size_line = String::new();
            reader
                .read_line(&mut size_line)
                .map_err(|e| format!("read chunk size: {}", e))?;
            let size_str = size_line.trim().split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_str, 16).unwrap_or(0);
            if size == 0 {
                // Trailers until the empty line.
                loop {
                    let mut tl = String::new();
                    reader
                        .read_line(&mut tl)
                        .map_err(|e| format!("read trailer: {}", e))?;
                    if tl.trim().is_empty() {
                        break;
                    }
                }
                break;
            }
            let mut chunk = vec![0u8; size];
            reader
                .read_exact(&mut chunk)
                .map_err(|e| format!("read chunk: {}", e))?;
            body_bytes.extend_from_slice(&chunk);
            // Trailing CRLF after the chunk.
            let mut crlf = [0u8; 2];
            let _ = reader.read_exact(&mut crlf);
        }
    } else if let Some(n) = content_length {
        let mut buf = vec![0u8; n];
        reader
            .read_exact(&mut buf)
            .map_err(|e| format!("read body: {}", e))?;
        body_bytes = buf;
    } else {
        reader
            .read_to_end(&mut body_bytes)
            .map_err(|e| format!("read: {}", e))?;
    }
    Ok(HttpResp {
        status,
        headers,
        body: body_bytes,
    })
}

// ---------- MCP (Streamable HTTP / JSON-RPC over SSE) ----------

/// Returns (result, session_id from the response header or the one passed in).
fn mcp_req(
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> Result<(Value, Option<String>), String> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    let resp = http_post(&mcp_url(), &body, session_id)?;
    let raw = String::from_utf8_lossy(&resp.body).into_owned();
    if resp.status >= 400 {
        let snippet: String = raw.chars().take(300).collect();
        return Err(format!("MCP HTTP {}: {}", resp.status, snippet));
    }
    let sid = resp
        .headers
        .iter()
        .find(|(k, _)| k == "mcp-session-id")
        .map(|(_, v)| v.clone())
        .or_else(|| session_id.map(|s| s.to_string()));
    // Streamable HTTP returns SSE lines: "event: message" + "data: {...}"
    let mut payload: Option<Value> = None;
    for line in raw.lines() {
        if let Some(d) = line.strip_prefix("data:") {
            payload = Some(
                serde_json::from_str(d.trim())
                    .map_err(|e| format!("bad SSE json: {}", e))?,
            );
            break;
        }
    }
    let payload = payload.ok_or_else(|| {
        let snippet: String = raw.chars().take(300).collect();
        format!("no SSE data in MCP response: {:?}", snippet)
    })?;
    if let Some(err) = payload.get("error") {
        let s = serde_json::to_string(err).unwrap_or_default();
        let snippet: String = s.chars().take(300).collect();
        return Err(format!("MCP error: {}", snippet));
    }
    Ok((
        payload.get("result").cloned().unwrap_or(Value::Null),
        sid,
    ))
}

fn load_session() -> Option<String> {
    let text = fs::read_to_string(session_file()).ok()?;
    serde_json::from_str::<Value>(&text)
        .ok()?
        .get("session_id")?
        .as_str()
        .map(|s| s.to_string())
}

fn save_session(sid: &str) {
    let _ = fs::write(
        session_file(),
        serde_json::to_string(&json!({"session_id": sid})).unwrap_or_default(),
    );
}

fn ensure_session() -> Result<String, String> {
    if let Some(sid) = load_session() {
        return Ok(sid);
    }
    let (_, sid) = mcp_req(
        "initialize",
        json!({
            "protocolVersion": "2025-03-26", "capabilities": {},
            "clientInfo": {"name": "browser-cli", "version": "1.0"}
        }),
        None,
    )?;
    let sid = sid.ok_or_else(|| "initialize returned no session id".to_string())?;
    // No notifications/initialized: the Playwright MCP server accepts
    // tools/call right after initialize, and sending the notification as a
    // request (with an id) makes it answer -32601 Method not found.
    save_session(&sid);
    Ok(sid)
}

fn call_tool(name: &str, args: Value) -> Result<Value, String> {
    let sid = ensure_session()?;
    match mcp_req(
        "tools/call",
        json!({"name": name, "arguments": args}),
        Some(&sid),
    ) {
        Ok((r, _)) => Ok(r),
        Err(e) => {
            // Session may have expired; re-initialise once and retry.
            if e.contains("404") || e.to_lowercase().contains("session") {
                let _ = fs::remove_file(session_file());
                let sid2 = ensure_session()?;
                let (r, _) = mcp_req(
                    "tools/call",
                    json!({"name": name, "arguments": args}),
                    Some(&sid2),
                )?;
                Ok(r)
            } else {
                Err(e)
            }
        }
    }
}

fn tool_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        for c in content {
            if c.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(t) = c.get("text").and_then(|t| t.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
    }
    parts.join("\n")
}

fn server_info_str(result: &Value, key: &str) -> String {
    result
        .get("serverInfo")
        .and_then(|s| s.get(key))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "None".to_string())
}

fn run(argv: &[String]) -> i32 {
    if argv.len() < 2 {
        println!("{}", usage());
        return 2;
    }
    let cmd = argv[1].as_str();
    let res: Result<i32, String> = (|| {
        match cmd {
            "navigate" => {
                if argv.len() < 3 {
                    println!("usage: {} navigate <url>", prog());
                    return Ok(2);
                }
                let r = call_tool("browser_navigate", json!({"url": argv[2]}))?;
                println!("{}", tool_text(&r));
            }
            "snapshot" => {
                let r = call_tool("browser_snapshot", json!({}))?;
                println!("{}", tool_text(&r));
            }
            "click" => {
                if argv.len() < 3 {
                    println!("usage: {} click <ref>", prog());
                    return Ok(2);
                }
                // browser_click requires `target` (the snapshot ref).
                let r = call_tool("browser_click", json!({"target": argv[2]}))?;
                println!("{}", tool_text(&r));
            }
            "type" => {
                if argv.len() < 4 {
                    println!("usage: {} type <ref> <text> [--submit]", prog());
                    return Ok(2);
                }
                // browser_type requires `target` (ref) + `text`.
                let mut args = json!({"target": argv[2], "text": argv[3]});
                if argv[4..].iter().any(|a| a == "--submit") {
                    args["submit"] = json!(true);
                }
                let r = call_tool("browser_type", args)?;
                println!("{}", tool_text(&r));
            }
            "screenshot" => {
                let path = argv.get(2).cloned().unwrap_or_else(|| "/tmp/browser.png".to_string());
                // browser_take_screenshot requires `type` + `scale`; `filename`
                // is where it saves (on the Instabox box, under the MCP cwd).
                let r = call_tool(
                    "browser_take_screenshot",
                    json!({"filename": path, "type": "png", "scale": "css"}),
                )?;
                println!("{}", tool_text(&r));
            }
            "eval" => {
                if argv.len() < 3 {
                    println!("usage: {} eval <js>", prog());
                    return Ok(2);
                }
                let r = call_tool("browser_evaluate", json!({"function": argv[2]}))?;
                println!("{}", tool_text(&r));
            }
            "wait" => {
                if argv.len() < 3 {
                    println!("usage: {} wait <text>", prog());
                    return Ok(2);
                }
                let r = call_tool("browser_wait_for", json!({"text": argv[2]}))?;
                println!("{}", tool_text(&r));
            }
            "back" => {
                let r = call_tool("browser_navigate_back", json!({}))?;
                println!("{}", tool_text(&r));
            }
            "close" => {
                let r = call_tool("browser_close", json!({}))?;
                println!("{}", tool_text(&r));
            }
            "status" => {
                let (result, _) = mcp_req(
                    "initialize",
                    json!({
                        "protocolVersion": "2025-03-26", "capabilities": {},
                        "clientInfo": {"name": "browser-cli", "version": "1.0"}
                    }),
                    None,
                )?;
                println!(
                    "mcp ok: {} {}",
                    server_info_str(&result, "name"),
                    server_info_str(&result, "version")
                );
            }
            _ => {
                println!("unknown command: {}", cmd);
                println!("{}", usage());
                return Ok(2);
            }
        }
        Ok(0)
    })();
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", e);
            1
        }
    }
}

fn main() {
    let argv: Vec<String> = env::args().collect();
    exit(run(&argv));
}
