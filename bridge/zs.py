#!/usr/bin/env python3
"""zs.py — tiny client for the live Zerostack gateway (zs-gateway.py, 127.0.0.1:8766).

Why: the gateway owns the transcript store and replays a digest into a fresh
zerostack process when one dies, so talking to the gateway (not to a raw
`zerostack` process) is what makes sessions recoverable.

Auth: the gateway token is read from /root/pi-bridge/gateway.env at runtime.
It is never printed, never written anywhere else, never passed on a command
line (it would show up in `ps`).

Usage:
  zs.py list
  zs.py new <name> [--cwd DIR] [--model M] [--brief TEXT]
  zs.py ask <session-id|name> "message" [--wait SECS] [--cwd DIR]
  zs.py wait <session-id|name> [--secs N]
  zs.py show <session-id|name> [--tail N]
  zs.py kill <session-id|name>        # kill the zerostack child, keep the transcript
  zs.py status <session-id|name>
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import time
import urllib.error
import urllib.request

BASE = "http://127.0.0.1:8766"
ENVF = "/root/pi-bridge/gateway.env"
SID_RE = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"

TOK: str | None = None


def token() -> str:
    try:
        with open(ENVF) as fh:
            for line in fh:
                if line.startswith("PI_GATEWAY_TOKEN="):
                    return line.strip().split("=", 1)[1]
    except OSError as exc:
        sys.exit(f"cannot read gateway env {ENVF}: {exc.strerror or exc}")
    sys.exit(f"no gateway token in {ENVF}")


def call(path: str, method: str = "GET", body: dict | None = None, timeout: int = 120) -> dict:
    global TOK
    if TOK is None:
        TOK = token()
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(  # noqa: S310 - fixed local http base, not user input
        BASE + path,
        data=data,
        method=method,
        headers={"Authorization": f"Bearer {TOK}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
            raw = resp.read().decode()
    except urllib.error.HTTPError as exc:
        return {"_http_error": exc.code, "body": exc.read().decode()[:800]}
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        return {"_raw": raw[:800]}


def resolve(ref: str) -> str:
    """Session id or name -> id."""
    if re.fullmatch(SID_RE, ref or ""):
        return ref
    for sess in call("/sessions"):
        if sess.get("name") == ref:
            return sess["id"]
    sys.exit(f"no session named {ref!r}")


def status(sid: str) -> dict:
    return call(f"/sessions/{sid}/status")


def wait(sid: str, secs: int = 600) -> dict:
    """Block until the session stops running; return the last status payload."""
    t0 = time.time()
    while time.time() - t0 < secs:
        st = status(sid)
        if not st.get("running"):
            return st
        time.sleep(2)
    return {"running": True, "timeout": True}


def ask(ref: str | None, msg: str, wait_secs: int = 900, cwd: str | None = None,
        model: str | None = None) -> dict:
    sid = resolve(ref) if ref else None
    body: dict = {"message": msg}
    if sid:
        res = call(f"/sessions/{sid}/prompt", "POST", body)
    else:
        body["cwd"] = cwd or "/root/work"
        if model:
            body["model"] = model
        res = call("/prompt", "POST", body)
    if res.get("_http_error"):
        return res
    sid = res.get("session_id", sid)
    st: dict = {}
    if wait_secs and sid:
        st = wait(sid, wait_secs)
    return {"session_id": sid, "result": res,
            "completed": not st.get("running", True) if st else False,
            "reply": st.get("last_assistant_text") if st else None,
            "timed_out": bool(st.get("timeout")) if st else False}


def render(content: object) -> str:
    """Transcript content is either a string or a list of typed blocks."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts: list[str] = []
    for block in content:
        if not isinstance(block, dict):
            continue
        kind = block.get("type")
        if kind == "text":
            parts.append(block.get("text") or "")
        elif kind == "thinking":
            parts.append("[thinking] " + (block.get("thinking") or block.get("text") or ""))
        elif kind == "toolCall":
            parts.append(f"[tool {block.get('name')}] {json.dumps(block.get('arguments'))[:200]}")
        else:
            parts.append(f"[{kind}]")
    return "\n".join(p for p in parts if p)


def main() -> None:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("list")

    p = sub.add_parser("new")
    p.add_argument("name")
    p.add_argument("--cwd", default="/root/work")
    p.add_argument("--model", default="")
    p.add_argument("--brief", default="")
    p.add_argument("--wait", type=int, default=600)

    p = sub.add_parser("ask")
    p.add_argument("ref")
    p.add_argument("message")
    p.add_argument("--wait", type=int, default=900)
    p.add_argument("--cwd", default=None)
    p.add_argument("--model", default=None)

    p = sub.add_parser("wait")
    p.add_argument("ref")
    p.add_argument("--secs", type=int, default=600)

    p = sub.add_parser("show")
    p.add_argument("ref")
    p.add_argument("--tail", type=int, default=6)

    p = sub.add_parser("kill")
    p.add_argument("ref")

    p = sub.add_parser("status")
    p.add_argument("ref")

    a = ap.parse_args()

    if a.cmd == "list":
        for sess in call("/sessions"):
            print(f"{sess['id']}  {sess.get('name') or '-':<18} msgs={sess.get('messages')!s:<4}"
                  f" running={sess.get('running')} live={sess.get('live')} cwd={sess.get('cwd')}")
    elif a.cmd == "new":
        # /prompt with no session_id creates a named session and sends the brief.
        body = {
            "message": a.brief or "Session opened. Reply with one line: ready.",
            "name": a.name,
            "cwd": a.cwd,
        }
        if a.model:
            body["model"] = a.model
        res = call("/prompt", "POST", body)
        sid = res.get("session_id")
        print(json.dumps({"id": sid, "name": a.name, "cwd": a.cwd, "response": res}, indent=2))
        if a.wait and sid:
            print("status:", json.dumps(wait(sid, a.wait))[:400])
    elif a.cmd == "ask":
        out = ask(a.ref, a.message, a.wait, a.cwd, a.model)
        print(json.dumps(out, indent=2)[:200000])
        if out.get("_http_error"):
            sys.exit(1)
        if isinstance(out.get("reply"), str) and out["reply"].startswith("[error:"):
            sys.exit(1)
        if out.get("timed_out"):
            sys.exit(2)
    elif a.cmd == "wait":
        print(json.dumps(wait(resolve(a.ref), a.secs), indent=2)[:2000])
    elif a.cmd == "show":
        sid = resolve(a.ref)
        data = call(f"/sessions/{sid}")
        msgs = (data.get("messages", {}).get("data", {}) or {}).get("messages", [])
        for msg in msgs[-a.tail:]:
            content = render(msg.get("content"))
            print(f"[{msg.get('role')}] {content[:700].replace(chr(10), chr(10) + '    ')}")
    elif a.cmd == "kill":
        sid = resolve(a.ref)
        print(json.dumps(call(f"/sessions/{sid}/stop", "POST", {}), indent=2)[:800])
    elif a.cmd == "status":
        print(json.dumps(status(resolve(a.ref)), indent=2)[:2000])


if __name__ == "__main__":
    main()
