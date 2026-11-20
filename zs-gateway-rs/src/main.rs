//! zs-gateway — Rust HTTP gateway in front of Zerostack's ACP server.
//!
//! Same HTTP surface, JSON shapes and auth as the Python gateway it replaces
//! (zs-gateway.py, kept on disk as a backup), plus the model system:
//! providers/models/keys, round-robin key rotation, fallback order + toggle,
//! and provider /models loading.
//!
//! Std-only HTTP + serde_json + toml. Single small binary, flat RAM, no
//! background processes beyond one thread per session and the idle reaper.
mod acp;
mod http;
mod models;
mod sha256;
mod stats;
mod util;

use serde_json::{json, Value};
use std::io::Write;
use std::sync::{Arc, Mutex};

pub const BASE: &str = "/root/pi-bridge";
pub const WORK: &str = "/root/work";
pub const SESS_DIR: &str = "/root/pi-bridge/zs-sessions";
pub const MODELS_FILE: &str = "/root/pi-bridge/models.json";
pub const ROTATION_FILE: &str = "/root/pi-bridge/key-rotation.json";
pub const ZS_CONFIG: &str = "/root/.config/zerostack/config.toml";
pub const ADMIN_HTML: &str = include_str!("admin.html");

pub fn log(msg: &str) {
    util::log(msg);
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

pub struct App {
    token: String,
    admin_pass: Mutex<Option<String>>,
    fails: Mutex<(u32, f64)>,
}

/// config.toml, cached until its mtime changes (the Admin page edits it).
fn cfg_cache() -> &'static Mutex<(Option<std::time::SystemTime>, Value)> {
    static C: std::sync::OnceLock<Mutex<(Option<std::time::SystemTime>, Value)>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new((None, Value::Object(Default::default()))))
}

pub const ZS_BIN: &str = "/root/.local/bin/zerostack";

pub fn zs_config() -> Value {
    let mtime = std::fs::metadata(ZS_CONFIG).and_then(|m| m.modified()).ok();
    let mut c = cfg_cache().lock().unwrap();
    if c.0 == mtime && c.1.is_object() {
        return c.1.clone();
    }
    let v = std::fs::read_to_string(ZS_CONFIG)
        .ok()
        .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
        .and_then(|t| serde_json::to_value(t).ok())
        .unwrap_or_else(|| json!({}));
    *c = (mtime, v.clone());
    v
}

fn gw_env_read(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("{}/gateway.env", BASE)).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix(&format!("{}=", key)) {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn gw_env_write(key: &str, val: &str) -> Result<(), String> {
    let path = format!("{}/gateway.env", BASE);
    let mut lines: Vec<(String, String)> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(&path) {
        for line in text.lines() {
            if let Some((k, v)) = line.trim().split_once('=') {
                lines.push((k.to_string(), v.to_string()));
            }
        }
    }
    let mut found = false;
    for (k, v) in lines.iter_mut() {
        if k == key {
            *v = val.to_string();
            found = true;
        }
    }
    if !found {
        lines.push((key.to_string(), val.to_string()));
    }
    let body: String = lines.iter().map(|(k, v)| format!("{}={}\n", k, v)).collect();
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(())
}

/// Atomic-ish rewrite of provider-keys.env keeping unknown lines intact.
fn keys_file_lines() -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(models::KEYS_FILE) {
        for line in text.lines() {
            if let Some((k, v)) = line.trim().split_once('=') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}
fn keys_file_write(lines: &[(String, String)]) -> bool {
    let body: String = lines.iter().map(|(k, v)| format!("{}={}\n", k, v)).collect();
    if std::fs::write(models::KEYS_FILE, body).is_err() {
        return false;
    }
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(models::KEYS_FILE, std::fs::Permissions::from_mode(0o600));
    true
}

// ---- endpoint helpers -----------------------------------------------------
fn monid_status() -> Value {
    let path = "/root/.config/monid/credentials.yaml";
    let set = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| {
            t.split("key:")
                .nth(1)
                .and_then(|r| r.split('"').nth(1).map(|s| s.to_string()))
        })
        .map(|k| !k.trim().is_empty() && k != "PASTE_MONID_KEY_HERE")
        .unwrap_or(false);
    json!({"set": set})
}

fn save_monid(key: &str) -> bool {
    if key.len() < 8 {
        return false;
    }
    let dir = "/root/.config/monid";
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let ok = std::fs::write(
        format!("{}/credentials.yaml", dir),
        format!("key: \"{}\"\nlabel: main\n", key),
    )
    .is_ok();
    if ok {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            format!("{}/credentials.yaml", dir),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    ok
}

fn provider_key_status() -> Value {
    let cfg = zs_config();
    let mut names: Vec<String> = Vec::new();
    if let Some(cps) = cfg.get("custom_providers").and_then(|v| v.as_object()) {
        for (_n, cp) in cps {
            if let Some(env) = cp.get("api_key_env").and_then(|v| v.as_str()) {
                if !names.iter().any(|x| x == env) {
                    names.push(env.to_string());
                }
            }
        }
    }
    let out: Vec<Value> = names
        .iter()
        .map(|n| json!({"env_name": n, "set": models::key_value(n).is_some()}))
        .collect();
    json!({"keys": out})
}

fn public_url(env_key: &str, default: &str) -> String {
    env_or(env_key, default)
}

// ---- model system routes --------------------------------------------------
fn ms_payload() -> Value {
    models::model_system_payload()
}

fn registry_edit<F: FnOnce(&mut Value) -> Result<(), String>>(f: F) -> Result<Value, String> {
    let mut reg = models::registry().0;
    f(&mut reg)?;
    models::normalize(&mut reg);
    models::save_registry(&reg)?;
    Ok(models::model_system_payload())
}

fn provider_add(b: &Value) -> Result<Value, String> {
    let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let display = b.get("display").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let base_url = b.get("base_url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if name.is_empty() || base_url.is_empty() {
        return Err("provider name and base URL are required".into());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
        return Err("provider name: letters, digits, _ . - only".into());
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err("base URL must start with http:// or https://".into());
    }
    let api_key = b.get("api_key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut stored_env = String::new();
    if !api_key.is_empty() {
        let envn = models::env_for_provider(&name);
        if !models::save_secret(&envn, &api_key) {
            return Err("could not save the API key".into());
        }
        stored_env = envn;
    }
    registry_edit(|reg| {
        let provs = reg["providers"].as_object_mut().ok_or("registry corrupt")?;
        if provs.contains_key(&name) {
            return Err(format!("provider '{}' already exists", name));
        }
        let envn = if stored_env.is_empty() { models::env_for_provider(&name) } else { stored_env.clone() };
        provs.insert(name.clone(), json!({
            "display": if display.is_empty() { name } else { display },
            "base_url": base_url,
            "api_key_env": envn,
            "api_style": b.get("api_style").and_then(|v| v.as_str()).unwrap_or(""),
            "models": [],
        }));
        Ok(())
    })
}

fn provider_edit(b: &Value) -> Result<Value, String> {
    let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let old = b.get("old_name").and_then(|v| v.as_str()).unwrap_or(&name).to_string();
    registry_edit(|reg| {
        let provs = reg["providers"].as_object_mut().ok_or("registry corrupt")?;
        let mut p = provs.get(&old).cloned().ok_or(format!("no provider '{}'", old))?;
        if let Some(d) = b.get("display").and_then(|v| v.as_str()) {
            if !d.trim().is_empty() {
                p["display"] = json!(d.trim());
            }
        }
        if let Some(u) = b.get("base_url").and_then(|v| v.as_str()) {
            if !u.trim().is_empty() {
                p["base_url"] = json!(u.trim());
            }
        }
        if let Some(s) = b.get("api_style").and_then(|v| v.as_str()) {
            p["api_style"] = json!(s);
        }
        if old != name {
            if provs.contains_key(&name) {
                return Err(format!("provider '{}' already exists", name));
            }
            // keep keys: the env var name travels with the provider
            provs.remove(&old);
            provs.insert(name.clone(), p.clone());
            // and rename the order keys
            if let Some(order) = reg["order"].as_array_mut() {
                for k in order.iter_mut() {
                    if let Some(key) = k.as_str() {
                        if let Some(rest) = key.strip_prefix(&format!("{}/", old)) {
                            *k = json!(format!("{}/{}", name, rest));
                        }
                    }
                }
            }
        } else {
            provs.insert(old, p);
        }
        Ok(())
    })
}

fn provider_remove(b: &Value) -> Result<Value, String> {
    let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    registry_edit(|reg| {
        let provs = reg["providers"].as_object_mut().ok_or("registry corrupt")?;
        if provs.remove(&name).is_none() {
            return Err(format!("no provider '{}'", name));
        }
        if let Some(order) = reg["order"].as_array_mut() {
            order.retain(|k| {
                k.as_str().map(|s| !s.starts_with(&format!("{}/", name))).unwrap_or(true)
            });
        }
        Ok(())
    })
}

fn models_add(b: &Value) -> Result<Value, String> {
    let provider = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let list = b.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let single_id = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut items: Vec<Value> = list;
    if !single_id.is_empty() {
        items.push(json!({
            "id": single_id,
            "context": b.get("context").cloned().unwrap_or(json!(0)),
            "output": b.get("output").cloned().unwrap_or(json!(0)),
        }));
    }
    if provider.is_empty() || items.is_empty() {
        return Err("provider and at least one model are required".into());
    }
    registry_edit(|reg| {
        let p = reg["providers"]
            .as_object_mut()
            .and_then(|o| o.get_mut(&provider))
            .ok_or(format!("no provider '{}'", provider))?;
        if p.get("models").and_then(|v| v.as_array()).is_none() {
            p["models"] = json!([]);
        }
        let arr = p["models"].as_array_mut().unwrap();
        for it in &items {
            let id = it.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if id.is_empty() {
                continue;
            }
            let ctx = it.get("context").and_then(|v| v.as_i64()).unwrap_or(0);
            let out = it.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
            let pos = arr.iter().position(|m| m.get("id").and_then(|v| v.as_str()) == Some(id.as_str()));
            match pos {
                Some(i) => {
                    arr[i]["context"] = json!(ctx);
                    arr[i]["output"] = json!(out);
                }
                None => arr.push(json!({"id": id, "context": ctx, "output": out})),
            }
        }
        Ok(())
    })
}

fn model_edit(b: &Value) -> Result<Value, String> {
    let provider = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let old_id = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let new_id = b.get("new_id").and_then(|v| v.as_str()).unwrap_or(&old_id).trim().to_string();
    if new_id.is_empty() {
        return Err("model id is required".into());
    }
    registry_edit(|reg| {
        let p = reg["providers"]
            .as_object_mut()
            .and_then(|o| o.get_mut(&provider))
            .ok_or(format!("no provider '{}'", provider))?;
        let arr = p["models"].as_array_mut().ok_or("no models")?;
        let i = arr
            .iter()
            .position(|m| m.get("id").and_then(|v| v.as_str()) == Some(old_id.as_str()))
            .ok_or(format!("no model '{}'", old_id))?;
        let ctx = b.get("context").and_then(|v| v.as_i64()).unwrap_or(0);
        let out = b.get("output").and_then(|v| v.as_i64()).unwrap_or(0);
        arr[i] = json!({"id": new_id, "context": ctx, "output": out});
        if old_id != new_id {
            if let Some(order) = reg["order"].as_array_mut() {
                let from = format!("{}/{}", provider, old_id);
                let to = format!("{}/{}", provider, new_id);
                for k in order.iter_mut() {
                    if k.as_str() == Some(from.as_str()) {
                        *k = json!(to);
                    }
                }
            }
        }
        Ok(())
    })
}

fn model_remove(b: &Value) -> Result<Value, String> {
    let provider = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id = b.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    registry_edit(|reg| {
        if let Some(p) = reg["providers"].as_object_mut().and_then(|o| o.get_mut(&provider)) {
            if let Some(arr) = p["models"].as_array_mut() {
                arr.retain(|m| m.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
            }
        }
        let key = format!("{}/{}", provider, id);
        if let Some(order) = reg["order"].as_array_mut() {
            order.retain(|k| k.as_str() != Some(key.as_str()));
        }
        Ok(())
    })
}

fn model_order(b: &Value) -> Result<Value, String> {
    let order: Vec<String> = b
        .get("order")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    registry_edit(|reg| {
        if !order.is_empty() {
            reg["order"] = json!(order);
        }
        if let Some(fb) = b.get("fallback_enabled").and_then(|v| v.as_bool()) {
            reg["fallback_enabled"] = json!(fb);
        }
        if let Some(d) = b.get("default").and_then(|v| v.as_str()) {
            if !d.is_empty() {
                let mut o: Vec<String> = reg["order"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                o.retain(|k| k != d);
                o.insert(0, d.to_string());
                reg["order"] = json!(o);
            }
        }
        Ok(())
    })
}

fn provider_models(b: &Value) -> Result<Value, String> {
    let base_url = b.get("base_url").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut api_key = b.get("api_key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if api_key.is_empty() {
        if let Some(pn) = b.get("provider").and_then(|v| v.as_str()) {
            let reg = models::registry();
            if let Some(p) = reg.provider(pn) {
                let envn = p.get("api_key_env").and_then(|v| v.as_str()).unwrap_or("");
                if let Some(k) = models::key_value(envn) {
                    api_key = k;
                }
            }
            if base_url.is_empty() {
                if let Some(u) = reg.provider(pn).and_then(|p| p.get("base_url").cloned()) {
                    if let Some(u) = u.as_str() {
                        return finish_provider_models(u, &api_key);
                    }
                }
            }
        }
    }
    finish_provider_models(&base_url, &api_key)
}

fn finish_provider_models(base_url: &str, api_key: &str) -> Result<Value, String> {
    if api_key.is_empty() {
        return Err("an API key is required to load models".into());
    }
    let list = models::fetch_provider_models(base_url, api_key)?;
    Ok(json!({"models": list, "count": list.len()}))
}

fn key_add(b: &Value) -> Result<Value, String> {
    let provider = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let value = b.get("api_key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if value.is_empty() {
        return Err("the key value is required".into());
    }
    let reg = models::registry();
    let base = reg
        .provider(&provider)
        .and_then(|p| p.get("api_key_env").and_then(|v| v.as_str()).map(String::from))
        .ok_or(format!("no provider '{}'", provider))?;
    let slot = models::next_key_slot(&base).ok_or("this provider has no free key slot left")?;
    if !models::save_secret(&slot, &value) {
        return Err("could not save the key".into());
    }
    Ok(json!({"ok": true, "env_name": slot}))
}

fn key_remove(b: &Value) -> Result<Value, String> {
    let provider = b.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let env_name = b.get("env_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let reg = models::registry();
    let base = reg
        .provider(&provider)
        .and_then(|p| p.get("api_key_env").and_then(|v| v.as_str()).map(String::from))
        .ok_or(format!("no provider '{}'", provider))?;
    if env_name != base && !env_name.starts_with(&format!("{}_", base)) {
        return Err("that key does not belong to this provider".into());
    }
    if !models::remove_secret(&env_name) {
        return Err("could not remove the key".into());
    }
    Ok(json!({"ok": true}))
}

// ---- request routing ------------------------------------------------------
fn json_ok(v: Value) -> http::Response {
    http::Response::json(200, &v)
}
fn json_err(status: u16, msg: &str) -> http::Response {
    http::Response::json(status, &json!({"error": msg}))
}
fn result_resp(r: Result<Value, String>) -> http::Response {
    match r {
        Ok(v) => json_ok(v),
        Err(e) => json_err(400, &e),
    }
}

fn read_static(path: &str) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

fn handle(app: &Arc<App>, req: &http::Request) -> http::Response {
    let path = req.path.as_str();
    let method = req.method.as_str();

    // ---- unauthenticated page routes ----
    if method == "GET" && (path == "/pi" || path == "/pi.html") {
        return match read_static(&format!("{}/zs.html", BASE)) {
            Some(b) => http::Response::html(b),
            None => http::Response::html(
                b"<!doctype html><meta charset=utf-8><title>Zerostack Chat</title><p style='font:15px system-ui;padding:24px'>zs.html is missing on the box. Copy it to /root/pi-bridge/zs.html and reload.</p>".to_vec(),
            ),
        };
    }
    if method == "GET" && path == "/admin" {
        return http::Response::html(ADMIN_HTML.as_bytes().to_vec());
    }

    // ---- opening password / token ----
    if method == "POST" && path == "/admin-login" {
        let now = util::now();
        let (count, until) = *app.fails.lock().unwrap();
        if until > now {
            return json_err(429, "too many attempts — try again in a few minutes");
        }
        let pass = app.admin_pass.lock().unwrap().clone();
        let pw = req.json();
        let pw = pw.get("password").and_then(|v| v.as_str()).unwrap_or("").to_string();
        match pass {
            None => json_err(400, "no opening password set — use the gateway token"),
            Some(hash) => {
                let digest = sha256::digest_hex(pw.as_bytes());
                if digest.eq_ignore_ascii_case(hash.trim()) {
                    *app.fails.lock().unwrap() = (0, 0.0);
                    json_ok(json!({"token": app.token}))
                } else {
                    let mut f = app.fails.lock().unwrap();
                    let c = count + 1;
                    if c >= 5 {
                        *f = (0, now + 600.0);
                    } else {
                        *f = (c, 0.0);
                    }
                    json_err(401, "wrong password")
                }
            }
        }
    } else {
        // ---- everything else needs the bearer token (except /health) ----
        let authed = path == "/health"
            || req.header("authorization") == Some(format!("Bearer {}", app.token).as_str());
        if !authed {
            return json_err(401, "unauthorized");
        }

        if method == "GET" {
            match path {
                "/health" => {
                    return json_ok(json!({
                        "ok": true,
                        "sessions_running": acp::any_running(),
                        "backend": "zerostack",
                        "impl": "rust",
                        "pid": std::process::id(),
                    }))
                }
                "/stats" => return json_ok(stats::snapshot()),
                "/sessions" => return json_ok(acp::sessions_list()),
                "/models" => return json_ok(models::models_payload()),
                "/model-system" => return json_ok(ms_payload()),
                "/keys" => return json_ok(provider_key_status()),
                "/instabox-key" => {
                    return json_ok(json!({"set": models::key_value("INSTABOX_API_KEY").is_some()}))
                }
                "/monid" => return json_ok(monid_status()),
                "/settings" => {
                    let reg = models::registry();
                    let order = reg.order();
                    let (p, m) = order
                        .first()
                        .and_then(|k| k.split_once('/').map(|(a, b)| (a.to_string(), b.to_string())))
                        .unwrap_or_default();
                    return json_ok(json!({"defaultProvider": p, "defaultModel": m}));
                }
                "/settings-info" => return json_ok(settings_info()),
                "/switch" => {
                    return json_ok(json!({
                        "backend": "zerostack",
                        "url": public_url("ZS_ZS_URL", "/pi"),
                    }))
                }
                _ => {}
            }

            // /sessions/{id}[/status|/events]
            if let Some(rest) = path.strip_prefix("/sessions/") {
                let (sid, sub) = match rest.split_once('/') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (rest.to_string(), String::new()),
                };
                let sess = acp::sessions().lock().unwrap().get(&sid).cloned();
                if sub == "status" {
                    return json_ok(json!({
                        "id": sid,
                        "running": sess.as_ref().map(|s| s.running()).unwrap_or(false),
                        "last_assistant_text": sess.as_ref().map(|s| s.lock().last_assistant.clone()).unwrap_or_default(),
                    }));
                }
                if sub == "events" {
                    let since: usize = req.q("since").and_then(|v| v.parse().ok()).unwrap_or(0);
                    let evs: Vec<Value> = match &sess {
                        Some(s) => {
                            let st = s.lock();
                            if since < st.events.len() {
                                st.events[since..].to_vec()
                            } else {
                                Vec::new()
                            }
                        }
                        None => {
                            let mut v = Vec::new();
                            if let Ok(t) = std::fs::read_to_string(format!(
                                "{}/{}.events.jsonl",
                                SESS_DIR, sid
                            )) {
                                for l in t.lines().skip(since) {
                                    if let Ok(x) = serde_json::from_str::<Value>(l) {
                                        v.push(x);
                                    }
                                }
                            }
                            v
                        }
                    };
                    return json_ok(json!({"events": evs, "next": since + evs.len()}));
                }
                if !sub.is_empty() {
                    return json_err(404, "unknown route");
                }
                return match sess {
                    Some(s) => {
                        let st = s.lock();
                        let user = st.messages.iter().filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("user")).count();
                        let asst = st.messages.iter().filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("assistant")).count();
                        json_ok(json!({
                            "id": sid,
                            "messages": {"data": {"messages": st.messages}},
                            "stats": {"data": {"userMessages": user, "assistantMessages": asst}},
                        }))
                    }
                    None => json_err(404, "not found"),
                };
            }
            return json_err(404, "unknown route");
        }

        if method == "DELETE" {
            if let Some(rest) = path.strip_prefix("/sessions/") {
                if !rest.contains('/') {
                    acp::delete_session(rest);
                    return json_ok(json!({"ok": true}));
                }
            }
            return json_err(404, "unknown route");
        }

        if method != "POST" {
            return json_err(405, "method not allowed");
        }

        let body = req.json();

        match path {
            "/instabox-key" => {
                let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if !key.starts_with("ibk_") {
                    return json_err(400, "bad key — an Instabox API key starts with ibk_");
                }
                if !models::save_secret("INSTABOX_API_KEY", &key) {
                    return json_err(400, "could not save key");
                }
                return json_ok(json!({"ok": true}));
            }
            "/monid" => {
                let key = body.get("key").and_then(|v| v.as_str()).unwrap_or("");
                if !save_monid(key) {
                    return json_err(400, "bad key");
                }
                return json_ok(json!({"ok": true}));
            }
            "/secret" => {
                let env_name = body.get("env_name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let value = body.get("value").and_then(|v| v.as_str()).unwrap_or("").trim();
                if value.is_empty() || !models::save_secret(env_name, value) {
                    return json_err(400, "bad env_name or empty value");
                }
                return json_ok(json!({"ok": true, "env_name": env_name}));
            }
            "/admin-password" => {
                let pw = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
                if pw.len() < 6 {
                    return json_err(400, "password must be at least 6 characters");
                }
                let digest = sha256::digest_hex(pw.as_bytes());
                if let Err(e) = gw_env_write("PI_ADMIN_PASS_SHA256", &digest) {
                    return json_err(500, &format!("could not store the password: {}", e));
                }
                *app.admin_pass.lock().unwrap() = Some(digest);
                return json_ok(json!({"ok": true, "restart_required": false}));
            }
            "/settings" => {
                let p = body.get("defaultProvider").and_then(|v| v.as_str()).unwrap_or("");
                let m = body.get("defaultModel").and_then(|v| v.as_str()).unwrap_or("");
                if p.is_empty() || m.is_empty() {
                    return json_err(400, "defaultProvider/defaultModel required");
                }
                let key = format!("{}/{}", p, m);
                return result_resp(model_order(&json!({"default": key})));
            }
            "/fallback" => {
                // legacy: {order:[quick-model names]} — accept model keys too
                let mut order: Vec<String> = body
                    .get("order")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                if order.is_empty() {
                    return json_err(400, "order must be a non-empty list");
                }
                let reg = models::registry();
                let known: Vec<String> = {
                    let mut v = Vec::new();
                    for (name, p) in reg.providers() {
                        for m in p.get("models").and_then(|x| x.as_array()).cloned().unwrap_or_default() {
                            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                                v.push(format!("{}/{}", name, id));
                            }
                        }
                    }
                    v
                };
                order.retain(|k| known.contains(k));
                if order.is_empty() {
                    return json_err(400, "order must name at least one known model");
                }
                return result_resp(model_order(&json!({"order": order})));
            }
            "/provider-models" => return result_resp(provider_models(&body)),
            "/providers" => {
                let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("add");
                return result_resp(match action {
                    "add" => provider_add(&body),
                    "edit" => provider_edit(&body),
                    "remove" => provider_remove(&body),
                    _ => Err("action must be add, edit or remove".into()),
                });
            }
            "/provider-keys" => {
                let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("add");
                return result_resp(match action {
                    "add" => key_add(&body),
                    "remove" => key_remove(&body),
                    _ => Err("action must be add or remove".into()),
                });
            }
            "/models" => {
                let action = body.get("action").and_then(|v| v.as_str()).unwrap_or("add");
                return result_resp(match action {
                    "add" => models_add(&body),
                    "edit" => model_edit(&body),
                    "remove" => model_remove(&body),
                    "order" => model_order(&body),
                    _ => Err("action must be add, edit, remove or order".into()),
                });
            }
            "/model-order" => return result_resp(model_order(&body)),
            "/prompt" => {
                let text = body.get("message").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if text.is_empty() {
                    return json_err(400, "message required");
                }
                let sid = body.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let sess = if sid.is_empty() {
                    None
                } else {
                    match acp::sessions().lock().unwrap().get(&sid).cloned() {
                        Some(s) => Some(s),
                        None => return json_err(404, "unknown session"),
                    }
                };
                let s = match sess {
                    Some(s) => s,
                    None => {
                        // Auto-name a new session from the first message when the
                        // caller did not supply a name — so the sidebar never shows
                        // a bare session id.
                        let nm = body
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let nm = if nm.is_empty() {
                            let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                            let mut n: String = t.chars().take(48).collect();
                            if t.chars().count() > 48 {
                                n.push('…');
                            }
                            n
                        } else {
                            nm
                        };
                        acp::new_session(
                            body.get("cwd").and_then(|v| v.as_str()).unwrap_or(WORK),
                            body.get("model").and_then(|v| v.as_str()).unwrap_or(""),
                            &nm,
                        )
                    }
                };
                if let Some(m) = body.get("model").and_then(|v| v.as_str()) {
                    if !m.is_empty() {
                        s.lock().model = m.to_string();
                    }
                }
                // If an existing session is still unnamed (name == id), name it
                // from this message too.
                let renamed = {
                    let mut st = s.lock();
                    if st.name == s.id {
                        let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        let mut n: String = t.chars().take(48).collect();
                        if t.chars().count() > 48 {
                            n.push('…');
                        }
                        if !n.is_empty() {
                            st.name = n;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if renamed {
                    acp::save_session(&s);
                }
                let was = acp::enqueue(&s, &text);
                if !was {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                acp::save_session(&s);
                return json_ok(json!({
                    "session_id": s.id, "accepted": true,
                    "disposition": if was { "queued" } else { "sent" },
                    "response": {"success": true},
                }));
            }
            "/switch" => {
                let want = body.get("backend").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
                return match want.as_str() {
                    // Pi is fully removed from this server (2026-11-21). Only
                    // Zerostack remains; switching to Pi is gone.
                    "zerostack" => json_ok(json!({"backend": "zerostack", "url": public_url("ZS_ZS_URL", "/pi")})),
                    _ => json_err(400, "only 'zerostack' is available — Pi was removed"),
                };
            }
            "/restart" => {
                std::thread::spawn(|| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    acp::shutdown_all();
                    log("restart requested — exiting for the supervisor loop");
                    std::process::exit(0);
                });
                return json_ok(json!({"ok": true, "restarting": true}));
            }
            _ => {}
        }

        if let Some(rest) = path.strip_prefix("/sessions/") {
            let (sid, sub) = match rest.split_once('/') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (rest.to_string(), String::new()),
            };
            let sess = acp::sessions().lock().unwrap().get(&sid).cloned();
            let s = match sess {
                Some(s) => s,
                None => return json_err(404, "unknown session"),
            };
            match sub.as_str() {
                "prompt" => {
                    let text = body.get("message").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if text.is_empty() {
                        return json_err(400, "message required");
                    }
                    let was = acp::enqueue(&s, &text);
                    acp::save_session(&s);
                    return json_ok(json!({"success": true,
                        "disposition": if was { "queued" } else { "sent" }}));
                }
                "stop" => {
                    let was = acp::stop_session(&s);
                    return json_ok(json!({"success": true, "stopped": was,
                        "note": "ACP has no cancel; the session process was ended"}));
                }
                "rename" => {
                    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if name.is_empty() {
                        return json_err(400, "name required");
                    }
                    {
                        let mut st = s.lock();
                        st.name = name.clone();
                        st.mtime = util::now();
                    }
                    acp::save_session(&s);
                    acp::log_event(&s, json!({"type": "session_info_changed"}));
                    return json_ok(json!({"success": true, "name": name}));
                }
                "model" => {
                    let model = body.get("model").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                    if model.is_empty() {
                        return json_err(400, "model required");
                    }
                    {
                        let mut st = s.lock();
                        st.model = model.clone();
                        st.mtime = util::now();
                        st.needs_preamble = true;
                    }
                    acp::kill_proc(&s);
                    acp::save_session(&s);
                    acp::log_event(&s, json!({"type": "session_info_changed"}));
                    return json_ok(json!({"success": true, "model": model,
                        "note": "process restarted on next prompt"}));
                }
                "compact" => {
                    {
                        let mut st = s.lock();
                        st.replay_from = st.messages.len();
                        st.needs_preamble = false;
                        st.mtime = util::now();
                    }
                    acp::save_session(&s);
                    return json_ok(json!({"success": true,
                        "note": "earlier turns are no longer replayed into new processes; transcript kept"}));
                }
                _ => return json_err(404, "unknown route"),
            }
        }
        return json_err(404, "unknown route");
    }
}

/// Settings panel data for the /pi page: what Zerostack actually has.
/// Skills come from the prompts dir, MCP servers from config.toml.
/// Vault: the Worker URL + agent are fixed; the gateway cannot know the
/// unlock state (only Muse holds the master secret), so the status says so.
fn settings_info() -> Value {
    // Zerostack version, best effort.
    let version = std::process::Command::new("/root/.local/bin/zerostack")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    // Skills: prompt files in the Zerostack prompts dir.
    let mut skills: Vec<Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/root/.local/share/zerostack/prompts") {
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".md") {
                    Some(n.trim_end_matches(".md").to_string())
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        for n in names {
            let desc = std::fs::read_to_string(format!(
                "/root/.local/share/zerostack/prompts/{}.md",
                n
            ))
            .ok()
            .and_then(|t| {
                t.lines()
                    .find(|l| {
                        let l = l.trim();
                        !l.is_empty() && !l.starts_with('#')
                    })
                    .map(|l| l.trim().to_string())
            })
            .unwrap_or_default();
            skills.push(json!({"name": n, "desc": desc}));
        }
    }
    // MCP servers from config.toml [mcp_servers].
    let mut mcp: Vec<Value> = Vec::new();
    if let Ok(text) = std::fs::read_to_string(ZS_CONFIG) {
        if let Ok(v) = toml::from_str::<toml::Value>(&text) {
            if let Some(servers) = v
                .get("mcp_servers")
                .and_then(|s| s.as_table())
            {
                let mut names: Vec<String> = servers.keys().cloned().collect();
                names.sort();
                for name in names {
                    let s = &servers[&name];
                    let url = s
                        .get("url")
                        .and_then(|u| u.as_str())
                        .map(|u| u.to_string())
                        .unwrap_or_default();
                    let command = s
                        .get("command")
                        .and_then(|c| c.as_str())
                        .map(|c| c.to_string())
                        .unwrap_or_default();
                    let target = if !url.is_empty() {
                        url
                    } else {
                        command
                    };
                    mcp.push(json!({
                        "name": name,
                        "url": target,
                        "note": "TUI only — ACP never loads MCP",
                    }));
                }
            }
        }
    }
    let running = acp::sessions_list()
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    json!({
        "zerostack_version": version,
        "zerostack_bin": "/root/.local/bin/zerostack",
        "config_path": ZS_CONFIG,
        "gateway_impl": "rust",
        "gateway_pid": std::process::id(),
        "sessions_running": running,
        "acp": "gateway talks to Zerostack over ACP; the gateway owns transcripts",
        "skills": skills,
        "mcp": mcp,
        "vault_url": "https://cf-d1-vault.hardeepwalia77.workers.dev",
        "vault_agent": "zerostack-altare",
        "vault_status": "locked unless Muse unlocked it — ask Muse to unlock for a service; vault tools work in the Zerostack terminal only",
        "vault_keys": [],
    })
}

fn main() {
    std::env::set_var("PATH", format!("/root/.cargo/bin:/root/.local/bin:{}", env_or("PATH", "/usr/local/bin:/usr/bin:/bin")));
    util::load_env_file(&format!("{}/provider-keys.env", BASE));
    util::load_env_file("/root/.profile");

    let token = gw_env_read("PI_GATEWAY_TOKEN")
        .or_else(|| std::env::var("PI_GATEWAY_TOKEN").ok())
        .unwrap_or_default();
    if token.is_empty() {
        log("no PI_GATEWAY_TOKEN in gateway.env or env — exiting");
        eprintln!("no PI_GATEWAY_TOKEN in gateway.env or env — exiting");
        std::process::exit(1);
    }
    let admin_pass = gw_env_read("PI_ADMIN_PASS_SHA256")
        .or_else(|| std::env::var("PI_ADMIN_PASS_SHA256").ok())
        .filter(|s| !s.trim().is_empty());

    let _ = std::fs::create_dir_all(SESS_DIR);
    acp::load_sessions();
    stats::seed();

    let port = env_or("ZS_GATEWAY_PORT", "8766");
    let host = env_or("ZS_GATEWAY_HOST", "127.0.0.1");

    let app = Arc::new(App {
        token,
        admin_pass: Mutex::new(admin_pass),
        fails: Mutex::new((0, 0.0)),
    });

    {
        let a = app.clone();
        std::thread::spawn(move || acp::idle_reaper());
        let _ = a;
    }

    log(&format!(
        "zs-gateway (rust) listening on {}:{} — zerostack ACP, {} stored sessions",
        host,
        port,
        acp::sessions().lock().unwrap().len()
    ));
    // Warm the disk cache once in the background (the first /stats paints 0%).
    std::thread::spawn(|| {
        let _ = stats::snapshot();
    });

    let handler: http::Handler = Arc::new(move |req| handle(&app, req));
    if let Err(e) = http::serve(&format!("{}:{}", host, port), handler) {
        log(&format!("bind failed: {}", e));
        eprintln!("bind failed: {}", e);
        std::process::exit(1);
    }
    let _ = std::io::stdout().flush();
}
