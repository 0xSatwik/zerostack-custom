//! Recall — find any session/chat or lesson in milliseconds, for few tokens.
//! Rust port of recall/recall.py (B4).
//!
//! Usage:
//!   recall find <keyword>        # grep the index, print matching lines
//!   recall digest <id|name>      # print that session's digest (the recall unit)
//!   recall lesson <keyword>      # grep the lessons index, print matches
//!   recall lessons               # list all lessons (one line each)
//!   recall sessions              # list all sessions (one line each)
//!
//! Rules (see README.md):
//! - Recall reads the index + a digest. Never the full transcript.
//! - The full transcript is only read when the digest is not enough.
//! - A digest is ~200-400 tokens; a transcript is 10k+. Use the digest.
//!
//! The recall store lives next to this binary (recall/) or at
//! /root/work/recall on the box. The binary resolves its base dir the same
//! way: next to the binary first, then /root/work/recall, then the local
//! workspace copy.

use std::fs;
use std::path::{Path, PathBuf};

fn base_dir() -> PathBuf {
    // Next to the binary (recall/ sits beside it on the box).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("recall");
            if p.join("index.jsonl").exists() {
                return p;
            }
            // Binary inside the recall dir itself.
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

fn load_index(base: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Ok(txt) = fs::read_to_string(base.join("index.jsonl")) {
        for line in txt.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                out.push(v);
            }
        }
    }
    out
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn int_field(v: &serde_json::Value, key: &str) -> u64 {
    v.get(key).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn cmd_find(base: &Path, kw: &str) {
    let kw = kw.to_lowercase();
    let mut hits: Vec<serde_json::Value> = Vec::new();
    for s in load_index(base) {
        let name = str_field(&s, "name").to_lowercase();
        let sum = str_field(&s, "sum").to_lowercase();
        let topics = s
            .get("topics")
            .and_then(|t| t.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
            .to_lowercase();
        let id = str_field(&s, "id");
        if name.contains(&kw) || sum.contains(&kw) || topics.contains(&kw) || id.contains(&kw) {
            hits.push(s);
        }
    }
    if hits.is_empty() {
        println!("no session matches {:?}", kw);
        return;
    }
    for s in hits {
        let model = str_field(&s, "model");
        let model_short = model.rsplit('/').next().unwrap_or("");
        println!(
            "{} | {} | {} | {} msgs | {}",
            str_field(&s, "id"),
            str_field(&s, "name"),
            model_short,
            int_field(&s, "msgs"),
            str_field(&s, "sum")
        );
    }
}

fn cmd_digest(base: &Path, r: &str) {
    let r = r.to_lowercase();
    let index = load_index(base);
    let mut matched: Option<serde_json::Value> = None;
    for s in &index {
        if str_field(s, "id").to_lowercase() == r || str_field(s, "name").to_lowercase() == r {
            matched = Some(s.clone());
            break;
        }
    }
    if matched.is_none() {
        for s in &index {
            if str_field(s, "name").to_lowercase().contains(&r) {
                matched = Some(s.clone());
                break;
            }
        }
    }
    let m = match matched {
        Some(m) => m,
        None => {
            println!("no session {:?} — try: recall find <keyword>", r);
            return;
        }
    };
    let path = base.join("digests").join(format!("{}.md", str_field(&m, "id")));
    match fs::read_to_string(&path) {
        Ok(t) => print!("{}", t),
        Err(_) => println!(
            "no digest for {} yet (full: {})",
            str_field(&m, "name"),
            str_field(&m, "full")
        ),
    }
}

fn cmd_lesson(base: &Path, kw: &str) {
    let kw = kw.to_lowercase();
    let path = base.join("lessons-index.md");
    let txt = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            println!("no lessons index yet");
            return;
        }
    };
    let hits: Vec<&str> = txt
        .lines()
        .filter(|l| !l.trim().is_empty() && l.to_lowercase().contains(&kw))
        .collect();
    if hits.is_empty() {
        println!("no lesson matches {:?}", kw);
        return;
    }
    for l in hits {
        println!("{}", l);
    }
}

fn cmd_lessons(base: &Path) {
    match fs::read_to_string(base.join("lessons-index.md")) {
        Ok(t) => print!("{}", t),
        Err(_) => println!("no lessons index yet"),
    }
}

fn cmd_sessions(base: &Path) {
    for s in load_index(base) {
        println!(
            "{} | {} | {} msgs | {} | {}",
            str_field(&s, "id"),
            str_field(&s, "name"),
            int_field(&s, "msgs"),
            str_field(&s, "updated"),
            str_field(&s, "sum")
        );
    }
}

fn usage() -> ! {
    eprintln!("usage: recall {{find|digest|lesson|lessons|sessions}} [arg]");
    std::process::exit(2);
}

fn main() {
    let base = base_dir();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() {
        usage();
    }
    let cmd = argv[0].as_str();
    let rest = &argv[1..];
    match cmd {
        "find" if !rest.is_empty() => cmd_find(&base, &rest.join(" ")),
        "digest" if !rest.is_empty() => cmd_digest(&base, &rest[0]),
        "lesson" if !rest.is_empty() => cmd_lesson(&base, &rest.join(" ")),
        "lessons" => cmd_lessons(&base),
        "sessions" => cmd_sessions(&base),
        _ => usage(),
    }
}
