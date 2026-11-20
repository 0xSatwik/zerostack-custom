#!/usr/bin/env python3
"""Recall — find any session/chat or lesson in milliseconds, for few tokens.

Usage:
  recall.py find <keyword>        # grep the index, print matching lines
  recall.py digest <id|name>      # print that session's digest (the recall unit)
  recall.py lesson <keyword>      # grep the lessons index, print matches
  recall.py lessons               # list all lessons (one line each)
  recall.py sessions              # list all sessions (one line each)

Rules (see README.md):
- Recall reads the index + a digest. Never the full transcript.
- The full transcript is only read when the digest is not enough.
- A digest is ~200-400 tokens; a transcript is 10k+. Use the digest.
"""
import json
import os
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
INDEX = os.path.join(BASE, "index.jsonl")
DIGESTS = os.path.join(BASE, "digests")
LESSONS_IDX = os.path.join(BASE, "lessons-index.md")
LESSONS = os.path.join(BASE, "lessons")


def load_index():
    out = []
    try:
        with open(INDEX) as f:
            for line in f:
                line = line.strip()
                if line:
                    out.append(json.loads(line))
    except OSError:
        pass
    return out


def cmd_find(kw):
    kw = kw.lower()
    hits = [s for s in load_index()
            if kw in s.get("name", "").lower()
            or kw in s.get("sum", "").lower()
            or kw in " ".join(s.get("topics", [])).lower()
            or kw in s.get("id", "")]
    if not hits:
        print("no session matches %r" % kw)
        return
    for s in hits:
        print("%s | %s | %s | %d msgs | %s"
              % (s["id"], s["name"], s.get("model", "").split("/")[-1],
                 s.get("msgs", 0), s.get("sum", "")))


def cmd_digest(ref):
    ref = ref.lower()
    match = None
    for s in load_index():
        if s["id"].lower() == ref or s["name"].lower() == ref:
            match = s
            break
    if not match:
        # fall back: first partial name match
        for s in load_index():
            if ref in s["name"].lower():
                match = s
                break
    if not match:
        print("no session %r — try: recall.py find <keyword>" % ref)
        return
    path = os.path.join(DIGESTS, match["id"] + ".md")
    try:
        with open(path) as f:
            sys.stdout.write(f.read())
    except OSError:
        print("no digest for %s yet (full: %s)"
              % (match["name"], match.get("full", "?")))


def cmd_lesson(kw):
    kw = kw.lower()
    try:
        with open(LESSONS_IDX) as f:
            lines = [l.rstrip("\n") for l in f if l.strip()]
    except OSError:
        print("no lessons index yet")
        return
    hits = [l for l in lines if kw in l.lower()]
    if not hits:
        print("no lesson matches %r" % kw)
        return
    for l in hits:
        print(l)


def cmd_lessons():
    try:
        with open(LESSONS_IDX) as f:
            sys.stdout.write(f.read())
    except OSError:
        print("no lessons index yet")


def cmd_sessions():
    for s in load_index():
        print("%s | %s | %d msgs | %s | %s"
              % (s["id"], s["name"], s.get("msgs", 0),
                 s.get("updated", ""), s.get("sum", "")))


def main(argv):
    if len(argv) < 2:
        print(__doc__.strip().split("\n")[0])
        print("usage: recall.py {find|digest|lesson|lessons|sessions} [arg]")
        return 2
    cmd = argv[1]
    if cmd == "find" and len(argv) > 2:
        cmd_find(" ".join(argv[2:]))
    elif cmd == "digest" and len(argv) > 2:
        cmd_digest(argv[2])
    elif cmd == "lesson" and len(argv) > 2:
        cmd_lesson(" ".join(argv[2:]))
    elif cmd == "lessons":
        cmd_lessons()
    elif cmd == "sessions":
        cmd_sessions()
    else:
        print("usage: recall.py {find|digest|lesson|lessons|sessions} [arg]")
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
