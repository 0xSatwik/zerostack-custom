#!/usr/bin/env python3
"""Auto-digest updater for the recall system.

Keeps digests + index current WITHOUT the agent having to remember.
Runs via cron every 6h (and on demand: python3 auto_digest.py).

What it does, for each session in the recall index:
  - Session file GONE (user deleted the chat) -> delete its digest + index line.
    (Deleting a session deletes its memory. That is the rule.)
  - Session has NEW messages since the digest was written -> summarize ONLY
    the new messages (the delta) with a cheap model and merge into the digest.
  - New session file not in the index -> add it + create an initial digest.
  - Nothing changed -> skip (no model call, no write).

Design notes (why it stays cheap + tiny):
  - Delta-only: the model sees ONLY the new messages + the old digest (~1-2 KB),
    never the full transcript. A digest update costs a few hundred tokens.
  - Cursor: digests/.cursor.json maps short-id -> message count the digest
    already covers. The digest is the recall unit; the transcript is ground truth.
  - Model failure is NOT fatal: the session is marked stale and retried next run.
    Nothing is deleted or overwritten on a failed model call.
  - muse-main is NOT touched by this script — Muse updates that digest itself
    (AGENTS.md standing rule) because that work lives in the Muse chat, not in
    a Zerostack session file.

Provider keys: read from /root/pi-bridge/provider-keys.env (HMS_API_KEY) and
/root/.profile + /root/.bashrc (TOKENLAB_API_KEY, MOSS_API_KEY). Keys are never
printed or written anywhere. Order: 100milionseek, tokenlab, moss.
"""
import json
import os
import re
import subprocess
import sys
import urllib.request
from datetime import date
from pathlib import Path

BASE = Path("/root/work/recall")
SESS_DIR = Path("/root/pi-bridge/zs-sessions")
INDEX = BASE / "index.jsonl"
DIGESTS = BASE / "digests"
CURSOR = DIGESTS / ".cursor.json"
LOG = BASE / "auto_digest.log"

# Short ids the script never auto-updates (Muse updates these itself).
SKIP_AUTO = {"muse-main"}

DIGEST_FORMAT = """# {name}
- id: {sid}, model: {model}, msgs: {msgs}, updated: {today}
- purpose: <one line>
- decisions: <key decisions, comma-separated>
- facts: <key facts, comma-separated>
- outcomes: <what got done / verified>
- open: <open items, or none>
- full: {full_path}"""

UPDATE_PROMPT = """You update a session digest for a recall system. The digest is the ONLY thing read to recall a session, so it must stay short (under ~150 words) and factual.

OLD DIGEST:
{old_digest}

NEW MESSAGES since that digest (most recent last):
{new_text}

Output ONLY the updated digest, in exactly this format (keep the same id, name, and full: path; update msgs: to {msgs} and updated: to {today}):
{format}

Rules: keep it under ~150 words; only facts from the messages above; drop nothing important from the old digest; add the new decisions/facts/outcomes; if nothing important happened, return the old digest with only msgs:/updated: changed."""

NEW_PROMPT = """You write a session digest for a recall system. The digest is the ONLY thing read to recall a session, so it must stay short (under ~150 words) and factual.

SESSION MESSAGES (first few):
{new_text}

Output ONLY the digest, in exactly this format:
{format}

Rules: keep it under ~150 words; only facts from the messages above; name the session from its first user message."""

PROVIDERS = [
    # (name, base_url, model, env_key, key_file)
    ("100milionseek", "https://100milionseek.top/v1", "deepseek-v4.1-flash-free",
     "HMS_API_KEY", "/root/pi-bridge/provider-keys.env"),
    ("tokenlab", "https://api.tokenlab.amd.com/v1", "DeepSeek-V4.1-Flash",
     "TOKENLAB_API_KEY", None),
    ("moss", "https://api.mosi.cn/v1", "nex-n2.6",
     "MOSS_API_KEY", None),
]


def log(msg):
    line = f"{date.today()} {msg}"
    print(line)
    try:
        with open(LOG, "a") as f:
            f.write(line + "\n")
    except OSError:
        pass


def load_env_files():
    """Collect provider keys from provider-keys.env + shell profiles."""
    keys = {}
    for path in ("/root/pi-bridge/provider-keys.env", "/root/.profile", "/root/.bashrc"):
        try:
            for line in open(path, errors="ignore"):
                m = re.match(r'^\s*(?:export\s+)?([A-Z_]+)=["\']?([^"\'\n]+)', line)
                if m and m.group(1) not in keys:
                    keys[m.group(1)] = m.group(2).strip()
        except OSError:
            pass
    # Also honour the live environment.
    for k, v in os.environ.items():
        keys.setdefault(k, v)
    return keys


def call_model(prompt, keys):
    """Try each provider in order; return the model's text or None."""
    body = json.dumps({
        "model": None,  # set per provider below
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 900,
        "temperature": 0.2,
    }).encode()
    for name, base, model, env_key, _ in PROVIDERS:
        key = keys.get(env_key)
        if not key:
            continue
        payload = json.loads(body)
        payload["model"] = model
        req = urllib.request.Request(
            base.rstrip("/") + "/chat/completions",
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json",
                     "Authorization": "Bearer " + key},
            method="POST")
        try:
            with urllib.request.urlopen(req, timeout=90) as r:
                data = json.load(r)
            text = data["choices"][0]["message"]["content"].strip()
            if text:
                return text
        except Exception as e:
            log(f"  model {name} failed: {type(e).__name__}")
    return None


def msg_text(m):
    """Render one session message as short plain text."""
    role = m.get("role", "?")
    c = m.get("content", "")
    if isinstance(c, list):
        parts = []
        for p in c:
            if isinstance(p, dict):
                if p.get("type") == "text" and p.get("text"):
                    parts.append(str(p["text"]))
                elif p.get("type") == "toolResult":
                    t = p.get("text", "")
                    if isinstance(t, list):
                        t = " ".join(str(x.get("text", "")) for x in t if isinstance(x, dict))
                    parts.append("[tool] " + str(t)[:300])
        c = " ".join(parts)
    return f"[{role}] {str(c)[:600]}"


def session_files():
    """Map short id -> (full id, path) for every session json on disk."""
    out = {}
    for p in sorted(SESS_DIR.glob("*.json")):
        full = p.stem
        out[full[:8]] = (full, p)
    return out


def read_index():
    rows = []
    try:
        for line in open(INDEX):
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    except OSError:
        pass
    return rows


def write_index(rows):
    with open(INDEX, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")


def load_cursor():
    try:
        return json.loads(open(CURSOR).read())
    except (OSError, ValueError):
        return {}


def save_cursor(c):
    with open(CURSOR, "w") as f:
        json.dump(c, f, indent=1, sort_keys=True)


def main():
    BASE.mkdir(parents=True, exist_ok=True)
    DIGESTS.mkdir(parents=True, exist_ok=True)
    rows = read_index()
    files = session_files()
    cursor = load_cursor()
    keys = load_env_files()
    today = str(date.today())
    changed = False

    # --- 1. sessions in the index: deleted? updated? ---
    kept = []
    for row in rows:
        sid = row["id"]
        if sid in SKIP_AUTO:
            kept.append(row)
            continue
        if sid not in files:
            # Session file gone = user deleted the chat -> delete its memory.
            dp = DIGESTS / f"{sid}.md"
            if dp.exists():
                dp.unlink()
            cursor.pop(sid, None)
            log(f"deleted {sid} ({row.get('name','?')}): session gone, digest + index entry removed")
            changed = True
            continue
        full, path = files[sid]
        try:
            msgs = json.load(open(path)).get("messages", [])
        except (OSError, ValueError):
            kept.append(row)
            continue
        n = len(msgs)
        covered = cursor.get(sid, row.get("msgs", 0))
        # If there is no digest yet, treat as a new session (step 2 handles it).
        dp = DIGESTS / f"{sid}.md"
        if not dp.exists():
            kept.append(row)
            continue
        if n <= covered:
            kept.append(row)
            continue
        # New messages: summarize the delta.
        new_msgs = msgs[covered:][-30:]  # cap the delta
        new_text = "\n".join(msg_text(m) for m in new_msgs)
        old = dp.read_text()
        fmt = DIGEST_FORMAT.format(
            name=row.get("name", sid), sid=sid,
            model=row.get("model", ""), msgs=n, today=today,
            full_path=f"/root/pi-bridge/zs-sessions/{full}.json")
        prompt = UPDATE_PROMPT.format(
            old_digest=old, new_text=new_text, msgs=n, today=today, format=fmt)
        out = call_model(prompt, keys)
        if not out:
            log(f"stale {sid}: model call failed, will retry next run")
            kept.append(row)
            continue
        dp.write_text(out.strip() + "\n")
        row["msgs"] = n
        row["updated"] = today
        cursor[sid] = n
        changed = True
        log(f"updated {sid} ({row.get('name','?')}): {covered} -> {n} msgs")

    # --- 2. new session files not in the index ---
    known = {r["id"] for r in kept}
    for sid, (full, path) in files.items():
        if sid in known or sid in SKIP_AUTO:
            continue
        try:
            data = json.load(open(path))
            msgs = data.get("messages", [])
        except (OSError, ValueError):
            continue
        n = len(msgs)
        # Name from the first user message.
        name = sid
        for m in msgs:
            if m.get("role") == "user":
                t = msg_text(m)
                name = re.sub(r"^\[user\]\s*", "", t)[:48].strip() or sid
                break
        model = data.get("model", "") or ""
        new_text = "\n".join(msg_text(m) for m in msgs[:8])
        fmt = DIGEST_FORMAT.format(
            name=name, sid=sid, model=model, msgs=n, today=today,
            full_path=f"/root/pi-bridge/zs-sessions/{full}.json")
        prompt = NEW_PROMPT.format(new_text=new_text, format=fmt)
        out = call_model(prompt, keys)
        dp = DIGESTS / f"{sid}.md"
        if out:
            dp.write_text(out.strip() + "\n")
        else:
            # Model down: write a minimal digest so the session is still findable.
            dp.write_text(fmt.format(name=name, sid=sid, model=model,
                                     msgs=n, today=today,
                                     full_path=f"/root/pi-bridge/zs-sessions/{full}.json")
                          .replace("<one line>", "new session; digest pending (model call failed)")
                          .replace("<key decisions, comma-separated>", "none yet")
                          .replace("<key facts, comma-separated>", "see transcript")
                          .replace("<what got done / verified>", "see transcript")
                          .replace("<open items, or none>", "digest pending") + "\n")
        kept.append({"id": sid, "name": name, "model": model, "msgs": n,
                     "updated": today, "topics": [],
                     "sum": (name[:80] + " — digest auto-created.")})
        cursor[sid] = n
        changed = True
        log(f"added {sid} ({name}): new session, digest created")

    if changed:
        write_index(kept)
        save_cursor(cursor)
        log("index + cursor saved")
    else:
        log("no changes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
