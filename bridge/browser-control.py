#!/usr/bin/env python3
"""Browser takeover control page + Stop/Start for the Instabox browser box.
Serves on 0.0.0.0:6081. Password-protected: the login accepts the gateway
token OR the opening password (the same two that unlock /pi and /admin).
A session cookie keeps the user logged in. The VNC iframe only renders
after login.
"""
import hashlib
import hmac
import json
import os
import secrets
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

VNC_URL = os.environ.get("VNC_URL", "/browser-vnc/vnc.html?autoconnect=true&resize=remote")
PORT = 6081
# The gateway token, copied here from the Altare box (chmod 600).
TOKEN_FILE = "/data/workspace/browser-auth.txt"
# The opening password's SHA-256, copied here from the Altare box
# (gateway.env PI_ADMIN_PASS_SHA256, chmod 600). The password itself is
# never stored here — only its hash, so the login can verify it.
PASS_HASH_FILE = "/data/workspace/browser-pass-hash.txt"

SESSIONS = set()


def _token():
    try:
        with open(TOKEN_FILE) as f:
            return f.read().strip()
    except OSError:
        return ""


def _pass_hash():
    try:
        with open(PASS_HASH_FILE) as f:
            return f.read().strip()
    except OSError:
        return ""


def _check_password(pw):
    # Accepts the gateway token directly, OR the opening password
    # (verified by comparing its SHA-256 to the stored hash).
    tok = _token()
    if tok and hmac.compare_digest(pw, tok):
        return True
    ph = _pass_hash()
    if ph:
        pw_hash = hashlib.sha256(pw.encode()).hexdigest()
        if hmac.compare_digest(pw_hash, ph):
            return True
    return False


def _authed(handler):
    # Cookie session, or an Authorization: Bearer <token> header (for API).
    cookie = handler.headers.get("Cookie", "")
    for part in cookie.split(";"):
        part = part.strip()
        if part.startswith("bsess=") and part[6:] in SESSIONS:
            return True
    auth = handler.headers.get("Authorization", "")
    if auth.startswith("Bearer "):
        return _check_password(auth[7:].strip())
    return False


LOGIN_PAGE = """<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Browser — login</title>
<style>
body{margin:0;font-family:system-ui;background:#0b0d10;color:#e7eaee;display:flex;align-items:center;justify-content:center;min-height:100vh}
.card{background:#171a1f;border:1px solid #262b33;border-radius:12px;padding:24px;width:min(360px,90vw)}
h1{font-size:18px;margin:0 0 6px}.mut{color:#9aa3ad;font-size:13px;margin:0 0 16px}
input{width:100%;box-sizing:border-box;background:#0b0d10;color:#e7eaee;border:1px solid #262b33;border-radius:8px;padding:10px;font-size:15px}
button{width:100%;margin-top:12px;background:#2563eb;color:#fff;border:0;border-radius:8px;padding:10px;font-size:15px;cursor:pointer}
.err{color:#f87171;font-size:13px;min-height:18px;margin-top:8px}
</style></head><body><div class="card">
<h1>Browser login</h1>
<p class="mut">Enter the same gateway token (or opening password) you use for Chat and Admin.</p>
<form method="post" action="/browser/login"><input type="password" name="password" placeholder="Gateway token" autofocus>
<button type="submit">Unlock browser</button></form>
<div class="err">__ERR__</div>
</div></body></html>"""

PAGE = """<!DOCTYPE html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Browser - muse-browser</title>
<style>
body{margin:0;font-family:system-ui;background:#111;color:#eee}
.bar{display:flex;gap:8px;align-items:center;padding:8px 12px;background:#1c1c1c}
.bar b{font-size:15px}
.bar span{color:#999;font-size:13px}
button{padding:8px 16px;border:0;border-radius:8px;font-size:15px;cursor:pointer}
#stop{background:#c0392b;color:#fff}
#start{background:#27ae60;color:#fff}
#status{margin-left:auto;font-size:13px}
iframe{border:0;width:100vw;height:calc(100vh - 52px);display:block;background:#000}
</style></head><body>
<div class="bar"><b>Browser</b><span>muse-browser &middot; Camoufox &middot; click / type to take over</span>
<button id="stop" onclick="act('stop')">Stop</button>
<button id="start" onclick="act('start')">Start</button>
<span id="status"></span></div>
<iframe id="v" src="__VNC_URL__" allow="clipboard-read; clipboard-write"></iframe>
<script>
async function act(a){
  document.getElementById('status').textContent = a + '...';
  try{
    const r = await fetch('/browser/'+a,{method:'POST'});
    const j = await r.json();
    document.getElementById('status').textContent = j.ok ? (a+' ok') : ('error: '+(j.error||''));
    if(a==='stop'){ document.getElementById('v').src='about:blank'; }
    if(a==='start'){ setTimeout(()=>{ document.getElementById('v').src='__VNC_URL__'; }, 8000); }
  }catch(e){ document.getElementById('status').textContent = 'error: '+e; }
}
</script></body></html>
""".replace("__VNC_URL__", VNC_URL)


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="application/json", extra=None):
        b = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        if extra:
            for k, v in extra.items():
                self.send_header(k, v)
        self.end_headers()
        self.wfile.write(b)

    def _body(self):
        try:
            n = int(self.headers.get("Content-Length") or 0)
        except ValueError:
            n = 0
        raw = self.rfile.read(max(0, n)) if n > 0 else b""
        try:
            return json.loads(raw.decode() or "{}")
        except ValueError:
            # Form-encoded (the login form).
            return {k: v[0] for k, v in parse_qs(raw.decode()).items()}

    def do_GET(self):
        path = urlparse(self.path).path
        # The tunnel forwards the FULL path, so via /browser the paths
        # arrive as /browser, /browser/status, ... — strip that prefix
        # so the same handlers work both via the tunnel and direct.
        if path.startswith("/browser"):
            path = path[len("/browser"):] or "/"
        if path in ("/", "/index.html", "/browser", "/browser/"):
            if not _authed(self):
                return self._send(200, LOGIN_PAGE.replace("__ERR__", ""), "text/html")
            return self._send(200, PAGE, "text/html")
        if path == "/status":
            if not _authed(self):
                return self._send(401, json.dumps({"ok": False, "error": "unauthorized"}))
            out = subprocess.run(
                ["bash", "-c", "ss -tln 2>/dev/null | grep -E '9377' | wc -l"],
                capture_output=True, text=True)
            return self._send(200, json.dumps(
                {"ok": True, "stack_ports_up": int(out.stdout.strip() or 0)}))
        return self._send(404, json.dumps({"ok": False, "error": "not found"}))

    def do_POST(self):
        p = urlparse(self.path).path
        # Same /browser prefix strip as do_GET (tunnel forwards full path).
        if p.startswith("/browser"):
            p = p[len("/browser"):] or "/"
        if p == "/login":
            body = self._body()
            pw = (body.get("password") or "").strip()
            if _check_password(pw):
                tok = secrets.token_hex(16)
                SESSIONS.add(tok)
                self.send_response(302)
                self.send_header("Set-Cookie",
                                 "bsess=%s; Path=/; HttpOnly; SameSite=Lax" % tok)
                self.send_header("Location", "/browser")
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            # Wrong password: show the login again with an error.
            return self._send(200, LOGIN_PAGE.replace("__ERR__", "Wrong token — try again."),
                              "text/html")
        if not _authed(self):
            return self._send(401, json.dumps({"ok": False, "error": "unauthorized"}))
        if p == "/stop":
            subprocess.run(["bash", "/data/workspace/stop-browser.sh"],
                           capture_output=True)
            return self._send(200, json.dumps({"ok": True}))
        if p == "/start":
            subprocess.run(["bash", "-c",
                            "nohup /data/workspace/browser-stack.sh > /data/workspace/browser-stack.log 2>&1 &"],
                           capture_output=True)
            return self._send(200, json.dumps({"ok": True}))
        return self._send(404, json.dumps({"ok": False, "error": "not found"}))


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", PORT), H).serve_forever()
