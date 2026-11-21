//! The /stats numbers zs.html has always shown: cgroup v2 CPU + RAM, workspace
//! disk against the plan quota. Read-only, cheap, all cached on the way out.
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Prev {
    usec: Option<u64>,
    ts: f64,
}

fn prev() -> &'static Mutex<Prev> {
    static P: std::sync::OnceLock<Mutex<Prev>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(Prev { usec: None, ts: 0.0 }))
}

struct WsCache {
    bytes: Option<u64>,
    ts: f64,
    busy: bool,
}

fn ws() -> &'static Mutex<WsCache> {
    static W: std::sync::OnceLock<Mutex<WsCache>> = std::sync::OnceLock::new();
    W.get_or_init(|| Mutex::new(WsCache { bytes: None, ts: 0.0, busy: false }))
}

const DISK_QUOTA_GB: f64 = 10.0;
const WS_TTL: f64 = 300.0;

fn cgroup(path: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/fs/cgroup/{}", path))
        .ok()
        .map(|s| s.trim().to_string())
}

fn cpu_quota_cores() -> Option<f64> {
    let raw = cgroup("cpu.max")?;
    if raw == "max" {
        return None;
    }
    let mut it = raw.split_whitespace();
    let q: f64 = it.next()?.parse().ok()?;
    let per: f64 = it.next()?.parse().ok()?;
    if per <= 0.0 {
        return None;
    }
    Some(q / per)
}

fn cpu_usage_usec() -> Option<u64> {
    let text = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("usage_usec") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn workspace_bytes() -> Option<u64> {
    {
        let c = ws().lock().unwrap();
        let due = c.bytes.is_none() || crate::util::now() - c.ts > WS_TTL;
        if !due || c.busy {
            return c.bytes;
        }
    }
    {
        let mut c = ws().lock().unwrap();
        c.busy = true;
    }
    let mut child = std::process::Command::new("du")
        .args(["-sxB1", "/root"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok();
    let out = child.take().and_then(|c| c.wait_with_output().ok());
    if let Some(o) = out {
        if let Ok(t) = String::from_utf8(o.stdout) {
            if let Some(first) = t.split_whitespace().next() {
                if let Ok(n) = first.parse::<u64>() {
                    let mut c = ws().lock().unwrap();
                    c.bytes = Some(n);
                    c.ts = crate::util::now();
                    c.busy = false;
                    return Some(n);
                }
            }
        }
    }
    ws().lock().unwrap().busy = false;
    ws().lock().unwrap().bytes
}

/// Warm the caches at startup so the first page paint is not "—".
pub fn seed() {
    if let Some(u) = cpu_usage_usec() {
        let mut p = prev().lock().unwrap();
        p.usec = Some(u);
        p.ts = crate::util::now();
    }
}

pub fn snapshot() -> Value {
    let now = crate::util::now();
    let quota = cpu_quota_cores();

    let cpu_pct: Option<f64> = {
        let usec = cpu_usage_usec();
        let mut p = prev().lock().unwrap();
        let mut v: Option<f64> = None;
        if let (Some(u), Some(pu)) = (usec, p.usec) {
            let dwall = now - p.ts;
            if dwall > 0.0 {
                let allowed = dwall * 1e6 * quota.unwrap_or(1.0);
                if allowed > 0.0 {
                    v = Some(((100.0 * (u.saturating_sub(pu)) as f64) / allowed).clamp(0.0, 100.0));
                }
            }
        }
        if let Some(u) = usec {
            p.usec = Some(u);
            p.ts = now;
        }
        v
    };

    let mut ram_used: Option<u64> = None;
    let mut ram_total: Option<u64> = None;
    if let (Some(cur), Some(mx)) = (cgroup("memory.current"), cgroup("memory.max")) {
        if let (Ok(c), Ok(m)) = (cur.parse::<u64>(), mx.parse::<u64>()) {
            ram_used = Some(c);
            ram_total = Some(m);
        }
    }
    if ram_total.is_none() {
        if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
            let mut total = 0u64;
            let mut avail = 0u64;
            for line in text.lines() {
                let (k, v) = match line.split_once(':') {
                    Some(x) => x,
                    None => continue,
                };
                let n: u64 = v
                    .split_whitespace()
                    .next()
                    .and_then(|x| x.parse::<u64>().ok())
                    .unwrap_or(0)
                    * 1024;
                match k {
                    "MemTotal" => total = n,
                    "MemAvailable" => avail = n,
                    _ => {}
                }
            }
            if total > 0 {
                ram_total = Some(total);
                ram_used = Some(total.saturating_sub(avail));
            }
        }
    }

    let pct = |u: Option<u64>, t: Option<u64>| -> Option<f64> {
        match (u, t) {
            (Some(u), Some(t)) if t > 0 => Some(((100.0 * u as f64) / t as f64).clamp(0.0, 100.0)),
            _ => None,
        }
    };
    let round1 = |v: f64| (v * 10.0).round() / 10.0;

    let quota_b = DISK_QUOTA_GB * 1024.0 * 1024.0 * 1024.0;
    let mut disk = json!({"pct": Value::Null});
    if let Some(b) = workspace_bytes() {
        disk = json!({
            "pct": pct(Some(b), Some(quota_b as u64)).map(round1),
            "used_gb": round1(b as f64 / (1024.0 * 1024.0 * 1024.0)),
            "total_gb": DISK_QUOTA_GB,
            "basis": "workspace(/root) vs plan quota",
        });
        if let Ok(out) = std::process::Command::new("df").args(["-B1", "/"]).output() {
            let t = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = t.lines().nth(1) {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() >= 4 {
                    let total: f64 = f[1].parse().unwrap_or(0.0);
                    let avail: f64 = f[3].parse().unwrap_or(0.0);
                    if total > 0.0 {
                        disk["host"] = json!({
                            "pct": round1(100.0 * (total - avail) / total),
                            "used_gb": round1((total - avail) / 1e9),
                            "total_gb": round1(total / 1e9),
                        });
                    }
                }
            }
        }
    }

    let gb = |n: Option<u64>| n.map(|x| round1(x as f64 / (1024.0 * 1024.0 * 1024.0)));
    let rp = pct(ram_used, ram_total);
    json!({
        "cpu": {"pct": cpu_pct.map(round1), "quota": quota.map(|q| (q * 100.0).round())},
        "ram": {"pct": rp.map(round1), "used_gb": gb(ram_used), "total_gb": gb(ram_total)},
        "disk": disk,
        "spec": {
            "ram": gb(ram_total).map(|g| format!("{} GiB", g)).unwrap_or_else(|| "6 GiB".into()),
            "cpu": quota.map(|q| format!("{}%", (q * 100.0).round())).unwrap_or_else(|| "175%".into()),
            "disk": "10 GiB",
        },
        "updated": now as i64,
    })
}

pub fn _unused(_d: Duration, _i: Instant) {}
