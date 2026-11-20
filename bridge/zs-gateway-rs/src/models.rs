//! Model registry: providers, models, keys, rotation, fallback order.
//!
//! Two files hold the state, both tiny:
//!   models.json         the registry (what the Admin page edits)
//!   key-rotation.json   {"provider": next_index} round-robin counters
//!
//! Key VALUES live only in provider-keys.env (chmod 600) and the process env.
//! No response ever carries a value: the API reports set / not set only.
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub const KEYS_FILE: &str = "/root/pi-bridge/provider-keys.env";
pub const KEY_SLOTS: usize = 10;

fn ms_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

pub fn read_json(path: &str, default: Value) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(default)
}

pub fn write_json(path: &str, v: &Value) {
    let tmp = format!("{}.tmp", path);
    if let Ok(mut f) = std::fs::File::create(&tmp) {
        let _ = f.write_all(serde_json::to_string_pretty(v).unwrap_or_default().as_bytes());
        let _ = f.flush();
    }
    let _ = std::fs::rename(&tmp, path);
}

// ---- provider-keys.env ----------------------------------------------------
fn parse_env_file(path: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = l.split_once('=') {
                let k = k.replace("export ", "").trim().to_string();
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                if !k.is_empty() {
                    out.push((k, v));
                }
            }
        }
    }
    out
}

pub fn key_value(env_name: &str) -> Option<String> {
    std::env::var(env_name).ok().filter(|v| !v.trim().is_empty()).or_else(|| {
        parse_env_file(KEYS_FILE)
            .into_iter()
            .find(|(k, _)| k == env_name)
            .map(|(_, v)| v)
            .filter(|v| !v.trim().is_empty())
    })
}

pub fn valid_env_name(n: &str) -> bool {
    !n.is_empty()
        && n.as_bytes()[0].is_ascii_uppercase()
        && n.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

pub fn save_secret(env_name: &str, value: &str) -> bool {
    if !valid_env_name(env_name) || value.is_empty() {
        return false;
    }
    let mut lines = parse_env_file(KEYS_FILE);
    let mut found = false;
    for (k, v) in lines.iter_mut() {
        if k == env_name {
            *v = value.to_string();
            found = true;
        }
    }
    if !found {
        lines.push((env_name.to_string(), value.to_string()));
    }
    let mut body = String::new();
    for (k, v) in lines {
        body.push_str(&format!("{}={}\n", k, v));
    }
    let ok = std::fs::write(KEYS_FILE, body).is_ok();
    if ok {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(KEYS_FILE, std::fs::Permissions::from_mode(0o600));
        std::env::set_var(env_name, value);
    }
    ok
}

pub fn remove_secret(env_name: &str) -> bool {
    if !valid_env_name(env_name) {
        return false;
    }
    let mut lines = parse_env_file(KEYS_FILE);
    let before = lines.len();
    lines.retain(|(k, _)| k != env_name);
    if lines.len() == before {
        return false;
    }
    let mut body = String::new();
    for (k, v) in lines {
        body.push_str(&format!("{}={}\n", k, v));
    }
    let ok = std::fs::write(KEYS_FILE, body).is_ok();
    if ok {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(KEYS_FILE, std::fs::Permissions::from_mode(0o600));
        std::env::remove_var(env_name);
    }
    ok
}

pub fn env_for_provider(name: &str) -> String {
    let mut s = String::new();
    let mut prev_us = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_uppercase());
            prev_us = false;
        } else if !prev_us {
            s.push('_');
            prev_us = true;
        }
    }
    let t = s.trim_matches('_').to_string();
    format!("{}_API_KEY", if t.is_empty() { "PROVIDER".to_string() } else { t })
}

// ---- registry -------------------------------------------------------------
pub struct Registry(pub Value);

impl Registry {
    pub fn providers(&self) -> Map<String, Value> {
        self.0
            .get("providers")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default()
    }
    pub fn order(&self) -> Vec<String> {
        self.0
            .get("order")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }
    pub fn fallback_enabled(&self) -> bool {
        self.0.get("fallback_enabled").and_then(|v| v.as_bool()).unwrap_or(true)
    }
    pub fn provider(&self, name: &str) -> Option<Value> {
        self.providers().get(name).cloned()
    }
    pub fn models_of(&self, name: &str) -> Vec<Value> {
        self.provider(name)
            .and_then(|p| p.get("models").and_then(|m| m.as_array()).cloned())
            .unwrap_or_default()
    }
    pub fn model_meta(&self, p: &str, id: &str) -> (i64, i64) {
        for m in self.models_of(p) {
            if m.get("id").and_then(|x| x.as_str()) == Some(id) {
                return (
                    m.get("context").and_then(|v| v.as_i64()).unwrap_or(0),
                    m.get("output").and_then(|v| v.as_i64()).unwrap_or(0),
                );
            }
        }
        (0, 0)
    }
}

pub fn registry() -> Registry {
    let _g = ms_lock().lock().unwrap();
    let mut reg = read_json(crate::MODELS_FILE, Value::Null);
    let ok = reg
        .get("providers")
        .map(|p| p.is_object())
        .unwrap_or(false);
    if !ok {
        reg = seed_registry();
        write_json(crate::MODELS_FILE, &reg);
    }
    if reg.get("order").is_none() {
        reg["order"] = json!([]);
    }
    if reg.get("fallback_enabled").is_none() {
        reg["fallback_enabled"] = json!(true);
    }
    Registry(reg)
}

/// First run without models.json: derive the registry from config.toml so the
/// box keeps exactly the models it already had.
fn seed_registry() -> Value {
    let cfg = crate::zs_config();
    let ctx = cfg.get("context_window").and_then(|v| v.as_i64()).unwrap_or(0);
    let out = cfg.get("max_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut provs: BTreeMap<String, Value> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    if let Some(cps) = cfg.get("custom_providers").and_then(|v| v.as_object()) {
        for (name, cp) in cps {
            let base = cp.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
            let envn = cp
                .get("api_key_env")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| env_for_provider(name));
            let mut models = Vec::new();
            if let Some(m) = cp.get("model").and_then(|v| v.as_str()) {
                models.push(json!({"id": m, "context": ctx, "output": out}));
            }
            provs.insert(
                name.clone(),
                json!({
                    "display": name, "base_url": base, "api_key_env": envn,
                    "api_style": cp.get("api_style").and_then(|v| v.as_str()).unwrap_or(""),
                    "models": models
                }),
            );
        }
    }

    let mut qm_pairs: Vec<(String, String)> = Vec::new();
    if let Some(qms) = cfg.get("quick_models").and_then(|v| v.as_object()) {
        for (_q, qm) in qms {
            let p = qm.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let m = qm.get("model").and_then(|v| v.as_str()).unwrap_or("");
            if p.is_empty() || m.is_empty() {
                continue;
            }
            qm_pairs.push((p.to_string(), m.to_string()));
            let e = provs.entry(p.to_string()).or_insert_with(|| {
                json!({"display": p, "base_url": "", "api_key_env": env_for_provider(p),
                       "api_style": "", "models": []})
            });
            let has = e["models"]
                .as_array()
                .map(|a| a.iter().any(|x| x.get("id").and_then(|v| v.as_str()) == Some(m)))
                .unwrap_or(false);
            if !has {
                e["models"].as_array_mut().unwrap().push(json!({"id": m, "context": ctx, "output": out}));
            }
        }
    }

    let dp = cfg.get("provider").and_then(|v| v.as_str()).unwrap_or("");
    let dm = cfg.get("model").and_then(|v| v.as_str()).unwrap_or("");
    if !dp.is_empty() && !dm.is_empty() {
        order.push(format!("{}/{}", dp, dm));
    }
    for (p, m) in &qm_pairs {
        let k = format!("{}/{}", p, m);
        if !order.contains(&k) {
            order.push(k);
        }
    }
    for (p, e) in provs.iter() {
        for m in e["models"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                let k = format!("{}/{}", p, id);
                if !order.contains(&k) {
                    order.push(k);
                }
            }
        }
    }
    json!({"providers": provs, "order": order, "fallback_enabled": true})
}

pub fn save_registry(reg: &Value) -> Result<(), String> {
    let _g = ms_lock().lock().unwrap();
    write_json(crate::MODELS_FILE, reg);
    sync_config(reg)
}

/// Keep only order entries that still name a real model, and append any model
/// missing from the order so every model stays reachable.
pub fn normalize(reg: &mut Value) {
    let providers = reg.get("providers").cloned().unwrap_or(json!({}));
    let mut known: Vec<String> = Vec::new();
    if let Some(p) = providers.as_object() {
        for (name, pv) in p {
            for m in pv.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                    known.push(format!("{}/{}", name, id));
                }
            }
        }
    }
    let mut order: Vec<String> = reg
        .get("order")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    order.retain(|k| known.contains(k));
    let mut seen = std::collections::HashSet::new();
    order.retain(|k| seen.insert(k.clone()));
    for k in &known {
        if !order.contains(k) {
            order.push(k.clone());
        }
    }
    reg["order"] = json!(order);
}

// ---- keys -----------------------------------------------------------------
pub fn provider_keys(env_base: &str) -> Vec<Value> {
    let mut out = Vec::new();
    if env_base.is_empty() {
        return out;
    }
    out.push(json!({"env": env_base, "set": key_value(env_base).is_some()}));
    for i in 2..=KEY_SLOTS {
        let n = format!("{}_{}", env_base, i);
        if key_value(&n).is_some() {
            out.push(json!({"env": n, "set": true}));
        }
    }
    out
}

/// Round-robin over the SET keys of one provider. Counter persisted, tiny file.
pub fn rotation_next(provider: &str, keys: &[Value]) -> Option<String> {
    let usable: Vec<String> = keys
        .iter()
        .filter(|k| k.get("set").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|k| k.get("env").and_then(|v| v.as_str()).map(String::from))
        .collect();
    if usable.is_empty() {
        return None;
    }
    let _g = ms_lock().lock().unwrap();
    let mut rot = read_json(crate::ROTATION_FILE, json!({}));
    let idx = rot
        .get(provider)
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as usize;
    let pick = usable[idx % usable.len()].clone();
    rot[provider] = json!((idx + 1) % usable.len());
    write_json(crate::ROTATION_FILE, &rot);
    Some(pick)
}

/// Next free key slot for a provider (PROVIDER_API_KEY, _2, _3 ...).
pub fn next_key_slot(env_base: &str) -> Option<String> {
    if key_value(env_base).is_none() {
        return Some(env_base.to_string());
    }
    for i in 2..=KEY_SLOTS {
        let n = format!("{}_{}", env_base, i);
        if key_value(&n).is_none() {
            return Some(n);
        }
    }
    None
}

// ---- outbound /models fetch ----------------------------------------------
/// GET {base_url}/models with the user's key. Sorted ids + context hints.
/// Uses curl (one-shot, TLS handled by the system library) so the gateway
/// binary stays dependency-free and small; results are cached briefly because
/// the Admin page reloads the provider list on every tab switch.
pub fn fetch_provider_models(base_url: &str, api_key: &str) -> Result<Vec<Value>, String> {
    let base = base_url.trim().trim_end_matches('/');
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err("base URL must start with http:// or https://".into());
    }
    let url = format!("{}/models", base);
    let out = std::process::Command::new("curl")
        .args([
            "-sS", "--max-time", "25", "-H", "Accept: application/json",
            "-H", &format!("Authorization: Bearer {}", api_key), &url,
        ])
        .output()
        .map_err(|e| format!("could not run curl: {}", e))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(format!("request failed: {}", msg.trim()));
    }
    let v: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "provider did not return JSON (check the base URL)".to_string())?;
    let items = if let Some(a) = v.get("data").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.get("models").and_then(|x| x.as_array()) {
        a.clone()
    } else if let Some(a) = v.as_array() {
        a.clone()
    } else {
        return Err("no model list in the provider response".into());
    };
    let mut out_models: Vec<(String, i64)> = Vec::new();
    for it in items {
        if let Some(id) = it.as_str() {
            out_models.push((id.to_string(), 0));
            continue;
        }
        let id = match it.get("id").and_then(|x| x.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let mut ctx = 0i64;
        for k in ["context_length", "context_window", "max_context", "max_model_len"] {
            if let Some(n) = it.get(k).and_then(|x| x.as_i64()) {
                ctx = n;
                break;
            }
        }
        out_models.push((id, ctx));
    }
    out_models.sort();
    out_models.dedup();
    Ok(out_models
        .into_iter()
        .map(|(id, ctx)| json!({"id": id, "context": ctx, "output": 0}))
        .collect())
}

// ---- legacy views ---------------------------------------------------------
/// The shape zs.html has always consumed from GET /models.
pub fn models_payload() -> Value {
    let cfg = crate::zs_config();
    let ctx = cfg.get("context_window").and_then(|v| v.as_i64());
    let out = cfg.get("max_tokens").and_then(|v| v.as_i64());
    let mut provs: BTreeMap<String, Value> = BTreeMap::new();
    let reg = registry();

    for (name, p) in reg.providers() {
        let mut models = Vec::new();
        for m in p.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
            let id = match m.get("id").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let mut e = json!({"id": id});
            let c = m.get("context").and_then(|v| v.as_i64()).filter(|v| *v > 0).or(ctx);
            let o = m.get("output").and_then(|v| v.as_i64()).filter(|v| *v > 0).or(out);
            if let Some(c) = c {
                e["contextWindow"] = json!(c);
            }
            if let Some(o) = o {
                e["maxTokens"] = json!(o);
            }
            models.push(e);
        }
        let envn = p.get("api_key_env").and_then(|v| v.as_str()).unwrap_or("");
        provs.insert(
            name,
            json!({
                "models": models,
                "baseUrl": p.get("base_url").cloned().unwrap_or(json!("")),
                "apiKey": if envn.is_empty() { json!("") } else { json!(format!("${}", envn)) },
            }),
        );
    }

    let order = reg.order();
    let (dp, dm) = order
        .first()
        .and_then(|k| k.split_once('/').map(|(a, b)| (a.to_string(), b.to_string())))
        .unwrap_or((String::new(), String::new()));
    json!({"providers": provs, "default": {"provider": dp, "model": dm}})
}

/// Full registry view for the Admin page. Never contains a key value.
pub fn model_system_payload() -> Value {
    let reg = registry();
    let mut provs = Vec::new();
    let mut names: Vec<String> = reg.providers().keys().cloned().collect();
    names.sort();
    let mut all_models: Vec<Value> = Vec::new();
    for name in &names {
        let p = reg.provider(name).unwrap_or(json!({}));
        let envn = p.get("api_key_env").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let models = p.get("models").cloned().unwrap_or(json!([]));
        for m in models.as_array().cloned().unwrap_or_default() {
            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                all_models.push(json!({
                    "key": format!("{}/{}", name, id),
                    "provider": name,
                    "id": id,
                    "display": p.get("display").and_then(|v| v.as_str()).unwrap_or(name),
                    "context": m.get("context").and_then(|v| v.as_i64()).unwrap_or(0),
                    "output": m.get("output").and_then(|v| v.as_i64()).unwrap_or(0),
                }));
            }
        }
        provs.push(json!({
            "name": name,
            "display": p.get("display").and_then(|v| v.as_str()).unwrap_or(name),
            "base_url": p.get("base_url").and_then(|v| v.as_str()).unwrap_or(""),
            "api_style": p.get("api_style").and_then(|v| v.as_str()).unwrap_or(""),
            "api_key_env": envn,
            "keys": provider_keys(&envn),
            "models": models,
        }));
    }
    let order = reg.order();
    let filtered: Vec<String> = order.clone();
    json!({
        "providers": provs,
        "models": all_models,
        "order": filtered,
        "default": filtered.first().cloned().unwrap_or_default(),
        "fallback_enabled": reg.fallback_enabled(),
    })
}

/// [current model] + the rest of the order, or just the current one when the
/// fallback toggle is off.
pub fn fallback_chain(current: &str) -> Vec<String> {
    let reg = registry();
    let mut chain: Vec<String> = Vec::new();
    if !current.is_empty() {
        chain.push(current.to_string());
    }
    if reg.fallback_enabled() {
        for k in reg.order() {
            if !k.is_empty() && !chain.contains(&k) {
                chain.push(k);
            }
        }
    }
    chain
}

// ---- config.toml projection ----------------------------------------------
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut prev = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev = false;
        } else if !prev {
            out.push('-');
            prev = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Write the registry into config.toml (root default + fallback keys, one
/// [custom_providers.*] block per provider, quick models in fallback order).
/// Everything else in the file (comments, mcp_servers, ...) is preserved.
pub fn sync_config(reg: &Value) -> Result<(), String> {
    let path = crate::ZS_CONFIG;
    let text = std::fs::read_to_string(path).map_err(|e| format!("read config: {}", e))?;

    // Drop the managed region and every custom_providers block.
    let mut cleaned = String::new();
    let mut skip_region = false;
    for line in text.lines() {
        if line.contains("# --- BEGIN gateway models ---") {
            skip_region = true;
            continue;
        }
        if line.contains("# --- END gateway models ---") {
            skip_region = false;
            continue;
        }
        if skip_region {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }
    let mut blocks_out: Vec<String> = Vec::new();
    let mut in_cp = false;
    for line in cleaned.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            in_cp = t.starts_with("[custom_providers.");
        }
        if in_cp {
            continue;
        }
        blocks_out.push(line.to_string());
    }
    let cleaned = blocks_out.join("\n");

    let (head, tail) = match cleaned.split_once("\n[") {
        Some((h, t)) => (h.to_string(), format!("[{}", t)),
        None => (cleaned.clone(), String::new()),
    };

    let order: Vec<String> = reg
        .get("order")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();

    let mut head_lines: Vec<String> = head.lines().map(|l| l.to_string()).collect();
    let set_root = |lines: &mut Vec<String>, key: &str, val: String| {
        let pat = format!("{} ", key);
        let pat2 = format!("{}=", key);
        if let Some(i) = lines.iter().position(|l| {
            let l = l.trim_start();
            l.starts_with(&pat) || l.starts_with(&pat2)
        }) {
            lines[i] = format!("{} = {}", key, val);
        } else {
            lines.push(format!("{} = {}", key, val));
        }
    };
    if let Some(first) = order.first() {
        if let Some((p, m)) = first.split_once('/') {
            set_root(&mut head_lines, "provider", toml_str(p));
            set_root(&mut head_lines, "model", toml_str(m));
        }
    }
    let fb = reg.get("fallback_enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    set_root(&mut head_lines, "fallback_enabled", fb.to_string());
    set_root(
        &mut head_lines,
        "fallback_order",
        format!("[{}]", order.iter().map(|k| toml_str(k)).collect::<Vec<_>>().join(", ")),
    );
    let head = head_lines.join("\n");

    // Reuse an existing quick-model name for the same provider/model pair.
    let mut existing: HashMap<(String, String), String> = HashMap::new();
    {
        let mut cur: Option<String> = None;
        for line in tail.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("[quick_models.") {
                cur = Some(rest.trim_end_matches(']').trim_matches('"').to_string());
                continue;
            }
            if t.starts_with('[') {
                cur = None;
                continue;
            }
            if let Some(name) = &cur {
                if let Some((k, v)) = t.split_once('=') {
                    let key = k.trim();
                    let val = v.trim().trim_matches('"');
                    if key == "provider" || key == "model" {
                        let e = existing.entry((name.clone(), String::new())).or_default();
                        let _ = e;
                    }
                    let _ = val;
                }
            }
        }
    }
    // Pair-wise read of quick_models from the parsed config (simpler, exact).
    let cfg = crate::zs_config();
    if let Some(qms) = cfg.get("quick_models").and_then(|v| v.as_object()) {
        for (name, qm) in qms {
            let p = qm.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let m = qm.get("model").and_then(|v| v.as_str()).unwrap_or("");
            if !p.is_empty() && !m.is_empty() {
                existing.insert((p.to_string(), m.to_string()), name.clone());
            }
        }
    }

    // Rebuild the quick_models blocks in fallback order under [quick_models.x].
    let mut body = String::new();
    body.push_str("\n# --- BEGIN gateway models ---\n");
    let mut used_names: Vec<String> = Vec::new();
    let mut quick: Vec<String> = Vec::new();
    for key in &order {
        let (p, mid) = match key.split_once('/') {
            Some(x) => x,
            None => continue,
        };
        let mut name = existing
            .get(&(p.to_string(), mid.to_string()))
            .cloned()
            .unwrap_or_else(|| slug(mid));
        if name.is_empty() {
            name = slug(mid);
        }
        let mut n = name.clone();
        let mut i = 2;
        while used_names.contains(&n) {
            n = format!("{}-{}", name, i);
            i += 1;
        }
        used_names.push(n.clone());
        quick.push(format!(
            "\n[quick_models.{}]\nprovider = {}\nmodel = {}\n",
            n,
            toml_str(p),
            toml_str(mid)
        ));
    }

    let mut prov_blocks: Vec<String> = Vec::new();
    if let Some(provs) = reg.get("providers").and_then(|v| v.as_object()) {
        for (name, p) in provs {
            let base = p.get("base_url").and_then(|v| v.as_str()).unwrap_or("");
            if base.is_empty() {
                continue;
            }
            let mut b = format!(
                "\n[custom_providers.{}]\nprovider_type = \"openai\"\nbase_url = {}\napi_key_env = {}\n",
                name,
                toml_str(base),
                toml_str(p.get("api_key_env").and_then(|v| v.as_str()).unwrap_or(&env_for_provider(name)))
            );
            if let Some(style) = p.get("api_style").and_then(|v| v.as_str()) {
                if !style.is_empty() {
                    b.push_str(&format!("api_style = {}\n", toml_str(style)));
                }
            }
            if let Some(first) = p
                .get("models")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
            {
                b.push_str(&format!("model = {}\n", toml_str(first)));
            }
            prov_blocks.push(b);
        }
    }
    body.push_str(&quick.join(""));
    for b in &prov_blocks {
        body.push_str(b);
    }
    body.push_str("# --- END gateway models ---\n");

    // Keep whatever followed the custom_providers blocks we removed (mcp_servers,
    // other tables). quick_models blocks are re-emitted above, so drop them here.
    let mut rest = String::new();
    let mut keep = false;
    for line in tail.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            keep = !(t.starts_with("[quick_models.") || t.starts_with("[custom_providers."));
        }
        if keep {
            rest.push_str(line);
            rest.push('\n');
        }
    }

    let mut out = head.trim_end().to_string();
    out.push('\n');
    out.push_str(body.trim_end());
    out.push('\n');
    if !rest.trim().is_empty() {
        out.push('\n');
        out.push_str(rest.trim_end());
        out.push('\n');
    }

    // Validate as TOML before writing; keep one .bak.
    toml::from_str::<toml::Value>(&out).map_err(|e| format!("refusing to write invalid config: {}", e))?;
    if Path::new(path).exists() {
        let _ = std::fs::write(format!("{}.bak", path), &text);
    }
    std::fs::write(path, out).map_err(|e| format!("write config: {}", e))?;
    crate::log(&format!(
        "config.toml projected from models.json ({} providers, {} quick models)",
        prov_blocks.len(),
        quick.len()
    ));
    Ok(())
}

pub fn split_model(model: &str) -> (Option<String>, Option<String>) {
    if model.is_empty() {
        return (None, None);
    }
    match model.split_once('/') {
        Some((p, m)) => (Some(p.to_string()), Some(m.to_string())),
        None => (None, Some(model.to_string())),
    }
}
