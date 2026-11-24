//! Minimal HTTP/1.1 server on std::net: one thread per connection, keep-alive,
//! Content-Length bodies. No framework, no async runtime — this gateway serves
//! one user over a tunnel, so the thread price is negligible and the RAM stays flat.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

pub struct Request {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }
    pub fn json(&self) -> serde_json::Value {
        if self.body.is_empty() {
            return serde_json::Value::Object(Default::default());
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
    }
    pub fn q(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(|s| s.as_str())
    }
}

pub struct Response {
    pub status: u16,
    pub ctype: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(status: u16, v: &serde_json::Value) -> Response {
        Response { status, ctype: "application/json", body: serde_json::to_vec(v).unwrap_or_default() }
    }
    pub fn html(body: Vec<u8>) -> Response {
        Response { status: 200, ctype: "text/html; charset=utf-8", body }
    }
}

pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

fn parse_query(q: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for pair in q.split('&') {
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

fn read_request(r: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    // Skip empty lines between pipelined requests.
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

    let n: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let cap = n.min(4 * 1024 * 1024);
    let mut body = vec![0u8; cap];
    if cap > 0 && r.read_exact(&mut body).is_err() {
        return None;
    }

    let (path, qs) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    Some(Request { method, path, query: parse_query(&qs), headers, body })
}

pub fn serve(addr: &str, handler: Handler) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    for conn in listener.incoming() {
        let stream = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        let h = handler.clone();
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
                let resp = h(&req);
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-cache\r\nConnection: {}\r\n\r\n",
                    resp.status,
                    status_text(resp.status),
                    resp.ctype,
                    resp.body.len(),
                    if keep { "keep-alive" } else { "close" }
                );
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

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}
