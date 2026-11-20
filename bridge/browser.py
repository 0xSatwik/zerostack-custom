#!/usr/bin/env python3
"""browser.py — CLI for the headed Camoufox browser on the Instabox box.

Why a CLI and not MCP: Zerostack's ACP mode (the mode the zs-gateway uses)
sets `mcp_manager: None` (src/extras/acp/mod.rs) — MCP servers from the config
are NOT loaded in ACP sessions, so the browser MCP tools never appear there.
The MCP config entry `[mcp_servers.browser]` works in the Zerostack TUI only.
This CLI talks to the Playwright MCP server over HTTP (via the localhost
relay on the Altare box) and is callable from Zerostack via bash, which IS
available in ACP mode. The browser skill (`/prompt browser`) teaches
Zerostack to use this CLI.

The MCP session id is persisted in SESSION_FILE so consecutive CLI calls share
one browser/page. If the session expired, the CLI re-initialises and retries.

Usage:
  browser.py navigate <url>            open a URL, print title + url
  browser.py snapshot                  print the accessibility snapshot
  browser.py click <ref>               click element by snapshot ref
  browser.py type <ref> <text> [--submit]
                                       type into a field by ref
  browser.py screenshot [path]         save a screenshot (default /tmp/browser.png)
  browser.py eval <js>                 run JavaScript, print result
  browser.py wait <text>               wait for text to appear
  browser.py back                      go back
  browser.py close                     close the page/browser
  browser.py status                    show whether the MCP server answers
"""
import json
import os
import sys
import urllib.request
import urllib.error

MCP_URL = os.environ.get("BROWSER_MCP_URL", "http://127.0.0.1:8931/mcp")
SESSION_FILE = os.environ.get("BROWSER_SESSION_FILE", "/tmp/browser-mcp-session.json")


def _req(method, params, session_id=None):
    body = json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }).encode()
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if session_id:
        headers["mcp-session-id"] = session_id
    req = urllib.request.Request(MCP_URL, data=body, headers=headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            raw = resp.read().decode()
            sid = resp.headers.get("mcp-session-id") or session_id
    except urllib.error.HTTPError as e:
        raise RuntimeError("MCP HTTP %s: %s" % (e.code, e.read().decode()[:300]))
    # Streamable HTTP returns SSE lines: "event: message" + "data: {...}"
    payload = None
    for line in raw.splitlines():
        if line.startswith("data:"):
            payload = json.loads(line[5:].strip())
            break
    if payload is None:
        raise RuntimeError("no SSE data in MCP response: %r" % raw[:300])
    if "error" in payload:
        raise RuntimeError("MCP error: %s" % json.dumps(payload["error"])[:300])
    return payload.get("result"), sid


def _load_session():
    try:
        with open(SESSION_FILE) as f:
            return json.load(f).get("session_id")
    except (OSError, ValueError):
        return None


def _save_session(sid):
    try:
        with open(SESSION_FILE, "w") as f:
            json.dump({"session_id": sid}, f)
    except OSError:
        pass


def _ensure_session():
    sid = _load_session()
    if sid:
        return sid
    result, sid = _req("initialize", {
        "protocolVersion": "2025-03-26", "capabilities": {},
        "clientInfo": {"name": "browser-cli", "version": "1.0"},
    })
    _save_session(sid)
    # No notifications/initialized: the Playwright MCP server accepts
    # tools/call right after initialize, and sending the notification as a
    # request (with an id) makes it answer -32601 Method not found.
    return sid


def _call_tool(name, arguments):
    sid = _ensure_session()
    try:
        result, _ = _req("tools/call", {
            "name": name, "arguments": arguments,
        }, sid)
    except RuntimeError as e:
        # Session may have expired; re-initialise once and retry.
        if "404" in str(e) or "session" in str(e).lower():
            try:
                os.remove(SESSION_FILE)
            except OSError:
                pass
            sid = _ensure_session()
            result, _ = _req("tools/call", {
                "name": name, "arguments": arguments,
            }, sid)
        else:
            raise
    return result


def _tool_text(result):
    parts = []
    for c in (result or {}).get("content", []):
        if c.get("type") == "text":
            parts.append(c.get("text", ""))
    return "\n".join(parts)


def main(argv):
    if len(argv) < 2:
        print(__doc__.strip())
        return 2
    cmd = argv[1]
    try:
        if cmd == "navigate":
            if len(argv) < 3:
                print("usage: browser.py navigate <url>")
                return 2
            print(_tool_text(_call_tool("browser_navigate", {"url": argv[2]})))
        elif cmd == "snapshot":
            print(_tool_text(_call_tool("browser_snapshot", {})))
        elif cmd == "click":
            if len(argv) < 3:
                print("usage: browser.py click <ref>")
                return 2
            # browser_click requires `target` (the snapshot ref).
            print(_tool_text(_call_tool("browser_click", {"target": argv[2]})))
        elif cmd == "type":
            if len(argv) < 4:
                print("usage: browser.py type <ref> <text> [--submit]")
                return 2
            # browser_type requires `target` (ref) + `text`.
            args = {"target": argv[2], "text": argv[3]}
            if "--submit" in argv[4:]:
                args["submit"] = True
            print(_tool_text(_call_tool("browser_type", args)))
        elif cmd == "screenshot":
            path = argv[2] if len(argv) > 2 else "/tmp/browser.png"
            # browser_take_screenshot requires `type` + `scale`; `filename`
            # is where it saves (on the Instabox box, under the MCP cwd).
            print(_tool_text(_call_tool("browser_take_screenshot", {
                "filename": path, "type": "png", "scale": "css",
            })))
        elif cmd == "eval":
            if len(argv) < 3:
                print("usage: browser.py eval <js>")
                return 2
            print(_tool_text(_call_tool("browser_evaluate", {"function": argv[2]})))
        elif cmd == "wait":
            if len(argv) < 3:
                print("usage: browser.py wait <text>")
                return 2
            print(_tool_text(_call_tool("browser_wait_for", {"text": argv[2]})))
        elif cmd == "back":
            print(_tool_text(_call_tool("browser_navigate_back", {})))
        elif cmd == "close":
            print(_tool_text(_call_tool("browser_close", {})))
        elif cmd == "status":
            result, _ = _req("initialize", {
                "protocolVersion": "2025-03-26", "capabilities": {},
                "clientInfo": {"name": "browser-cli", "version": "1.0"},
            })
            print("mcp ok: %s %s" % (
                (result or {}).get("serverInfo", {}).get("name"),
                (result or {}).get("serverInfo", {}).get("version")))
        else:
            print("unknown command: %s" % cmd)
            print(__doc__.strip())
            return 2
    except RuntimeError as e:
        print("error: %s" % e, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
