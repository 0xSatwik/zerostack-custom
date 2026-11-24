//! Auto-digest updater for the recall system — Rust port of
//! recall/auto_digest.py (B5). Keeps digests + index current without the
//! agent having to remember. Runs every 6h (tmux `recall-auto`) and on demand.
//!
//! What it does, for each session in the recall index:
//!   - Session file GONE **and** the session is in the deletion marker
//!     (.deleted.json, written by the gateway when the user deletes a chat)
//!     -> delete its digest + index line. That is the only case a digest
//!     is deleted.
//!   - Session file GONE and NOT in the marker -> the file was LOST (the
//!     gateway store dropped old sessions before), not user-deleted.
//!     The digest is PRESERVED and the session is logged as lost. This is
//!     the guard: deleting a session chat deletes its memory ONLY when the
//!     user actually deleted it.
//!   - Session has NEW messages since the digest -> summarize ONLY the delta
//!     with a cheap model and merge into the digest.
//!   - New session file not in the index -> add it + create a digest.
//!   - Nothing changed -> skip (no model call, no write).
//!
//! Model failure is NOT fatal: the session is marked stale and retried next
//! run. Nothing is deleted or overwritten on a failed model call.
//! muse-main is NOT touched — Muse updates that digest itself.
//!
//! Provider keys: read from /root/pi-bridge/provider-keys.env (HMS_API_KEY)
//! and /root/.profile + /root/.bashrc (TOKENLAB_API_KEY, MOSS_API_KEY).
//! Keys are never printed. The model is called via the `curl` binary with
//! the Bearer token in a 0600 temp config file, never on a command line.
//! Order: 100milionseek, tokenlab, moss.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKIP_AUTO: &[&str] = &["muse-main"];

const DIGEST_FORMAT: &str = "# {name}\n- id: {sid}, model: {model}, msgs: {msgs}, updated: {today}\n- purpose: <one line>\n- decisions: <key decisions, comma-separated>\n- facts: <key facts, comma-separated>\n- outcomes: <what got done / verified>\n- open: <open items, or none>\n- full: {full_path}";

const UPDATE_PROMPT: &str = "You update a session digest for a recall system. The digest is the ONLY thing read to recall a session, so it must stay short (under ~150 words) and factual.\n\nOLD DIGEST:\n{old_digest}\n\nNEW MESSAGES since that digest (most recent last):\n{new_text}\n\nOutput ONLY the updated digest, in exactly this format (keep the same id, name, and full: path; update msgs: to {msgs} and updated: to {today}):\n{format}\n\nRules: keep it under ~150 words; only facts from the messages above; drop nothing important from the old digest; add the new decisions/facts/outcomes; if nothing important happened, return the old digest with only msgs:/updated: changed.";

const NEW_PROMPT: &str = "You write a session digest for a recall system. The digest is the ONLY thing read to recall a session, so it must stay short (under ~150 words) and factual.\n\nSESSION MESSAGES (first few):\n{new_text}\n\nOutput ONLY the digest, in exactly this format:\n{format}\n\nRules: keep it under ~150 words; only facts from the messages above; name the session from its first user message.";

struct Provider {
    name: &'static str,
    base: &'static str,
    model: &'static str,
    env_key: &'static str,
}

const PROVIDERS: &[Provider] = &[
    Provider { name: "100milionseek", base: "https://100milionseek.top/v1", model: "deepseek-v4.1-flash-free", env_key: "HMS_API_KEY" },
    Provider { name: "tokenlab", base: "https://api.tokenlab.amd.com/v1", model: "DeepSeek-V4.1-Flash", env_key: "TOKENLAB_API_KEY" },
    Provider { name: "moss", base: "https://api.mosi.cn/v1", model: "nex-n2.6", env_key: "MOSS_API_KEY" },
];

fn base_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("recall");
            if p.join("index.jsonl").exists() {
                return p;
            }
            if dir.join("index.jsonl").exists() {
                return dir.to_path_buf();
            }
        }
    }
    for cand in ["/root/work/recall", "/home/hatch/workspace/altare-pi-setup/recall"] {
        let p = PathBuf::from(cand);
        if p.join("index.jsonl").exists() {
            return p;
        }
    }
    PathBuf::from("/root/work/recall")
}

fn sess_dir() -> PathBuf {
    for cand in ["/root/pi-bridge/zs-sessions", "/home/hatch/workspace/altare-pi-setup/zs-sessions"] {
        let p = PathBuf::from(cand);
        if p.is_dir() {
            return p;
        }
    }
    PathBuf::from("/root/pi-bridge/zs-sessions")
}

fn today() -> String {
    // YYYY-MM-DD in local time, via `date` (no chrono dep).
    let out = Command::new("date").arg("+%Y-%m-%d").output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "unknown".to_string(),
    }
}

fn log(base: &Path, msg: &str) {
    let line = format!("{} {}", today(), msg);
    println!("{}", line);
    let log_path = base.join("auto_digest.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(f, "{}", line);
    }
}

fn load_env_files() -> HashMap<String, String> {
    let mut keys: HashMap<String, String> = HashMap::new();
    for path in ["/root/pi-bridge/provider-keys.env", "/root/.profile", "/root/.bashrc"] {
        if let Ok(txt) = fs::read_to_string(path) {
            for line in txt.lines() {
                let line = line.trim();
                let line = line.strip_prefix("export ").unwrap_or(line);
                if let Some((k, v)) = line.split_once('=') {
                    let k = k.trim();
                    let v = v.trim().trim_matches(|c| c == '"' || c == '\'');
                    if !k.is_empty() && !keys.contains_key(k) {
                        keys.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
    }
    for (k, v) in std::env::vars() {
        keys.entry(k).or_insert(v);
    }
    keys
}

/// Call a chat-completions endpoint via curl. The Bearer token goes in a
/// 0600 temp config file, never on the command line. Returns the model's
/// reply text, or None when every provider failed.
fn call_model(prompt: &str, keys: &HashMap<String, String>) -> Option<String> {
    let pid = std::process::id();
    let payload_path = format!("/tmp/auto-digest-payload-{}.json", pid);
    let conf_path = format!("/tmp/auto-digest-curl-{}.conf", pid);
    for prov in PROVIDERS {
        let key = match keys.get(prov.env_key) {
            Some(k) if !k.is_empty() => k,
            _ => continue,
        };
        let payload = serde_json::json!({
            "model": prov.model,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": 900,
            "temperature": 0.2,
        });
        if fs::write(&payload_path, payload.to_string()).is_err() {
            continue;
        }
        // Restrict the temp files so the key is not world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&payload_path, fs::Permissions::from_mode(0o600));
        }
        let conf = format!(
            "url = \"{}/chat/completions\"\nheader = \"Content-Type: application/json\"\nheader = \"Authorization: Bearer {}\"\ndata-binary = \"@{}\"\nmax-time = 90\nsilent = true\n",
            prov.base.trim_end_matches('/'),
            key,
            payload_path
        );
        if fs::write(&conf_path, conf).is_err() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&conf_path, fs::Permissions::from_mode(0o600));
        }
        let out = Command::new("curl").arg("-K").arg(&conf_path).output();
        let _ = fs::remove_file(&conf_path);
        let _ = fs::remove_file(&payload_path);
        let out = match out {
            Ok(o) if o.status.success() => o,
            _ => continue,
        };
        let data: serde_json::Value = match serde_json::from_slice(&out.stdout) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(text) = data
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
        {
            let text = text.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

fn msg_text(m: &serde_json::Value) -> String {
    let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("?");
    let c = m.get("content");
    let mut text = String::new();
    match c {
        Some(serde_json::Value::Array(parts)) => {
            let mut bits = Vec::new();
            for p in parts {
                if !p.is_object() {
                    continue;
                }
                let t = p.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if t == "text" {
                    if let Some(s) = p.get("text").and_then(|v| v.as_str()) {
                        bits.push(s.to_string());
                    }
                } else if t == "toolResult" {
                    let mut tt = String::new();
                    if let Some(serde_json::Value::Array(arr)) = p.get("text") {
                        for x in arr {
                            if let Some(s) = x.get("text").and_then(|v| v.as_str()) {
                                tt.push_str(s);
                                tt.push(' ');
                            }
                        }
                    } else if let Some(s) = p.get("text").and_then(|v| v.as_str()) {
                        tt = s.to_string();
                    }
                    let mut t2: String = tt.chars().take(300).collect();
                    t2 = format!("[tool] {}", t2);
                    bits.push(t2);
                }
            }
            text = bits.join(" ");
        }
        Some(serde_json::Value::String(s)) => text = s.clone(),
        _ => {}
    }
    let short: String = text.chars().take(600).collect();
    format!("[{}] {}", role, short)
}

/// Map short id (first 8 chars) -> (full id, path) for every session json.
fn session_files(sess: &Path) -> HashMap<String, (String, PathBuf)> {
    let mut out = HashMap::new();
    if let Ok(rd) = fs::read_dir(sess) {
        let mut paths: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let full = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if full.len() < 8 {
                continue;
            }
            out.insert(full[..8].to_string(), (full, p));
        }
    }
    out
}

fn read_index(base: &Path) -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    if let Ok(txt) = fs::read_to_string(base.join("index.jsonl")) {
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                rows.push(v);
            }
        }
    }
    rows
}

fn write_index(base: &Path, rows: &[serde_json::Value]) {
    let mut out = String::new();
    for r in rows {
        out.push_str(&serde_json::to_string(r).unwrap_or_default());
        out.push('\n');
    }
    let _ = fs::write(base.join("index.jsonl"), out);
}

fn load_json_map(path: &Path) -> HashMap<String, serde_json::Value> {
    match fs::read_to_string(path) {
        Ok(txt) => serde_json::from_str::<HashMap<String, serde_json::Value>>(&txt)
            .unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

fn save_json_map(path: &Path, m: &HashMap<String, serde_json::Value>) {
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    let mut out = String::from("{\n");
    for (i, k) in keys.iter().enumerate() {
        let v = serde_json::to_string(&m[*k]).unwrap_or_default();
        out.push_str(&format!(" \"{}\": {}", k, v));
        if i + 1 < keys.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("}\n");
    let _ = fs::write(path, out);
}

fn fmt_digest(name: &str, sid: &str, model: &str, msgs: usize, today: &str, full_path: &str) -> String {
    DIGEST_FORMAT
        .replace("{name}", name)
        .replace("{sid}", sid)
        .replace("{model}", model)
        .replace("{msgs}", &msgs.to_string())
        .replace("{today}", today)
        .replace("{full_path}", full_path)
}

fn main() {
    let base = base_dir();
    let sess = sess_dir();
    let digests = base.join("digests");
    let _ = fs::create_dir_all(&digests);
    let cursor_path = digests.join(".cursor.json");
    let deleted_path = digests.join(".deleted.json");
    let log_path = base.join("auto_digest.log");
    let _ = log_path;
    let rows = read_index(&base);
    let files = session_files(&sess);
    let mut cursor = load_json_map(&cursor_path);
    let mut deleted = load_json_map(&deleted_path);
    let keys = load_env_files();
    let today_s = today();
    let mut changed = false;

    // --- 1. sessions in the index: deleted? updated? ---
    let mut kept: Vec<serde_json::Value> = Vec::new();
    for mut row in rows {
        let sid = row.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if sid.is_empty() {
            continue;
        }
        if SKIP_AUTO.contains(&sid.as_str()) {
            kept.push(row);
            continue;
        }
        if !files.contains_key(&sid) {
            // Session file gone. Delete its memory ONLY when the gateway
            // recorded an explicit user deletion in .deleted.json. A missing
            // file with no marker means the file was LOST, not deleted —
            // the digest is the only memory left, so it is preserved.
            if deleted.contains_key(&sid) {
                let dp = digests.join(format!("{}.md", sid));
                let _ = fs::remove_file(&dp);
                cursor.remove(&sid);
                deleted.remove(&sid);
                log(&base, &format!(
                    "deleted {} ({}): user deleted the chat, digest + index entry removed",
                    sid,
                    row.get("name").and_then(|v| v.as_str()).unwrap_or("?")
                ));
                changed = true;
                continue;
            }
            log(&base, &format!(
                "LOST {} ({}): session file gone but no deletion marker — digest PRESERVED (not user-deleted)",
                sid,
                row.get("name").and_then(|v| v.as_str()).unwrap_or("?")
            ));
            kept.push(row);
            continue;
        }
        let (full, path) = files.get(&sid).unwrap().clone();
        let data: serde_json::Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            Some(v) => v,
            None => {
                kept.push(row);
                continue;
            }
        };
        let msgs = data
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        let covered = cursor
            .get(&sid)
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or_else(|| row.get("msgs").and_then(|v| v.as_u64()).map(|n| n as usize).unwrap_or(0));
        let dp = digests.join(format!("{}.md", sid));
        if !dp.exists() {
            // No digest yet — step 2 handles it as a new session.
            kept.push(row);
            continue;
        }
        if msgs <= covered {
            kept.push(row);
            continue;
        }
        // New messages: summarize the delta (last 30 new ones, like Python).
        let arr = data.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
        let delta = &arr[covered.min(arr.len())..];
        let start = delta.len().saturating_sub(30);
        let new_msgs = &delta[start..];
        let new_text: Vec<String> = new_msgs.iter().map(msg_text).collect();
        let new_text = new_text.join("\n");
        let old = fs::read_to_string(&dp).unwrap_or_default();
        let fmt = fmt_digest(
            row.get("name").and_then(|v| v.as_str()).unwrap_or(&sid),
            &sid,
            row.get("model").and_then(|v| v.as_str()).unwrap_or(""),
            msgs,
            &today_s,
            &format!("{}/{}.json", sess.display(), full),
        );
        let prompt = UPDATE_PROMPT
            .replace("{old_digest}", &old)
            .replace("{new_text}", &new_text)
            .replace("{msgs}", &msgs.to_string())
            .replace("{today}", &today_s)
            .replace("{format}", &fmt);
        match call_model(&prompt, &keys) {
            Some(out) => {
                let _ = fs::write(&dp, out.trim().to_string() + "\n");
                if let Some(o) = row.as_object_mut() {
                    o.insert("msgs".to_string(), serde_json::json!(msgs));
                    o.insert("updated".to_string(), serde_json::json!(today_s));
                }
                cursor.insert(sid.clone(), serde_json::json!(msgs));
                changed = true;
                log(&base, &format!(
                    "updated {} ({}): {} -> {} msgs",
                    sid,
                    row.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                    covered,
                    msgs
                ));
            }
            None => {
                log(&base, &format!("stale {}: model call failed, will retry next run", sid));
            }
        }
        kept.push(row);
    }

    // --- 2. new session files not in the index ---
    let known: std::collections::HashSet<String> = kept
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    let mut file_ids: Vec<String> = files.keys().cloned().collect();
    file_ids.sort();
    for sid in file_ids {
        if known.contains(&sid) || SKIP_AUTO.contains(&sid.as_str()) {
            continue;
        }
        let (full, path) = files.get(&sid).unwrap().clone();
        let data: serde_json::Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
        {
            Some(v) => v,
            None => continue,
        };
        let arr = data
            .get("messages")
            .and_then(|m| m.as_array())
            .cloned()
            .unwrap_or_default();
        let n = arr.len();
        let mut name = sid.clone();
        for m in &arr {
            if m.get("role").and_then(|v| v.as_str()) == Some("user") {
                let t = msg_text(m);
                let t = t.strip_prefix("[user]").unwrap_or(&t).trim();
                let t: String = t.chars().take(48).collect();
                if !t.is_empty() {
                    name = t;
                }
                break;
            }
        }
        let model = data.get("model").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let first: Vec<String> = arr.iter().take(8).map(msg_text).collect();
        let new_text = first.join("\n");
        let fmt = fmt_digest(&name, &sid, &model, n, &today_s, &format!("{}/{}.json", sess.display(), full));
        let prompt = NEW_PROMPT
            .replace("{new_text}", &new_text)
            .replace("{format}", &fmt);
        let dp = digests.join(format!("{}.md", sid));
        match call_model(&prompt, &keys) {
            Some(out) => {
                let _ = fs::write(&dp, out.trim().to_string() + "\n");
            }
            None => {
                // Model down: write a minimal digest so the session is findable.
                let minimal = fmt
                    .replace("<one line>", "new session; digest pending (model call failed)")
                    .replace("<key decisions, comma-separated>", "none yet")
                    .replace("<key facts, comma-separated>", "see transcript")
                    .replace("<what got done / verified>", "see transcript")
                    .replace("<open items, or none>", "digest pending");
                let _ = fs::write(&dp, minimal + "\n");
            }
        }
        kept.push(serde_json::json!({
            "id": sid,
            "name": name,
            "model": model,
            "msgs": n,
            "updated": today_s,
            "topics": [],
            "sum": format!("{} — digest auto-created.", &name.chars().take(80).collect::<String>()),
        }));
        cursor.insert(sid.clone(), serde_json::json!(n));
        changed = true;
        log(&base, &format!("added {} ({}): new session, digest created", sid, name));
    }

    if changed {
        write_index(&base, &kept);
        save_json_map(&cursor_path, &cursor);
        save_json_map(&deleted_path, &deleted);
        log(&base, "index + cursor saved");
    } else {
        log(&base, "no changes");
    }
}
