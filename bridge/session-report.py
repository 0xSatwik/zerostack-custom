#!/usr/bin/env python3
"""session-report.py — Markdown report of gateway sessions in /root/pi-bridge/zs-sessions/.

Reads every <uuid>.json session file and prints a Markdown table with:
short id, name, model, message count, last activity (human time),
and whether the session has an assistant answer.

Usage:
  python3 session-report.py [--dir DIR] [--model SUBSTR]
"""

import argparse
import json
import sys
from datetime import datetime
from pathlib import Path

DEFAULT_DIR = "/root/pi-bridge/zs-sessions"


def cell(text):
    """Make a string safe for a Markdown table cell."""
    return str(text).replace("|", "\\|").replace("\n", " ").strip() or "—"


def assistant_text(msg):
    """Concatenated text blocks of an assistant message (ignoring thinking)."""
    parts = []
    for block in msg.get("content", []):
        # Text chunks may omit "type"; any block with non-empty "text" counts.
        if isinstance(block, dict) and block.get("text"):
            parts.append(block["text"])
    return "".join(parts)


def effective_model(sess):
    """Top-level model, falling back to the last assistant message's model."""
    model = (sess.get("model") or "").strip()
    if model:
        return model
    for msg in reversed(sess.get("messages", [])):
        if msg.get("role") == "assistant":
            provider = (msg.get("provider") or "").strip()
            model = (msg.get("model") or "").strip()
            if provider and model:
                return f"{provider}/{model}"
            if model:
                return model
    return ""


def has_answer(sess):
    """True if any assistant message carries non-empty text (not just thinking)."""
    for msg in sess.get("messages", []):
        if msg.get("role") == "assistant" and assistant_text(msg).strip():
            return True
    return False


def load_sessions(directory):
    sessions = []
    for path in sorted(Path(directory).glob("*.json")):
        try:
            with open(path, "r", encoding="utf-8") as fh:
                sess = json.load(fh)
        except (OSError, json.JSONDecodeError) as exc:
            print(f"warning: skipping {path.name}: {exc}", file=sys.stderr)
            continue
        if isinstance(sess, dict):
            sessions.append(sess)
    return sessions


def main():
    ap = argparse.ArgumentParser(description="Markdown report of gateway sessions.")
    ap.add_argument("--dir", default=DEFAULT_DIR, help=f"sessions directory (default: {DEFAULT_DIR})")
    ap.add_argument("--model", default=None, metavar="SUBSTR",
                    help="only show sessions whose model contains this substring (case-insensitive)")
    args = ap.parse_args()

    sessions = load_sessions(args.dir)

    if args.model:
        needle = args.model.lower()
        sessions = [s for s in sessions if needle in effective_model(s).lower()]

    if not sessions:
        print("_No sessions found._")
        return

    rows = []
    for s in sessions:
        short_id = (s.get("id") or "")[:8]
        name = s.get("name") or "—"
        model = effective_model(s) or "—"
        n_msgs = len(s.get("messages", []))
        mtime = s.get("mtime")
        try:
            last = datetime.fromtimestamp(mtime).strftime("%Y-%m-%d %H:%M")
        except (TypeError, ValueError, OSError):
            last = "—"
        answered = "yes" if has_answer(s) else "no"
        rows.append([short_id, name, model, n_msgs, last, answered])

    # Newest first by last activity.
    rows.sort(key=lambda r: r[4], reverse=True)

    header = ["id", "name", "model", "msgs", "last activity", "answered"]
    print("| " + " | ".join(header) + " |")
    print("|" + "|".join("---" for _ in header) + "|")
    for r in rows:
        r = [cell(r[0]), cell(r[1]), cell(r[2]), str(r[3]), cell(r[4]), cell(r[5])]
        print("| " + " | ".join(r) + " |")


if __name__ == "__main__":
    main()
