use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub fn log(msg: &str) {
    use std::io::IsTerminal;
    let t = UNIX_EPOCH.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    let (secs, _) = (t, 0);
    // local-ish timestamp: keep it simple and monotonic-friendly
    let line = format!("{} {} (epoch {})\n", date_str(), msg, secs);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{}/zs-gateway.log", crate::BASE))
    {
        let _ = f.write_all(line.as_bytes());
    }
    if std::io::stderr().is_terminal() {
        let _ = std::io::stderr().write_all(line.as_bytes());
    }
}

fn date_str() -> String {
    // Days since epoch -> Y-M-D, no chrono dependency.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let mut y = 1970i64;
    let mut d = days;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let yd = if leap { 366 } else { 365 };
        if d < yd {
            break;
        }
        d -= yd;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let mdays = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0usize;
    while m < 12 && d >= mdays[m] {
        d -= mdays[m];
        m += 1;
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m + 1, d + 1, h, mi, s)
}

pub fn load_env_file(path: &str) {
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') || !l.contains('=') {
                continue;
            }
            let (k, v) = match l.split_once('=') {
                Some(x) => x,
                None => continue,
            };
            let k = k.replace("export ", "").trim().to_string();
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !k.is_empty() && !v.is_empty() && !k.starts_with('-') && std::env::var(&k).is_err() {
                std::env::set_var(k, v);
            }
        }
    }
}
