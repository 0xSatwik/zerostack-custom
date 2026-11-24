//! Zerostack bridge watcher — runs on the Altare sandbox 'pi' as root.
//! Rust port of /root/pi-bridge/watcher.py (B4).
//!
//! Polls /root/pi-bridge/inbox/*.json, runs Zerostack (zs ask general-work)
//! on each job, writes the result to /root/pi-bridge/outbox/<id>.json.
//! Muse (on the other side) talks to it only through the Altare Files API,
//! so no SSH into the sandbox is needed.
//!
//! Start once, in tmux (sandbox has no systemd):
//!   tmux new -s pi-bridge -d '/root/pi-bridge/watcher'
//!   # after a sandbox restart, run that again — restarts stop everything.
//! Job JSON: {"id": "...", "model": "moss/nex-n2.6", "prompt": "...",
//!            "timeout": 1800, "cwd": "/root/work/<id>"}
//! Out JSON: {"id": "...", "status": "done|error|timeout",
//!            "output": "...", "stderr": "...", "exit_code": 0,
//!            "started_at": "...", "finished_at": "..."}

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const BASE: &str = "/root/pi-bridge";
const INBOX: &str = "/root/pi-bridge/inbox";
const OUTBOX: &str = "/root/pi-bridge/outbox";
const DONE: &str = "/root/pi-bridge/done";
const WORK: &str = "/root/work";
const LOG: &str = "/root/pi-bridge/watcher.log";
const ZS: &str = "/root/pi-bridge/zs";
const POLL_SECS: u64 = 5;

fn now_iso() -> String {
    // UTC ISO-8601 like Python's datetime.now(timezone.utc).isoformat().
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00", y, mo, d, h, mi, s)
}

fn unix_to_ymd_hms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let s = rem % 60;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y as u64, mo, d, h, mi, s)
}

fn log(msg: &str) {
    let line = format!("{} {}", now_iso(), msg);
    println!("{}", line);
    let _ = std::io::stdout().flush();
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(LOG) {
        let _ = writeln!(f, "{}", line);
    }
}

fn load_profile_env() {
    // Zerostack reads API keys from env; they live as exports in
    // /root/.profile and /root/.bashrc.
    for path in ["/root/.profile", "/root/.bashrc"] {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("export ") {
                    if let Some((k, v)) = rest.split_once('=') {
                        let k = k.trim();
                        let v = v.trim().trim_matches('"').trim_matches('\'');
                        if std::env::var_os(k).is_none() {
                            std::env::set_var(k, v);
                        }
                    }
                }
            }
        }
    }
}

/// Run a command with a timeout (seconds). Returns (exit_code, stdout, stderr).
/// exit_code is None on timeout (child killed).
fn run_with_timeout(cmd: &str, args: &[String], cwd: &Path, timeout_secs: u64) -> (Option<i32>, String, String) {
    let mut child = match Command::new(cmd)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return (None, String::new(), format!("spawn failed: {}", e)),
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let code = status.code();
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut so) = child.stdout.take() {
                    use std::io::Read;
                    let _ = so.read_to_string(&mut out);
                }
                if let Some(mut se) = child.stderr.take() {
                    use std::io::Read;
                    let _ = se.read_to_string(&mut err);
                }
                return (code, out, err);
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let mut out = String::new();
                    let mut err = String::new();
                    if let Some(mut so) = child.stdout.take() {
                        use std::io::Read;
                        let _ = so.read_to_string(&mut out);
                    }
                    if let Some(mut se) = child.stderr.take() {
                        use std::io::Read;
                        let _ = se.read_to_string(&mut err);
                    }
                    return (None, out, err);
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return (None, String::new(), format!("wait failed: {}", e)),
        }
    }
}

fn ensure_session() {
    // The gateway's session store can lose sessions (older standing sessions
    // vanished once); if general-work is gone, recreate it so `ask` works.
    let (code, _, _) = run_with_timeout(ZS, &["status".to_string(), "general-work".to_string()], Path::new(WORK), 30);
    if code != Some(0) {
        log("general-work missing, recreating it");
        let _ = run_with_timeout(
            ZS,
            &["new".to_string(), "general-work".to_string(), "--brief".to_string(), "Ready.".to_string(), "--wait".to_string(), "30".to_string()],
            Path::new(WORK),
            60,
        );
    }
}

fn run_job(job: &serde_json::Value) -> serde_json::Value {
    let jid = job.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let model = job.get("model").and_then(|v| v.as_str()).unwrap_or("moss/nex-n2.6");
    let prompt = job.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
    let timeout = job.get("timeout").and_then(|v| v.as_u64()).unwrap_or(1800);
    let cwd_str = job
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{}/{}", WORK, jid));
    let cwd = PathBuf::from(&cwd_str);
    let _ = fs::create_dir_all(&cwd);
    let started = now_iso();
    log(&format!("START {} model={} cwd={}", jid, model, cwd.display()));

    ensure_session();
    let args: Vec<String> = vec![
        "ask".to_string(),
        "general-work".to_string(),
        prompt.to_string(),
        "--wait".to_string(),
        timeout.to_string(),
    ];
    let (code, stdout, stderr) = run_with_timeout(ZS, &args, &cwd, timeout);
    let finished = now_iso();
    // zs ask exits 0 on a reply, 1 on provider/tool error, 2 on wait timeout.
    let (status, exit_code) = match code {
        Some(0) => ("done", serde_json::json!(0)),
        Some(2) => ("timeout", serde_json::json!(2)),
        Some(c) => ("error", serde_json::json!(c)),
        None => ("timeout", serde_json::Value::Null),
    };
    let stderr_tail: String = stderr.chars().rev().take(4000).collect::<String>().chars().rev().collect();
    let out = serde_json::json!({
        "id": jid,
        "status": status,
        "output": stdout,
        "stderr": if code.is_none() { format!("timed out after {}s", timeout) } else { stderr_tail },
        "exit_code": exit_code,
        "started_at": started,
        "finished_at": finished,
    });
    let out_path = format!("{}/{}.json", OUTBOX, jid);
    let _ = fs::write(&out_path, serde_json::to_string_pretty(&out).unwrap_or_default());
    log(&format!("END {} status={} exit={}", jid, status, exit_code));
    out
}

fn main() {
    for d in [INBOX, OUTBOX, DONE, WORK] {
        let _ = fs::create_dir_all(d);
    }
    load_profile_env();
    log(&format!("watcher up, polling {}", INBOX));
    loop {
        // Collect inbox *.json sorted by mtime (oldest first).
        let mut jobs: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(INBOX) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) == Some("json") {
                    jobs.push(p);
                }
            }
        }
        jobs.sort_by_key(|p| {
            fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
        });
        for f in jobs {
            let content = match fs::read_to_string(&f) {
                Ok(c) => c,
                Err(e) => {
                    log(&format!("BAD {}: {}", f.file_name().unwrap_or_default().to_string_lossy(), e));
                    let _ = fs::rename(&f, format!("{}/{}", DONE, f.file_name().unwrap_or_default().to_string_lossy()));
                    continue;
                }
            };
            let job: serde_json::Value = match serde_json::from_str(&content) {
                Ok(j) => j,
                Err(e) => {
                    log(&format!("BAD {}: {}", f.file_name().unwrap_or_default().to_string_lossy(), e));
                    let _ = fs::rename(&f, format!("{}/{}", DONE, f.file_name().unwrap_or_default().to_string_lossy()));
                    continue;
                }
            };
            let name = f.file_name().unwrap_or_default().to_string_lossy().to_string();
            let _ = fs::rename(&f, format!("{}/{}", DONE, name));
            run_job(&job);
        }
        std::thread::sleep(Duration::from_secs(POLL_SECS));
    }
}
