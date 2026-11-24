//! Zerostack ACP client + session engine.
//!
//! Facts this mirrors from zerostack 1.8.6 (src/extras/acp/mod.rs):
//!   * only initialize / session/new / session/prompt exist; there is no
//!     session/cancel, no session/load, no session/list, no set_model.
//!   * session/prompt answers only when the turn ENDS; streaming arrives as
//!     session/update notifications.
//!   * the TCP listener accepts exactly once, so one process per session.
//! Consequence: the gateway owns the transcripts, replays a text digest into
//! the next prompt after a respawn, and "stop" means "end the process".
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::models;
use crate::util::{now, log};

pub const ACP_PORT_BASE: u16 = 7300;
pub const ACP_PORT_SPAN: u16 = 100;
pub const TURN_TIMEOUT: Duration = Duration::from_secs(3600);
pub const PREAMBLE_MAX: usize = 12000;
pub const IDLE_REAP_SECS: f64 = 900.0;
/// How many times a transient provider failure (503 / 429 / 502) is retried
/// against the SAME model before the fallback chain takes over.
pub const TRANSIENT_RETRIES: u32 = 3;

struct Conn {
    stream: Mutex<TcpStream>,
    pending: Mutex<HashMap<String, Sender<Value>>>,
}

pub struct State {
    pub name: String,
    pub model: String,
    pub mtime: f64,
    pub messages: Vec<Value>,
    pub events: Vec<Value>,
    pub last_assistant: String,
    pub replay_from: usize,
    pub needs_preamble: bool,
    pub turn: Option<Value>,
    pub proc: Option<Child>,
    /// provider-keys.env slot the running process was spawned with (rotation)
    pub key_env: Option<String>,
    pub port: Option<u16>,
    pub conn: Option<Arc<Conn>>,
    pub acp_sid: Option<String>,
    pub running: bool,
    pub closed: bool,
    pub deleted: bool,
    pub worker_started: bool,
    pub q: VecDeque<String>,
}

pub struct Session {
    pub id: String,
    pub cwd: String,
    pub created: f64,
    pub st: Mutex<State>,
    pub cv: Condvar,
}

impl Session {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn name(&self) -> String {
        self.lock().name.clone()
    }
    pub fn model(&self) -> String {
        self.lock().model.clone()
    }
    pub fn running(&self) -> bool {
        self.lock().running
    }
    pub fn live(&self) -> bool {
        self.lock().proc.is_some()
    }
    pub fn msg_count(&self) -> usize {
        self.lock().messages.len()
    }
    pub fn mtime(&self) -> f64 {
        self.lock().mtime
    }
}

pub fn sessions() -> &'static Mutex<HashMap<String, Arc<Session>>> {
    static S: std::sync::OnceLock<Mutex<HashMap<String, Arc<Session>>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        t.as_secs() as u32, (t.subsec_nanos() >> 16) as u16, (n & 0xfff) as u16,
        ((t.subsec_nanos() as u64 ^ n) & 0xffff) as u16, (t.as_nanos() as u64 ^ (n << 32)) & 0xffffffffffff)
}

pub fn sess_path(sid: &str) -> String {
    format!("{}/{}.json", crate::SESS_DIR, sid)
}

pub fn save_session(s: &Arc<Session>) {
    let st = s.lock();
    if st.deleted {
        return;
    }
    let data = json!({
        "id": s.id, "name": st.name, "cwd": s.cwd, "model": st.model,
        "created": s.created, "mtime": st.mtime, "replay_from": st.replay_from,
        "messages": st.messages, "last_assistant": st.last_assistant,
    });
    drop(st);
    let path = sess_path(&s.id);
    let tmp = format!("{}.tmp", path);
    if std::fs::write(&tmp, serde_json::to_vec(&data).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub fn new_session(cwd: &str, model: &str, name: &str) -> Arc<Session> {
    let sid = next_id();
    let s = Arc::new(Session {
        id: sid.clone(),
        cwd: if cwd.is_empty() { crate::WORK.to_string() } else { cwd.to_string() },
        created: now(),
        st: Mutex::new(State {
            name: if name.is_empty() { sid.clone() } else { name.to_string() },
            model: model.to_string(),
            mtime: now(),
            messages: Vec::new(),
            events: Vec::new(),
            last_assistant: String::new(),
            replay_from: 0,
            needs_preamble: false,
            turn: None,
            proc: None,
            key_env: None,
            port: None,
            conn: None,
            acp_sid: None,
            running: false,
            closed: false,
            deleted: false,
            worker_started: false,
            q: VecDeque::new(),
        }),
        cv: Condvar::new(),
    });
    sessions().lock().unwrap().insert(sid.clone(), s.clone());
    save_session(&s);
    start_worker(&s);
    s
}

pub fn log_event(s: &Arc<Session>, rec: Value) {
    let mut st = s.lock();
    if st.deleted {
        return;
    }
    st.events.push(rec.clone());
    let n = st.events.len();
    if n > 2000 {
        st.events.drain(0..n - 2000);
    }
    drop(st);
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}/{}.events.jsonl", crate::SESS_DIR, s.id))
    {
        let _ = writeln!(f, "{}", rec);
    }
}

fn ensure_assistant(st: &mut State) -> usize {
    if st.turn.is_none() {
        let (p, m) = models::split_model(&st.model);
        st.messages.push(json!({
            "role": "assistant", "content": [],
            "timestamp": now() * 1000.0,
            "model": m.unwrap_or_default(),
            "provider": p.unwrap_or_default(),
        }));
        st.turn = Some(json!({}));
    }
    st.messages.len() - 1
}

fn push_block(st: &mut State, kind: &str, text: &str) {
    let idx = ensure_assistant(st);
    let content = st.messages[idx]["content"].as_array_mut().unwrap();
    if let Some(last) = content.last_mut() {
        let same = last.get("type").and_then(|v| v.as_str()) == Some(kind);
        if same {
            last[kind] = json!(format!(
                "{}{}",
                last.get(kind).and_then(|v| v.as_str()).unwrap_or(""),
                text
            ));
            return;
        }
    }
    // Store the block WITH its type field. Without "type", the frontend
    // drops text blocks (it matches on b.type === "text") and only the
    // thinking block renders — the user sees "thinking" and no answer.
    // The type field also makes the merge above work (it compares
    // last.type to kind), so consecutive deltas collapse into one block.
    let mut m = serde_json::Map::new();
    m.insert("type".to_string(), json!(kind));
    m.insert(kind.to_string(), json!(text));
    content.push(serde_json::Value::Object(m));
}

// ---- ports ----------------------------------------------------------------
fn used_ports() -> &'static Mutex<HashSet<u16>> {
    static P: std::sync::OnceLock<Mutex<HashSet<u16>>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(HashSet::new()))
}

fn free_port() -> Option<u16> {
    let mut used = used_ports().lock().unwrap();
    for p in ACP_PORT_BASE..ACP_PORT_BASE + ACP_PORT_SPAN {
        if used.contains(&p) {
            continue;
        }
        if let Ok(l) = std::net::TcpListener::bind(("127.0.0.1", p)) {
            drop(l);
            used.insert(p);
            return Some(p);
        }
    }
    None
}

fn release_port(p: u16) {
    used_ports().lock().unwrap().remove(&p);
}

// ---- ACP rpc --------------------------------------------------------------
fn rpc(conn: &Arc<Conn>, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let rid = format!("zs-{:x}{:x}", now() as u64, N.fetch_add(1, Ordering::Relaxed));
    let (tx, rx) = channel();
    conn.pending.lock().unwrap().insert(rid.clone(), tx);
    let payload = serde_json::to_string(&json!({
        "jsonrpc": "2.0", "id": rid, "method": method, "params": params
    }))
    .unwrap_or_default();
    {
        let mut stream = conn.stream.lock().unwrap();
        if stream.write_all(payload.as_bytes()).is_err() || stream.write_all(b"\n").is_err() {
            conn.pending.lock().unwrap().remove(&rid);
            return Err(format!("ACP send failed on {}", method));
        }
        let _ = stream.flush();
    }
    match rx.recv_timeout(timeout) {
        Ok(msg) => {
            if let Some(err) = msg.get("error") {
                let m = err.get("message").and_then(|v| v.as_str()).unwrap_or("engine error");
                return Err(m.to_string());
            }
            Ok(msg)
        }
        Err(_) => {
            conn.pending.lock().unwrap().remove(&rid);
            Err(format!("{} timed out after {}s", method, timeout.as_secs()))
        }
    }
}

fn handle_update(s: &Arc<Session>, params: &Value) {
    let u = params.get("update").cloned().unwrap_or(json!({}));
    let kind = u.get("sessionUpdate").and_then(|v| v.as_str()).unwrap_or("");
    let text_of = |u: &Value| -> String {
        u.get("content").and_then(|c| c.get("text")).and_then(|t| t.as_str()).unwrap_or("").to_string()
    };
    match kind {
        "agent_message_chunk" => {
            let txt = text_of(&u);
            if txt.is_empty() {
                return;
            }
            {
                let mut st = s.lock();
                push_block(&mut st, "text", &txt);
                let la = format!("{}{}", st.last_assistant, txt);
                st.last_assistant = la[la.len().saturating_sub(4000)..].to_string();
            }
            log_event(s, json!({"type": "message_update",
                "assistantMessageEvent": {"type": "text_delta", "delta": txt}}));
        }
        "agent_thought_chunk" => {
            let txt = text_of(&u);
            if txt.is_empty() {
                return;
            }
            s.lock().map_push_thinking(&txt);
            log_event(s, json!({"type": "message_update",
                "assistantMessageEvent": {"type": "thinking_delta", "delta": txt}}));
        }
        "tool_call" => {
            let name = u.get("title").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            {
                let mut st = s.lock();
                let idx = ensure_assistant(&mut st);
                let args = u.get("rawInput").cloned().unwrap_or(json!({}));
                st.messages[idx]["content"].as_array_mut().unwrap().push(json!({
                    "type": "toolCall", "id": u.get("toolCallId").cloned().unwrap_or(json!("")),
                    "name": name, "arguments": args,
                }));
            }
            log_event(s, json!({"type": "message_update",
                "assistantMessageEvent": {"type": "toolcall_start", "toolName": name,
                    "id": u.get("toolCallId").cloned().unwrap_or(json!(""))}}));
            log_event(s, json!({"type": "tool_execution_start", "toolName": name}));
        }
        "tool_call_update" => {
            let mut text = String::new();
            if let Some(arr) = u.get("content").and_then(|v| v.as_array()) {
                for c in arr {
                    if let Some(inner) = c.get("content") {
                        if inner.get("type").and_then(|v| v.as_str()) == Some("text") {
                            text.push_str(inner.get("text").and_then(|v| v.as_str()).unwrap_or(""));
                        }
                    } else if c.get("type").and_then(|v| v.as_str()) == Some("text") {
                        text.push_str(c.get("text").and_then(|v| v.as_str()).unwrap_or(""));
                    }
                }
            }
            let failed = u.get("status").and_then(|v| v.as_str()) == Some("failed");
            {
                let mut st = s.lock();
                st.messages.push(json!({
                    "role": "toolResult",
                    "toolCallId": u.get("toolCallId").cloned().unwrap_or(json!("")),
                    "content": [{"type": "text", "text": text}],
                    "isError": failed,
                    "timestamp": now() * 1000.0,
                }));
            }
            let tname = u.get("title").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
            log_event(s, json!({"type": "tool_execution_end", "toolName": tname}));
            log_event(s, json!({"type": "message_update",
                "assistantMessageEvent": {"type": "toolcall_end"}}));
        }
        _ => {}
    }
}

impl State {
    fn map_push_thinking(&mut self, txt: &str) {
        let idx = ensure_assistant(self);
        let content = self.messages[idx]["content"].as_array_mut().unwrap();
        if let Some(last) = content.last_mut() {
            if last.get("type").and_then(|v| v.as_str()) == Some("thinking") {
                let prev = last.get("thinking").and_then(|v| v.as_str()).unwrap_or("").to_string();
                last["thinking"] = json!(format!("{}{}", prev, txt));
                return;
            }
        }
        content.push(json!({"type": "thinking", "thinking": txt}));
    }
}

fn reader_thread(s: Arc<Session>, conn: Arc<Conn>, stream: TcpStream) {
    let r = BufReader::new(stream);
    for line in r.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = msg.get("id").cloned();
        if msg.get("method").is_some() && id.is_some() {
            // server -> client request (e.g. session/request_permission)
            let reply = json!({"jsonrpc": "2.0", "id": id.unwrap(),
                "result": {"outcome": {"outcome": "selected", "optionId": "allow"}}});
            if let Ok(mut st) = conn.stream.lock() {
                let _ = st.write_all(format!("{}\n", reply).as_bytes());
            }
        } else if let Some(id) = id {
            if let Some(key) = id.as_str() {
                if let Some(tx) = conn.pending.lock().unwrap().remove(key) {
                    let _ = tx.send(msg.clone());
                }
            }
        } else if msg.get("method").and_then(|v| v.as_str()) == Some("session/update") {
            let p = msg.get("params").cloned().unwrap_or(json!({}));
            handle_update(&s, &p);
        }
    }
    // connection ended: wake every waiter, mark the session idle again
    conn.pending.lock().unwrap().clear();
    s.lock().running = false;
}

// ---- spawn / kill ---------------------------------------------------------
fn spawn_proc(s: &Arc<Session>, forced_model: &str) -> Result<(), String> {
    {
        let st = s.lock();
        if st.closed || st.deleted {
            return Err("session closed".into());
        }
    }
    let cfg = crate::zs_config();
    let want = if forced_model.is_empty() { s.model() } else { forced_model.to_string() };
    let (mut provider, mut model) = models::split_model(&want);
    if provider.is_none() {
        provider = cfg.get("provider").and_then(|v| v.as_str()).map(String::from);
    }
    if model.is_none() {
        model = cfg.get("model").and_then(|v| v.as_str()).map(String::from);
    }
    let port = free_port().ok_or("no free ACP port")?;

    let mut cmd = Command::new(crate::ZS_BIN);
    cmd.arg("--acp")
        .arg("--acp-host").arg("127.0.0.1")
        .arg("--acp-port").arg(port.to_string());
    if let Some(p) = &provider {
        cmd.arg("--provider").arg(p);
    }
    if let Some(m) = &model {
        cmd.arg("--model").arg(m);
    }
    // Feature toggles -> spawn flags. features.json is the single source; a
    // feature the user turned off is actively disabled here rather than left
    // to a config default.
    if crate::feature_on("hooks") {
        // hooks are on by default in zerostack; nothing to pass
    } else {
        cmd.arg("--no-hooks");
    }
    if crate::feature_on("advisor") {
        cmd.arg("--advisor");
        if let Some(am) = cfg
            .get("advisor")
            .and_then(|a| a.get("model"))
            .and_then(|v| v.as_str())
        {
            cmd.arg("--advisor-model").arg(am);
        }
    }
    if crate::feature_on("status_signals") {
        cmd.arg("--status-socket")
            .arg(format!("{}/{}.status.sock", crate::SESS_DIR, s.id));
    }
    cmd.current_dir(&s.cwd).stdin(Stdio::null());

    let logpath = format!("{}/{}.log", crate::SESS_DIR, s.id);
    let logf = std::fs::OpenOptions::new().create(true).append(true).open(&logpath).ok();
    if let Some(f) = logf {
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(Stdio::from(f2));
        }
        cmd.stderr(Stdio::from(f));
    }

    // Key rotation: point the provider's api_key_env at the next HEALTHY key
    // for this spawn, round-robin, counter persisted in key-rotation.json.
    // Cooling / flagged keys are skipped; if none is left the spawn fails
    // loudly instead of silently burning a dead key.
    let mut used_key: Option<String> = None;
    if let Some(prov) = &provider {
        let reg = models::registry();
        if let Some(p) = reg.provider(prov) {
            let base = p.get("api_key_env").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !base.is_empty() {
                let keys = models::provider_keys(&base);
                match models::rotation_pick(prov, &keys) {
                    Ok(Some(pick)) => {
                        if let Some(val) = models::key_value(&pick) {
                            cmd.env(&base, val);
                            used_key = Some(pick);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => return Err(e),
                }
            }
        }
    }

    let mut child = cmd.spawn().map_err(|e| format!("could not start zerostack: {}", e))?;

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut sock: Option<TcpStream> = None;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            release_port(port);
            return Err(format!("zerostack exited immediately (see {}.log)", s.id));
        }
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
            sock = Some(stream);
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    let stream = match sock {
        Some(x) => x,
        None => {
            let _ = child.kill();
            release_port(port);
            return Err("zerostack ACP port never opened".into());
        }
    };
    let _ = stream.set_nodelay(true);
    let read_half = match stream.try_clone() {
        Ok(x) => x,
        Err(e) => {
            let _ = child.kill();
            release_port(port);
            return Err(format!("socket clone: {}", e));
        }
    };
    let conn = Arc::new(Conn { stream: Mutex::new(stream), pending: Mutex::new(HashMap::new()) });

    {
        let mut st = s.lock();
        if st.closed || st.deleted {
            drop(st);
            let _ = child.kill();
            release_port(port);
            return Err("session closed during spawn".into());
        }
        st.proc = Some(child);
        st.port = Some(port);
        st.conn = Some(conn.clone());
        st.acp_sid = None;
        st.key_env = used_key.clone();
        if !st.messages.is_empty() {
            st.needs_preamble = true;
        }
    }

    let sc = s.clone();
    let cc = conn.clone();
    std::thread::spawn(move || reader_thread(sc, cc, read_half));

    let init = json!({
        "protocolVersion": 1,
        "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false},
                               "terminal": false}
    });
    rpc(&conn, "initialize", init, Duration::from_secs(25))?;
    let r = rpc(&conn, "session/new", json!({"cwd": s.cwd, "mcpServers": []}), Duration::from_secs(25))?;
    let sid = r
        .get("result")
        .and_then(|v| v.get("sessionId"))
        .and_then(|v| v.as_str())
        .map(String::from);
    s.lock().acp_sid = sid;
    log(&format!(
        "session {}: zerostack up on 127.0.0.1:{} (model {}/{})",
        s.id, port,
        provider.unwrap_or_default(), model.unwrap_or_default()
    ));
    Ok(())
}

pub fn kill_proc(s: &Arc<Session>) {
    let (conn, mut child, port) = {
        let mut st = s.lock();
        st.acp_sid = None;
        st.key_env = None;
        (st.conn.take(), st.proc.take(), st.port.take())
    };
    if let Some(c) = conn {
        if let Ok(st) = c.stream.lock() {
            let _ = st.shutdown(Shutdown::Both);
        }
        c.pending.lock().unwrap().clear();
    }
    if let Some(ch) = child.as_mut() {
        if ch.try_wait().ok().flatten().is_none() {
            let _ = ch.kill();
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match ch.try_wait() {
                    Ok(Some(_)) => break,
                    _ => std::thread::sleep(Duration::from_millis(40)),
                }
            }
        }
    }
    if let Some(p) = port {
        release_port(p);
    }
}

fn ensure_proc(s: &Arc<Session>, forced_model: &str) -> Result<(), String> {
    {
        let mut st = s.lock();
        if st.closed || st.deleted {
            return Err("session closed".into());
        }
        if st.conn.is_some() && st.proc.is_some() {
            let alive = st.proc.as_mut().map(|c| c.try_wait().ok().flatten().is_none()).unwrap_or(false);
            if alive {
                drop(st);
                return Ok(());
            }
        }
    }
    kill_proc(s);
    spawn_proc(s, forced_model)
}

fn preamble(s: &Arc<Session>, upto: usize) -> String {
    let st = s.lock();
    let end = upto.min(st.messages.len());
    let from = st.replay_from.min(end);
    let mut lines: Vec<String> = Vec::new();
    for m in &st.messages[from..end] {
        let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }
        let mut txt = String::new();
        if let Some(arr) = m.get("content").and_then(|v| v.as_array()) {
            for b in arr {
                if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                    txt.push_str(b.get("text").and_then(|v| v.as_str()).unwrap_or(""));
                }
            }
        }
        let txt = txt.trim().to_string();
        if txt.is_empty() {
            continue;
        }
        let cut: String = txt.chars().take(2000).collect();
        lines.push(format!("{}: {}", if role == "user" { "user" } else { "assistant" }, cut));
    }
    let body = lines.join("\n\n");
    if body.is_empty() {
        return String::new();
    }
    let mut tail = body;
    if tail.len() > PREAMBLE_MAX {
        // cut on a char boundary
        let mut cut = tail.len() - PREAMBLE_MAX;
        while cut < tail.len() && !tail.is_char_boundary(cut) {
            cut += 1;
        }
        tail = tail[cut..].to_string();
    }
    format!("[Earlier conversation in this chat — context only, do not repeat it]\n{}\n[End of earlier conversation]", tail)
}

fn drop_empty_turn(s: &Arc<Session>) {
    let mut st = s.lock();
    if let Some(last) = st.messages.last() {
        if last.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            let empty = last
                .get("content")
                .and_then(|v| v.as_array())
                .map(|a| {
                    !a.iter().any(|b| {
                        b.get("text").and_then(|v| v.as_str()).map(|t| !t.is_empty()).unwrap_or(false)
                            || b.get("thinking").and_then(|v| v.as_str()).map(|t| !t.is_empty()).unwrap_or(false)
                            || b.get("type").and_then(|v| v.as_str()) == Some("toolCall")
                    })
                })
                .unwrap_or(true);
            if empty {
                st.messages.pop();
                st.turn = None;
            }
        }
    }
}

/// A provider 5xx/429 does not reach us as a JSON-RPC error: zerostack replies
/// with a successful `session/prompt` result carrying stopReason "refusal" and
/// streams the failure to the client as an `[error: ...]` message chunk. So the
/// retry decision has to be made from the turn body, not from `rpc`'s Result.
fn refusal_is_failure(msg: &Value) -> bool {
    msg.get("result")
        .and_then(|r| r.get("stopReason"))
        .and_then(|v| v.as_str())
        .map(|r| r == "refusal" || r == "error")
        .unwrap_or(false)
}

/// How long the provider asked us to wait, in seconds (default 20).
fn retry_after_secs(text: &str) -> u64 {
    let m = text.to_lowercase();
    for key in ["\"retry_after_s\":", "retry_after_s=", "retry-after:"] {
        if let Some(i) = m.find(key) {
            let tail = &m[i + key.len()..];
            let digits: String = tail
                .trim_start_matches(' ')
                .trim_start_matches('"')
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u64>() {
                if n > 0 && n <= 600 {
                    return n;
                }
            }
        }
    }
    20
}

/// Is this provider error worth retrying against the SAME model (busy upstream,
/// rate limit, gateway hiccup)? Anything else is a model/key problem and should
/// go straight to the fallback chain.
fn transient_failure(text: &str) -> bool {
    let m = text.to_lowercase();
    let has = |s: &str| m.contains(s);
    has("status 503")
        || has("status 502")
        || has("status 429")
        || has("status 500")
        || has("service unavailable")
        || has("upstream_busy")
        || has("too many requests")
        || has("at capacity")
        || has("rate limit")
}

/// Everything the failed attempt wrote into the transcript, so it can be shown
/// in one clean retry line and then discarded.
fn attempt_text(s: &Arc<Session>, mark: usize) -> String {
    let st = s.lock();
    let mut out = String::new();
    for m in st.messages.iter().skip(mark) {
        if let Some(arr) = m.get("content").and_then(|v| v.as_array()) {
            for b in arr {
                if let Some(t) = b.get("text").and_then(|v| v.as_str()) {
                    out.push_str(t);
                }
            }
        }
    }
    out.trim().to_string()
}

/// Roll the transcript back to before a failed attempt, so a retry does not
/// leave three copies of the same provider error in the user's chat.
fn drop_failed_attempt(s: &Arc<Session>, mark: usize) {
    let mut st = s.lock();
    if st.messages.len() > mark {
        st.messages.truncate(mark);
    }
    st.turn = None;
    st.last_assistant.clear();
}

fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

fn note_delta(s: &Arc<Session>, text: &str) {
    {
        let mut st = s.lock();
        let idx = ensure_assistant(&mut st);
        st.messages[idx]["content"].as_array_mut().unwrap().push(json!({"type": "text", "text": text}));
    }
    log_event(s, json!({"type": "message_update",
        "assistantMessageEvent": {"type": "text_delta", "delta": text}}));
}

pub fn run_turn(s: &Arc<Session>, text: &str) {
    {
        let mut st = s.lock();
        st.running = true;
        RUNNING_COUNT.fetch_add(1, Ordering::SeqCst);
        st.turn = None;
        st.last_assistant.clear();
        st.messages.push(json!({
            "role": "user", "content": [{"type": "text", "text": text}],
            "timestamp": now() * 1000.0,
        }));
    }
    log_event(s, json!({"type": "agent_start"}));

    // Fallback chain: the session model, then the registry order (or only the
    // session model when the toggle is off). Zerostack has no built-in
    // fallback, so the gateway walks the chain itself: a spawn or turn failure
    // for one model moves to the next.
    let chain = models::fallback_chain(&s.model());
    let mut last_err = String::new();
    let mut ok = false;
    for (i, cand) in chain.iter().enumerate() {
        let idx = {
            let st = s.lock();
            st.messages.len().saturating_sub(1)
        };
        let pref = {
            let mut st = s.lock();
            let p = if st.needs_preamble { preamble(s, idx) } else { String::new() };
            st.needs_preamble = false;
            p
        };
        let payload = if pref.is_empty() { text.to_string() } else { format!("{}\n\n{}", pref, text) };

        // Key retry: if the key this attempt used died in a key-class way
        // (auth / 429 / 5xx / timeout), mark it and run the SAME model again
        // with the provider's next healthy key before falling back on models.
        let mut attempts = 0;
        // Transient (503/429/502) retries for THIS model, reset per candidate.
        let mut transient = 0;
        loop {
            attempts += 1;
            let used_key = {
                match ensure_proc(s, cand) {
                    Ok(()) => s.lock().key_env.clone(),
                    Err(e) => {
                        last_err = e.clone();
                        log(&format!("session {}: spawn for {} failed: {}", s.id, cand, e));
                        if attempts >= 3 || i + 1 >= chain.len() {
                            note_delta(s, &format!("\n[{} failed: {}]\n", cand, e));
                        } else {
                            note_delta(s, &format!("\n[{} failed: {} — trying {}]\n", cand, e, chain[i + 1]));
                        }
                        break;
                    }
                }
            };
            let conn = { s.lock().conn.clone() };
            let acp_sid = { s.lock().acp_sid.clone() };
            let conn = match conn {
                Some(c) => c,
                None => {
                    last_err = "no ACP connection".to_string();
                    kill_proc(s);
                    break;
                }
            };
            let params = json!({
                "sessionId": acp_sid.unwrap_or_default(),
                "prompt": [{"type": "text", "text": payload.clone()}],
            });
            // Where this attempt starts in the transcript, so a retry can
            // discard the failed attempt's partial output instead of stacking
            // three copies of the same provider error in the user's chat.
            let mark = { s.lock().messages.len() };
            let res = rpc(&conn, "session/prompt", params, TURN_TIMEOUT);
            match res {
                Ok(msg) if !refusal_is_failure(&msg) => {
                    if let Some(k) = &used_key {
                        models::mark_key_success(k);
                    }
                    ok = true;
                    break;
                }
                res => {
                    // A refusal carries the provider error in the message
                    // chunks it already streamed; a transport error carries it
                    // in the Result. Both are the same thing to us here.
                    let e = match &res {
                        Ok(_) => {
                            let t = attempt_text(s, mark);
                            if t.is_empty() { "the provider refused the turn".to_string() } else { t }
                        }
                        Err(err) => err.clone(),
                    };
                    last_err = e.clone();
                    log(&format!("session {}: model {} failed: {}", s.id, cand, e));

                    // 503 / 429 / 502 from the provider: the request itself is
                    // fine, the upstream is busy. Wait what it asked for and
                    // run the same model again before burning a fallback slot.
                    if transient_failure(&e) && transient < TRANSIENT_RETRIES {
                        transient += 1;
                        drop_failed_attempt(s, mark);
                        // The zerostack session already recorded this prompt and
                        // never got an assistant reply, so re-sending to the same
                        // process would stack two user messages (providers reject
                        // that). Respawn instead; `payload` above already carries
                        // the replayed history, so no second digest is needed.
                        kill_proc(s);
                        let wait = retry_after_secs(&e).min(60);
                        note_delta(s, &format!(
                            "\n[{}: {} — retry {}/{} in {}s]\n",
                            cand, one_line(&e, 160), transient, TRANSIENT_RETRIES, wait
                        ));
                        std::thread::sleep(Duration::from_secs(wait));
                        continue;
                    }

                    kill_proc(s);
                    // Take a bad key out of rotation and retry the same model
                    // with another key, if there is one left.
                    let mut retried = false;
                    if let Some(k) = &used_key {
                        if models::classify_failure(&e).is_some() {
                            models::mark_key_failure(k, &e);
                            if let Some(prov) = cand.split_once('/').map(|(p, _)| p.to_string()) {
                                if let Some(base) = models::registry()
                                    .provider(&prov)
                                    .and_then(|p| p.get("api_key_env").and_then(|v| v.as_str()).map(String::from))
                                {
                                    if models::other_key_left(&base, k) && attempts < 4 {
                                        note_delta(s, &format!(
                                            "\n[{}: key {} failed ({}) — retrying with the next key]\n",
                                            cand, k, one_line(&e, 160)
                                        ));
                                        // A different key deserves its own
                                        // transient budget.
                                        transient = 0;
                                        retried = true;
                                    }
                                }
                            }
                        }
                    }
                    if retried {
                        continue;
                    }
                    if i + 1 < chain.len() {
                        note_delta(s, &format!(
                            "\n[{} failed: {} — falling back to {}]\n",
                            cand, one_line(&e, 160), chain[i + 1]
                        ));
                        // Start the next candidate in its own assistant block,
                        // so the transcript shows which model produced what.
                        s.lock().turn = None;
                    }
                    break;
                }
            }
        }
        if ok {
            break;
        }
    }
    if !ok {
        note_delta(s, &format!("\n[error: {}]", if last_err.is_empty() { "no model could run".into() } else { last_err }));
        s.lock().needs_preamble = true;
    }
    drop_empty_turn(s);
    {
        let mut st = s.lock();
        st.running = false;
        RUNNING_COUNT.fetch_sub(1, Ordering::SeqCst);
        st.turn = None;
        st.mtime = now();
        if st.closed || st.deleted {
            // DELETE landed mid-turn: sweep once more
        }
    }
    if { let st = s.lock(); st.closed || st.deleted } {
        kill_proc(s);
    }
    log_event(s, json!({"type": "agent_end"}));
    save_session(s);
}

fn worker(s: Arc<Session>) {
    loop {
        let item = {
            let mut st = s.lock();
            loop {
                if let Some(t) = st.q.pop_front() {
                    break Some(t);
                }
                if st.closed {
                    break None;
                }
                let (g, _t) = s.cv.wait_timeout(st, Duration::from_secs(1)).unwrap_or_else(|e| e.into_inner());
                st = g;
            }
        };
        let text = match item {
            Some(t) => t,
            None => break,
        };
        run_turn(&s, &text);
    }
}

pub fn start_worker(s: &Arc<Session>) {
    let mut st = s.lock();
    if st.worker_started {
        return;
    }
    st.worker_started = true;
    drop(st);
    let sc = s.clone();
    std::thread::spawn(move || worker(sc));
}

pub fn enqueue(s: &Arc<Session>, text: &str) -> bool {
    start_worker(s);
    let running = {
        let mut st = s.lock();
        st.mtime = now();
        st.q.push_back(text.to_string());
        st.running
    };
    s.cv.notify_all();
    running
}

pub fn load_sessions() {
    let mut entries: Vec<String> = std::fs::read_dir(crate::SESS_DIR)
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort();
    for fn_ in entries {
        let path = format!("{}/{}", crate::SESS_DIR, fn_);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let d: Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                log(&format!("load session {} failed: {}", fn_, e));
                continue;
            }
        };
        let sid = match d.get("id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mut events: Vec<Value> = Vec::new();
        if let Ok(t) = std::fs::read_to_string(format!("{}/{}.events.jsonl", crate::SESS_DIR, sid)) {
            let lines: Vec<&str> = t.lines().collect();
            let start = lines.len().saturating_sub(2000);
            for l in &lines[start..] {
                if let Ok(v) = serde_json::from_str::<Value>(l) {
                    events.push(v);
                }
            }
        }
        let s = Arc::new(Session {
            id: sid.clone(),
            cwd: d.get("cwd").and_then(|v| v.as_str()).unwrap_or(crate::WORK).to_string(),
            created: d.get("created").and_then(|v| v.as_f64()).unwrap_or_else(now),
            st: Mutex::new(State {
                name: d.get("name").and_then(|v| v.as_str()).unwrap_or(&sid).to_string(),
                model: d.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                mtime: d.get("mtime").and_then(|v| v.as_f64()).unwrap_or_else(now),
                messages: d.get("messages").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                events,
                last_assistant: d.get("last_assistant").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                replay_from: d.get("replay_from").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                needs_preamble: false,
                turn: None,
                proc: None,
                key_env: None,
                port: None,
                conn: None,
                acp_sid: None,
                running: false,
                closed: false,
                deleted: false,
                worker_started: false,
                q: VecDeque::new(),
            }),
            cv: Condvar::new(),
        });
        sessions().lock().unwrap().insert(sid, s);
    }
}

pub fn sessions_list() -> Value {
    let all: Vec<Arc<Session>> = sessions().lock().unwrap().values().cloned().collect();
    let mut out: Vec<Value> = all
        .iter()
        .map(|s| {
            let mut st = s.lock();
            let live = st.proc.as_mut().map(|c| c.try_wait().ok().flatten().is_none()).unwrap_or(false);
            json!({
                "id": s.id, "name": st.name, "cwd": s.cwd, "model": st.model,
                "messages": st.messages.len(), "mtime": st.mtime,
                "running": st.running,
                "live": live,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let ra = a.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
        let rb = b.get("running").and_then(|v| v.as_bool()).unwrap_or(false);
        rb.cmp(&ra).then_with(|| {
            let ma = a.get("mtime").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let mb = b.get("mtime").and_then(|v| v.as_f64()).unwrap_or(0.0);
            mb.partial_cmp(&ma).unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    json!(out)
}

/// Recall deletion marker. The auto-digest updater deletes a digest ONLY
/// when the session id is in this file — it is the proof the user actually
/// deleted the chat (via /pi or the CLI). Without a marker, a missing
/// session file means the file was LOST, and the digest is preserved.
/// Written here, in delete_session, the single place a user deletion happens.
const RECALL_DELETED_MARKER: &str = "/root/work/recall/digests/.deleted.json";

fn mark_session_deleted(sid: &str) {
    // Recall indexes by the short id (first 8 chars of the session id).
    let short: String = sid.chars().take(8).collect();
    let mut m: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(RECALL_DELETED_MARKER)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    m.insert(short, serde_json::json!(crate::util::now()));
    if let Ok(txt) = serde_json::to_string(&serde_json::Value::Object(m)) {
        let _ = std::fs::write(RECALL_DELETED_MARKER, txt + "\n");
    }
}

pub fn delete_session(sid: &str) -> bool {
    let s = sessions().lock().unwrap().remove(sid);
    if let Some(s) = s {
        {
            let mut st = s.lock();
            st.closed = true;
            st.deleted = true;
        }
        s.cv.notify_all();
        for _ in 0..6 {
            kill_proc(&s);
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    for suffix in [".json", ".json.tmp", ".events.jsonl", ".log"] {
        let _ = std::fs::remove_file(format!("{}/{}{}", crate::SESS_DIR, sid, suffix));
    }
    // Record the explicit user deletion so auto-digest knows this one is
    // really gone (vs a session file lost by the store, whose digest stays).
    mark_session_deleted(sid);
    true
}

pub fn stop_session(s: &Arc<Session>) -> bool {
    let was = s.lock().running;
    kill_proc(s);
    {
        let mut st = s.lock();
        st.running = false;
        st.turn = None;
        st.needs_preamble = true;
        st.mtime = now();
    }
    drop_empty_turn(s);
    if was {
        log_event(s, json!({"type": "agent_end"}));
    }
    save_session(s);
    was
}

pub fn idle_reaper() {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let all: Vec<Arc<Session>> = sessions().lock().unwrap().values().cloned().collect();
        for s in all {
            let (running, has_proc, idle) = {
                let st = s.lock();
                (st.running, st.proc.is_some(), now() - st.mtime)
            };
            if running || !has_proc || idle < IDLE_REAP_SECS {
                continue;
            }
            kill_proc(&s);
            s.lock().needs_preamble = true;
            log(&format!("session {}: idle, process released", s.id));
        }
    }
}

pub fn shutdown_all() {
    let all: Vec<Arc<Session>> = sessions().lock().unwrap().values().cloned().collect();
    for s in all {
        s.lock().closed = true;
        kill_proc(&s);
        save_session(&s);
    }
}

pub fn any_running() -> usize {
    // Lock-free: /health must answer even when a turn holds a session lock
    // or the sessions map is contended. The counter is bumped in run_turn
    // (exactly once per turn: +1 at start, -1 at end, no early returns).
    RUNNING_COUNT.load(Ordering::SeqCst)
}

/// Lock-free count of sessions with a turn in flight. Kept in sync by
/// run_turn only; a stale count is cosmetic (/health), never load-bearing.
static RUNNING_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
