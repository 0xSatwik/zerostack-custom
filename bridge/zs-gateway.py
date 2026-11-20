#!/usr/bin/env python3
"""zs-gateway.py — HTTP gateway in front of Zerostack's ACP server.

Serves the same HTTP surface as pi-gateway.py so the existing /pi front end
(pi.html) can talk to Zerostack with almost no changes. Stdlib only.

WHY THIS EXISTS (and why it is shaped this way)
-----------------------------------------------
Zerostack's ACP server (src/extras/acp/mod.rs, zerostack 1.8.6,
agent-client-protocol 5.3.0 / schema 1.18.0) registers exactly three request
handlers:

    initialize      -> agentInfo {name:"zerostack", version:"1.0.5"}
    session/new     -> {sessionId}
    session/prompt  -> {stopReason}   (responds only when the turn ENDS)

It emits `session/update` notifications while a turn runs:
    agent_message_chunk, agent_thought_chunk, tool_call, tool_call_update

Four consequences drive this gateway's design:

1. `session/cancel` DOES NOT EXIST. A live probe returns
   {"code":-32601,"message":"Method not found","data":"session/cancel"}.
   So /stop cannot be a protocol call: it terminates the zerostack process
   that owns the session. That is the only stop that exists.

2. There is no session/load, session/list, session/resume, session/set_model
   or session/set_mode. Sessions live only in an in-process HashMap, so they
   die with the process. The gateway therefore OWNS the transcript store
   (one JSON file per session) and replays a compact text digest of the
   earlier conversation into the next prompt when a process is respawned
   (after stop, model switch, crash or gateway restart). /compact controls
   how much gets replayed.

3. `session/prompt` answers only at turn end, so /prompt returns immediately
   with accepted=true and streaming is driven by the reader thread through
   /sessions/{id}/events.

4. MCP is disabled in ACP mode (mod.rs passes `mcp_manager: None`), and the
   `mcpServers` field of session/new is ignored. Vault MCP works in the TUI,
   not through ACP. The gateway does not pretend otherwise.

Transport note: the ACP TCP listener calls accept() exactly once, so each
zerostack process serves exactly one client. Hence one process per session,
which mirrors pi-gateway.py's one `pi --mode rpc` process per session.

Run (tmux session zs-gateway):
  PATH=/root/.local/bin:$PATH python3 /root/pi-bridge/zs-gateway.py
  >> /root/pi-bridge/zs-gateway.log 2>&1
"""
import hashlib, hmac, json, os, queue, re, socket, subprocess, sys, threading, time, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs
import tomllib

BASE = "/root/pi-bridge"
WORK = "/root/work"
SESS_DIR = os.path.join(BASE, "zs-sessions")
ZS_BIN = os.environ.get("ZS_BIN", "/root/.local/bin/zerostack")
ZS_CONFIG = os.environ.get("ZS_CONFIG", "/root/.config/zerostack/config.toml")
PORT = int(os.environ.get("ZS_GATEWAY_PORT", "8766"))
HOST = os.environ.get("ZS_GATEWAY_HOST", "127.0.0.1")
# Where each backend's page lives, for the Pi | Zerostack switcher. The live
# /pi URL keeps pointing at the Pi gateway until the tunnel is repointed.
PI_PUBLIC_URL = os.environ.get("ZS_PI_URL", "http://127.0.0.1:8765/pi")
ZS_PUBLIC_URL = os.environ.get("ZS_ZS_URL", "")   # "" = same origin (/pi)
ACP_PORT_BASE = 7300
ACP_PORT_SPAN = 100
TURN_TIMEOUT = 3600.0
PREAMBLE_MAX = 12000
# Each session's zerostack process costs ~43 MB even when idle, so a chat that
# has not been touched for a while gives its process back. The transcript is on
# disk and the next prompt respawns + replays, so this is invisible to the user.
IDLE_REAP_SECS = 900.0
IDLE_SWEEP_SECS = 60.0

os.makedirs(SESS_DIR, exist_ok=True)


def log(msg):
    line = "%s %s\n" % (time.strftime("%Y-%m-%d %H:%M:%S"), msg)
    try:
        with open(os.path.join(BASE, "zs-gateway.log"), "a") as f:
            f.write(line)
    except Exception:
        pass
    # Only echo to stderr when it is a terminal. Under tmux the log file is the
    # redirect target for stderr too, so writing both would duplicate every line.
    try:
        if sys.stderr.isatty():
            sys.stderr.write(line)
            sys.stderr.flush()
    except Exception:
        pass


# ---- env / auth -----------------------------------------------------------
def load_env_file(path):
    """Provider keys + vault token live here. Values are never logged."""
    try:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                k, v = line.split("=", 1)
                k = k.replace("export ", "").strip()
                v = v.strip().strip('"').strip("'")
                if k and v and not k.startswith("-"):
                    os.environ.setdefault(k, v)
    except FileNotFoundError:
        pass


load_env_file(os.path.join(BASE, "provider-keys.env"))
load_env_file("/root/.profile")

TOKEN = None
ADMIN_PASS_SHA256 = None
_gw_env = os.path.join(BASE, "gateway.env")
if os.path.exists(_gw_env):
    for line in open(_gw_env):
        if line.startswith("PI_GATEWAY_TOKEN="):
            TOKEN = line.strip().split("=", 1)[1]
        elif line.startswith("PI_ADMIN_PASS_SHA256="):
            ADMIN_PASS_SHA256 = line.strip().split("=", 1)[1]
TOKEN = TOKEN or os.environ.get("PI_GATEWAY_TOKEN")
ADMIN_PASS_SHA256 = ADMIN_PASS_SHA256 or os.environ.get("PI_ADMIN_PASS_SHA256")
if not TOKEN:
    log("no PI_GATEWAY_TOKEN in gateway.env or env — exiting")
    sys.exit(1)

LOGIN_FAILS = {}


# ---- zerostack config (READ ONLY — never rewritten) ----------------------
def zs_config():
    try:
        with open(ZS_CONFIG, "rb") as f:
            return tomllib.load(f)
    except Exception as e:
        log("config read failed: %s" % e)
        return {}


def split_model(model):
    """'provider/model' -> (provider, model). Provider names carry no slash."""
    if not model:
        return None, None
    if "/" in model:
        p, m = model.split("/", 1)
        return p, m
    return None, model


def models_payload():
    cfg = zs_config()
    provs = {}
    ctx = cfg.get("context_window")
    out = cfg.get("max_tokens")

    def entry(p):
        return provs.setdefault(p, {"models": []})

    def add(p, mid):
        if not p or not mid:
            return
        e = entry(p)
        if not any(m.get("id") == mid for m in e["models"]):
            m = {"id": mid}
            if ctx:
                m["contextWindow"] = ctx
            if out:
                m["maxTokens"] = out
            e["models"].append(m)

    for name, cp in (cfg.get("custom_providers") or {}).items():
        e = entry(name)
        if cp.get("base_url"):
            e["baseUrl"] = cp["base_url"]
        if cp.get("api_key_env"):
            e["apiKey"] = "$" + cp["api_key_env"]   # env var NAME only, never a value
        add(name, cp.get("model"))

    for _q, qm in (cfg.get("quick_models") or {}).items():
        add(qm.get("provider"), qm.get("model"))

    add(cfg.get("provider"), cfg.get("model"))
    return {"providers": provs,
            "default": {"provider": cfg.get("provider"), "model": cfg.get("model")}}


def provider_key_status():
    """Env var names the config references + whether each is set. No values."""
    cfg = zs_config()
    names = []
    for _n, cp in (cfg.get("custom_providers") or {}).items():
        env = cp.get("api_key_env")
        if env and env not in names:
            names.append(env)
    out = []
    for n in names:
        v = os.environ.get(n)
        out.append({"env_name": n, "set": bool(v and v.strip())})
    return out


# ---- stats (same shape and basis as pi-gateway.py) ------------------------
CPU_MOUNT = "/sys/fs/cgroup"
DISK_QUOTA_GB = 10.0
_cpu_prev = {"usec": None, "ts": None}
_ws_cache = {"bytes": None, "ts": 0.0, "busy": False}
WS_TTL = 300.0


def _cgroup(path, default=None):
    try:
        with open(os.path.join(CPU_MOUNT, path)) as f:
            return f.read().strip()
    except Exception:
        return default


def _cpu_quota_cores():
    raw = _cgroup("cpu.max")
    if not raw or raw == "max":
        return None
    try:
        q, per = raw.split()
        return int(q) / int(per)
    except Exception:
        return None


def _cpu_usage_usec():
    try:
        with open(os.path.join(CPU_MOUNT, "cpu.stat")) as f:
            for line in f:
                if line.startswith("usage_usec"):
                    return int(line.split()[1])
    except Exception:
        pass
    return None


def _read_cpu():
    try:
        with open("/proc/stat") as f:
            parts = f.readline().split()
        vals = [int(x) for x in parts[1:]]
        idle = vals[3] + (vals[4] if len(vals) > 4 else 0)
        return sum(vals) - idle, sum(vals)
    except Exception:
        return None, None


def _workspace_bytes(force=False):
    now = time.time()
    due = _ws_cache["bytes"] is None or (now - _ws_cache["ts"] > WS_TTL)
    if (force or due) and not _ws_cache["busy"]:
        _ws_cache["busy"] = True

        def _work():
            try:
                o = subprocess.run(["du", "-sxB1", "/root"], capture_output=True,
                                   text=True, timeout=600).stdout.split()
                _ws_cache["bytes"] = int(o[0])
                _ws_cache["ts"] = time.time()
            except Exception:
                pass
            finally:
                _ws_cache["busy"] = False

        threading.Thread(target=_work, daemon=True).start()
    return _ws_cache["bytes"]


def _seed_stats():
    u = _cpu_usage_usec()
    if u is not None:
        _cpu_prev["usec"], _cpu_prev["ts"] = u, time.time()
    try:
        _workspace_bytes(force=True)
    except Exception:
        pass


def stats_snapshot():
    now = time.time()
    quota = _cpu_quota_cores()

    cpu_pct = None
    usec = _cpu_usage_usec()
    if usec is not None and _cpu_prev["usec"] is not None:
        d_usec = usec - _cpu_prev["usec"]
        d_wall = now - _cpu_prev["ts"]
        if d_wall > 0:
            allowed = d_wall * 1e6 * (quota or (os.cpu_count() or 1))
            cpu_pct = max(0.0, min(100.0, 100.0 * d_usec / allowed))
    if usec is not None:
        _cpu_prev["usec"], _cpu_prev["ts"] = usec, now
    else:
        busy, total = _read_cpu()
        if busy is not None and _cpu_prev.get("hbusy") is not None:
            d_total = total - _cpu_prev["hbusy"]
            d_busy = busy - _cpu_prev["hbusy"]
            if d_total > 0:
                cpu_pct = max(0.0, min(100.0, 100.0 * d_busy / d_total))
        if busy is not None:
            _cpu_prev["hbusy"], _cpu_prev["htotal"] = busy, total

    ram_used = ram_total = None
    cur, mx = _cgroup("memory.current"), _cgroup("memory.max")
    if cur and mx and mx != "max":
        try:
            ram_used, ram_total = int(cur), int(mx)
        except Exception:
            pass
    if not ram_total:
        try:
            info = {}
            with open("/proc/meminfo") as f:
                for line in f:
                    k, _, v = line.partition(":")
                    info[k] = int(v.split()[0]) * 1024
            ram_total = info.get("MemTotal")
            ram_used = ram_total - info.get("MemAvailable", 0) if ram_total else None
        except Exception:
            pass

    def pct(used, total):
        if not used or not total:
            return None
        return max(0.0, min(100.0, 100.0 * used / total))

    # Disk = the workspace footprint vs the 10 GiB plan quota, NOT the host
    # overlay (statvfs("/") reports the 845 GB host disk and would be nonsense
    # next to a "10 GiB" label). Host numbers are reported separately.
    disk = {"pct": None}
    ws = _workspace_bytes()
    quota_b = DISK_QUOTA_GB * (1024 ** 3)
    if ws:
        disk = {"pct": pct(ws, quota_b),
                "used_gb": round(ws / (1024 ** 3), 2),
                "total_gb": DISK_QUOTA_GB,
                "basis": "workspace(/root) vs plan quota"}
    try:
        st = os.statvfs("/")
        host_total = st.f_blocks * st.f_frsize
        host_used = host_total - st.f_bavail * st.f_frsize
        disk["host"] = {"pct": pct(host_used, host_total),
                        "used_gb": round(host_used / 1e9, 1),
                        "total_gb": round(host_total / 1e9, 1)}
    except Exception:
        pass

    def gb(n):
        return round(n / (1024 ** 3), 1) if n else None

    rp = pct(ram_used, ram_total)
    return {
        "cpu": {"pct": None if cpu_pct is None else round(cpu_pct, 1),
                "quota": None if not quota else round(quota * 100)},
        "ram": {"pct": None if rp is None else round(rp, 1),
                "used_gb": gb(ram_used), "total_gb": gb(ram_total)},
        "disk": disk,
        "spec": {"ram": (str(gb(ram_total)) + " GiB") if ram_total else "6 GiB",
                 "cpu": (str(round(quota * 100)) + "%") if quota else "175%",
                 "disk": "10 GiB"},
        "updated": int(now),
    }


# ---- sessions -------------------------------------------------------------
class Session:
    def __init__(self, sid, name="", cwd=WORK, model=""):
        self.id = sid
        self.name = name or sid
        self.cwd = cwd
        self.model = model
        self.created = time.time()
        self.mtime = time.time()
        self.messages = []
        self.events = []
        self.running = False
        self.last_assistant = ""
        self.replay_from = 0
        self.needs_preamble = False
        self.proc = None
        self.port = None
        self.sock = None
        self.rfile = None
        self.acp_sid = None
        self.pending = {}
        self.lock = threading.RLock()
        self.q = queue.Queue()
        self.closed = False
        self.deleted = False
        self.turn = None
        self.worker_started = False

    def meta(self):
        return {"id": self.id, "name": self.name, "cwd": self.cwd,
                "model": self.model, "created": self.created, "mtime": self.mtime,
                "replay_from": self.replay_from, "messages": self.messages,
                "last_assistant": self.last_assistant}


SESSIONS = {}
SESS_LOCK = threading.RLock()


def sess_path(sid):
    return os.path.join(SESS_DIR, sid + ".json")


def save_session(s):
    # A deleted session must stay deleted. DELETE sets `deleted` and the worker
    # may still be finishing a turn when it lands, so a plain `closed` check is
    # not enough: the tombstone is permanent for this object.
    if s.deleted:
        return
    with s.lock:
        data = s.meta()
        tmp = sess_path(s.id) + ".tmp"
        try:
            with open(tmp, "w") as f:
                json.dump(data, f)
            os.replace(tmp, sess_path(s.id))
            os.chmod(sess_path(s.id), 0o600)
        except Exception as e:
            log("save_session %s failed: %s" % (s.id, e))


def load_sessions():
    for fn in sorted(os.listdir(SESS_DIR)):
        if not fn.endswith(".json"):
            continue
        try:
            with open(os.path.join(SESS_DIR, fn)) as f:
                d = json.load(f)
            s = Session(d["id"], d.get("name") or "", d.get("cwd") or WORK,
                        d.get("model") or "")
            s.created = d.get("created") or time.time()
            s.mtime = d.get("mtime") or s.created
            s.messages = d.get("messages") or []
            s.replay_from = d.get("replay_from") or 0
            s.last_assistant = d.get("last_assistant") or ""
            # Replay the tail of the event log so /events still works for a
            # chat that was live before a gateway restart.
            try:
                with open(os.path.join(SESS_DIR, s.id + ".events.jsonl")) as ef:
                    s.events = [json.loads(l) for l in ef.readlines()[-2000:]]
            except Exception:
                s.events = []
            SESSIONS[s.id] = s
        except Exception as e:
            log("load session %s failed: %s" % (fn, e))


def log_event(s, rec):
    if s.deleted:
        return
    with s.lock:
        s.events.append(rec)
        if len(s.events) > 2000:
            s.events = s.events[-2000:]
    try:
        with open(os.path.join(SESS_DIR, s.id + ".events.jsonl"), "a") as f:
            f.write(json.dumps(rec) + "\n")
    except Exception:
        pass


def ensure_assistant(s):
    if s.turn is None:
        s.turn = {"role": "assistant", "content": [], "timestamp": time.time() * 1000,
                  "model": split_model(s.model)[1] or s.model,
                  "provider": split_model(s.model)[0] or ""}
        s.messages.append(s.turn)
    return s.turn


# ---- ACP plumbing ---------------------------------------------------------
_used_ports = set()
_port_lock = threading.Lock()


def _free_port():
    with _port_lock:
        for p in range(ACP_PORT_BASE, ACP_PORT_BASE + ACP_PORT_SPAN):
            if p in _used_ports:
                continue
            sk = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            try:
                sk.bind(("127.0.0.1", p))
                _used_ports.add(p)
                return p
            except OSError:
                continue
            finally:
                sk.close()
    raise RuntimeError("no free ACP port")


def _release_port(p):
    with _port_lock:
        _used_ports.discard(p)


def kill_proc(s, quiet=False):
    with s.lock:
        proc, sock, rf = s.proc, s.sock, s.rfile
        s.proc = s.sock = s.rfile = None
        s.acp_sid = None
        port = s.port
        s.port = None
    had = bool(proc or sock)
    # shutdown() FIRST, then close. Closing a socket does not wake another
    # thread already blocked in recv() on Linux, and closing the makefile
    # buffered reader would then block on that thread's lock — which is what
    # made DELETE hang. shutdown(SHUT_RDWR) makes the pending read return.
    try:
        if sock:
            sock.shutdown(socket.SHUT_RDWR)
    except Exception:
        pass
    try:
        if sock:
            sock.close()
    except Exception:
        pass
    try:
        if rf:
            rf.close()
    except Exception:
        pass
    if proc and proc.poll() is None:
        try:
            pid = proc.pid
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
                try:
                    proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
            if proc.poll() is None:
                log("session %s: pid %s survived terminate+kill" % (s.id, pid))
        except Exception as e:
            log("session %s: kill failed: %s" % (s.id, e))
    if port:
        _release_port(port)
    if not quiet and had:
        log("session %s: zerostack process stopped" % s.id)


def _reader(s):
    rf = s.rfile
    try:
        for line in rf:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except Exception:
                continue
            if "method" in msg and msg.get("id") is not None:
                # server->client request (e.g. session/request_permission).
                # ACP auto-approves asks server-side; answer defensively.
                try:
                    s.sock.sendall((json.dumps({"jsonrpc": "2.0", "id": msg["id"],
                                                "result": {"outcome": {"outcome": "selected",
                                                                       "optionId": "allow"}}}) + "\n").encode())
                except Exception:
                    pass
            elif msg.get("id") is not None:
                ev = s.pending.pop(msg["id"], None)
                if ev:
                    ev["msg"] = msg
                    ev["done"].set()
            elif msg.get("method") == "session/update":
                try:
                    handle_update(s, msg.get("params") or {})
                except Exception as e:
                    log("handle_update failed: %s" % e)
    except Exception:
        pass
    # connection ended -> turn (if any) is over
    for ev in list(s.pending.values()):
        ev["done"].set()
    with s.lock:
        s.running = False


def rpc(s, method, params, timeout=60):
    rid = "zs-" + uuid.uuid4().hex[:10]
    ev = {"done": threading.Event(), "msg": None}
    with s.lock:
        sock = s.sock
        if not sock:
            raise RuntimeError("no ACP connection")
        s.pending[rid] = ev
    # Send OUTSIDE the lock: sendall can block on a full socket buffer, and the
    # reader thread needs the lock in handle_update to drain that buffer.
    payload = json.dumps({"jsonrpc": "2.0", "id": rid, "method": method,
                          "params": params}) + "\n"
    try:
        sock.sendall(payload.encode())
    except Exception as e:
        s.pending.pop(rid, None)
        raise RuntimeError("ACP send failed: %s" % e)
    if not ev["done"].wait(timeout):
        s.pending.pop(rid, None)
        raise TimeoutError("%s timed out after %ss" % (method, timeout))
    msg = ev["msg"] or {}
    if "error" in msg:
        err = msg["error"]
        raise RuntimeError(str(err.get("message") if isinstance(err, dict) else err))
    return msg


def handle_update(s, params):
    u = params.get("update") or {}
    kind = u.get("sessionUpdate")
    with s.lock:
        if kind == "agent_message_chunk":
            txt = (u.get("content") or {}).get("text") or ""
            if not txt:
                return
            m = ensure_assistant(s)
            if m["content"] and m["content"][-1].get("type") == "text":
                m["content"][-1]["text"] += txt
            else:
                m["content"].append({"type": "text", "text": txt})
            s.last_assistant = (s.last_assistant + txt)[-4000:]
            log_event(s, {"type": "message_update",
                          "assistantMessageEvent": {"type": "text_delta", "delta": txt}})

        elif kind == "agent_thought_chunk":
            txt = (u.get("content") or {}).get("text") or ""
            if not txt:
                return
            m = ensure_assistant(s)
            if m["content"] and m["content"][-1].get("type") == "thinking":
                m["content"][-1]["thinking"] += txt
            else:
                m["content"].append({"type": "thinking", "thinking": txt})
            log_event(s, {"type": "message_update",
                          "assistantMessageEvent": {"type": "thinking_delta", "delta": txt}})

        elif kind == "tool_call":
            name = u.get("title") or "tool"
            m = ensure_assistant(s)
            m["content"].append({"type": "toolCall", "id": u.get("toolCallId"),
                                 "name": name, "arguments": u.get("rawInput") or {}})
            log_event(s, {"type": "message_update",
                          "assistantMessageEvent": {"type": "toolcall_start",
                                                    "toolName": name, "id": u.get("toolCallId")}})
            log_event(s, {"type": "tool_execution_start", "toolName": name})

        elif kind == "tool_call_update":
            text = ""
            for c in (u.get("content") or []):
                if not isinstance(c, dict):
                    continue
                inner = c.get("content") or {}
                if inner.get("type") == "text":
                    text += inner.get("text") or ""
                elif c.get("type") == "text":
                    text += c.get("text") or ""
            status = u.get("status")
            s.messages.append({"role": "toolResult", "toolCallId": u.get("toolCallId"),
                               "content": [{"type": "text", "text": text}],
                               "isError": status == "failed",
                               "timestamp": time.time() * 1000})
            log_event(s, {"type": "tool_execution_end", "toolName": u.get("title") or "tool"})
            log_event(s, {"type": "message_update",
                          "assistantMessageEvent": {"type": "toolcall_end"}})


def spawn_proc(s):
    if s.closed or s.deleted:
        raise RuntimeError("session closed")
    cfg = zs_config()
    provider, model = split_model(s.model)
    provider = provider or cfg.get("provider")
    model = model or cfg.get("model")
    port = _free_port()
    cmd = [ZS_BIN, "--acp", "--acp-host", "127.0.0.1", "--acp-port", str(port)]
    if provider:
        cmd += ["--provider", provider]
    if model:
        cmd += ["--model", model]
    logf = open(os.path.join(SESS_DIR, s.id + ".log"), "ab")
    proc = subprocess.Popen(cmd, cwd=s.cwd, stdin=subprocess.DEVNULL,
                            stdout=logf, stderr=logf, env=os.environ.copy())
    sock = None
    deadline = time.time() + 25
    while time.time() < deadline:
        if proc.poll() is not None:
            _release_port(port)
            raise RuntimeError("zerostack exited immediately (see %s.log)" % s.id)
        try:
            sock = socket.create_connection(("127.0.0.1", port), timeout=2)
            break
        except OSError:
            time.sleep(0.15)
    if sock is None:
        try:
            proc.kill()
        except Exception:
            pass
        _release_port(port)
        raise RuntimeError("zerostack ACP port never opened")
    sock.settimeout(None)
    with s.lock:
        # DELETE can land while we were waiting for the port to open: the
        # session was closed and its old process killed, so this new process
        # would be an orphan nobody can reach. Drop it instead.
        if s.closed or s.deleted:
            try:
                sock.close()
            except Exception:
                pass
            try:
                proc.kill()
            except Exception:
                pass
            _release_port(port)
            raise RuntimeError("session closed during spawn")
        s.proc, s.port, s.sock = proc, port, sock
        s.rfile = sock.makefile("rb")
    threading.Thread(target=_reader, args=(s,), daemon=True).start()
    rpc(s, "initialize", {"protocolVersion": 1,
                          "clientCapabilities": {"fs": {"readTextFile": False,
                                                        "writeTextFile": False},
                                                 "terminal": False}}, timeout=25)
    r = rpc(s, "session/new", {"cwd": s.cwd, "mcpServers": []}, timeout=25)
    with s.lock:
        s.acp_sid = (r.get("result") or {}).get("sessionId")
        if s.messages:
            # fresh process: earlier turns only exist in our transcript store
            s.needs_preamble = True
    log("session %s: zerostack up on 127.0.0.1:%s (pid %s)" % (s.id, port, proc.pid))


def ensure_proc(s):
    if s.closed or s.deleted:
        raise RuntimeError("session closed")
    if s.proc is not None and s.proc.poll() is None and s.sock is not None:
        return s
    kill_proc(s, quiet=True)
    spawn_proc(s)
    return s


def preamble(s, upto=None):
    """Text digest of the earlier conversation for a freshly spawned process.

    ACP has no session/load, so continuity after a respawn is carried in the
    next prompt. Text only (tool traffic is dropped), tail-capped, and bounded
    by /compact's replay_from. `upto` excludes the prompt being sent now, which
    is already in the transcript and would otherwise be duplicated.
    """
    with s.lock:
        end = len(s.messages) if upto is None else upto
        msgs = s.messages[s.replay_from:end]
        if not msgs:
            return ""
        lines = []
        for m in msgs:
            role = m.get("role")
            if role not in ("user", "assistant"):
                continue
            txt = ""
            for b in (m.get("content") or []):
                if isinstance(b, dict) and b.get("type") == "text":
                    txt += b.get("text") or ""
            txt = txt.strip()
            if txt:
                lines.append("%s: %s" % ("user" if role == "user" else "assistant",
                                         txt[:2000]))
        body = "\n\n".join(lines)
    if not body:
        return ""
    body = body[-PREAMBLE_MAX:]
    return ("[Earlier conversation in this chat — context only, do not repeat it]\n"
            + body + "\n[End of earlier conversation]")


def drop_empty_turn(s):
    """Remove a trailing assistant turn that never produced any content.

    A stopped run (or a failed spawn) leaves a turn object with no text and no
    tool calls behind; the page would render it as an empty "(no text)" bubble.
    """
    with s.lock:
        if s.messages and s.messages[-1].get("role") == "assistant":
            blocks = s.messages[-1].get("content") or []
            if not any(isinstance(b, dict) and (b.get("text") or b.get("thinking")
                                                or b.get("type") == "toolCall")
                       for b in blocks):
                s.messages.pop()


def run_turn(s, text):
    with s.lock:
        s.running = True
        s.turn = None
        s.last_assistant = ""
        # Append the user message HERE, not when /prompt was accepted. A queued
        # prompt must land after the previous assistant turn; appending it on
        # arrival produced user,user,assistant,assistant in the transcript.
        s.messages.append({"role": "user",
                           "content": [{"type": "text", "text": text}],
                           "timestamp": time.time() * 1000})
        idx = len(s.messages) - 1
    log_event(s, {"type": "agent_start"})
    try:
        if s.closed or s.deleted:
            raise RuntimeError("session closed")
        ensure_proc(s)
        payload = text
        with s.lock:
            pre = preamble(s, idx) if s.needs_preamble else ""
            s.needs_preamble = False
        if pre:
            payload = pre + "\n\n" + text
        rpc(s, "session/prompt",
            {"sessionId": s.acp_sid,
             "prompt": [{"type": "text", "text": payload}]},
            timeout=TURN_TIMEOUT)
    except Exception as e:
        log("session %s: turn failed: %s" % (s.id, e))
        log_event(s, {"type": "message_update",
                      "assistantMessageEvent": {"type": "text_delta",
                                                "delta": "\n[error: %s]" % e}})
        with s.lock:
            s.needs_preamble = True
    finally:
        drop_empty_turn(s)
        if s.closed or s.deleted:
            # DELETE landed mid-turn; any process spawned after its kill window
            # would be unreachable, so sweep once more here.
            kill_proc(s, quiet=True)
        with s.lock:
            s.running = False
            s.turn = None
            s.mtime = time.time()
        log_event(s, {"type": "agent_end"})
        save_session(s)


def worker(s):
    while not s.closed:
        try:
            item = s.q.get(timeout=1.0)
        except queue.Empty:
            continue
        if item is None:
            break
        if isinstance(item, tuple):
            text = item[0]
        else:
            text = item
        try:
            run_turn(s, text)
        except Exception as e:
            log("session %s worker error: %s" % (s.id, e))
            with s.lock:
                s.running = False


def ensure_worker(s):
    """One consumer thread per session, started lazily.

    Sessions restored from disk have no thread yet; without this a prompt to a
    restored session would sit in its queue forever.
    """
    with s.lock:
        if s.worker_started:
            return
        s.worker_started = True
    threading.Thread(target=worker, args=(s,), daemon=True).start()


def new_session(cwd=WORK, model="", name=""):
    sid = str(uuid.uuid4())
    s = Session(sid, name=name, cwd=cwd, model=model)
    SESSIONS[sid] = s
    ensure_worker(s)
    save_session(s)
    return s


def sessions_list():
    out = []
    with SESS_LOCK:
        for s in SESSIONS.values():
            out.append({"id": s.id, "name": s.name, "cwd": s.cwd, "model": s.model,
                        "messages": len(s.messages), "mtime": s.mtime,
                        "running": s.running,
                        "live": bool(s.proc and s.proc.poll() is None)})
    out.sort(key=lambda x: (1 if x["running"] else 0, x.get("mtime") or 0), reverse=True)
    return out


# ---- admin page -----------------------------------------------------------
ADMIN_HTML = r"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Zerostack Admin</title>
<style>
:root{--bg:#f6f7f9;--card:#fff;--tx:#16181d;--mut:#6b7280;--bd:#e3e6ea;--ac:#2563eb;--ok:#15803d;--bad:#dc2626}
@media (prefers-color-scheme:dark){:root{--bg:#0e1013;--card:#171a1f;--tx:#e7eaee;--mut:#9aa3ad;--bd:#262b33;--ac:#5b93ff}}
*{box-sizing:border-box}body{margin:0;font:15px/1.5 -apple-system,Segoe UI,Roboto,sans-serif;background:var(--bg);color:var(--tx)}
.wrap{max-width:880px;margin:0 auto;padding:20px 14px 60px}
h1{font-size:21px;margin:4px 0 2px}h2{font-size:16px;margin:0 0 10px}
.sub{color:var(--mut);font-size:13px;margin-bottom:16px}
.card{background:var(--card);border:1px solid var(--bd);border-radius:12px;padding:16px;margin-bottom:14px}
.row{display:flex;gap:8px;align-items:center;flex-wrap:wrap;padding:7px 0;border-bottom:1px solid var(--bd)}
.row:last-child{border-bottom:none}.row .grow{flex:1;min-width:130px}
.mono{font-family:ui-monospace,Menlo,monospace;font-size:13px}.mut{color:var(--mut);font-size:12.5px}
.badge{font-size:11.5px;padding:2px 8px;border-radius:99px;border:1px solid var(--bd)}
.badge.ok{color:var(--ok);border-color:var(--ok)}.badge.no{color:var(--bad);border-color:var(--bad)}
button{background:var(--ac);color:#fff;border:none;border-radius:8px;padding:7px 13px;font-size:13.5px;cursor:pointer}
button.ghost{background:transparent;color:var(--tx);border:1px solid var(--bd)}
input{background:var(--card);color:var(--tx);border:1px solid var(--bd);border-radius:8px;padding:8px 10px;font-size:14px}
#toast{position:fixed;left:50%;bottom:22px;transform:translateX(-50%);background:var(--tx);color:var(--bg);padding:9px 16px;border-radius:9px;font-size:13.5px;display:none;max-width:90%}
.note{background:rgba(37,99,235,.08);border-left:3px solid var(--ac);padding:9px 12px;border-radius:6px;font-size:13px;margin-top:10px}
</style></head><body><div class="wrap">
<h1>Zerostack Admin</h1>
<div class="sub">Staging gateway on port 8766. Models and keys are read from the live Zerostack config — this page never writes to it.</div>

<div class="card" id="loginCard"><h2>Unlock</h2>
<div class="row"><input type="password" id="tok" class="grow" placeholder="Opening password">
<button onclick="saveTok()">Unlock</button></div>
<div class="mut">The same opening password as Pi Chat. The raw gateway token also works.</div></div>

<div id="app" style="display:none">
<div class="card"><h2>Status</h2>
<div class="row"><div class="grow">Gateway <span id="health" class="badge">…</span> <span class="mut" id="deflt"></span></div>
<button class="ghost" onclick="loadAll()">Refresh</button>
<button onclick="restartGw()">Restart gateway</button></div>
<div class="mut">Restarting drops every live Zerostack process; transcripts stay on disk and resume on the next prompt.</div></div>

<div class="card"><h2>Models <span class="mut">(from config.toml)</span></h2><div id="models"></div></div>

<div class="card"><h2>Provider keys</h2><div id="keys"></div>
<div class="note">Keys are read from the box environment (provider-keys.env and /root/.profile). Values are never shown or returned by this page.</div></div>

<div class="card"><h2>ACP facts for this build</h2>
<div class="mut" style="line-height:1.7">
• Zerostack ACP exposes only <span class="mono">initialize</span>, <span class="mono">session/new</span>, <span class="mono">session/prompt</span>.<br>
• <span class="mono">session/cancel</span> does not exist → Stop ends the session's Zerostack process.<br>
• No <span class="mono">session/load</span> → the gateway stores transcripts and replays a text digest after a respawn.<br>
• MCP is disabled in ACP mode, so the vault tools are not available through this gateway.
</div></div>
</div></div>
<div id="toast"></div>
<script>
const $=id=>document.getElementById(id);
let TOK=sessionStorage.getItem('zsgtok')||'';
function toast(m){const t=$('toast');t.textContent=m;t.style.display='block';setTimeout(()=>t.style.display='none',4600)}
async function api(path,opts={}){opts.headers=Object.assign({'Authorization':'Bearer '+TOK,'Content-Type':'application/json'},opts.headers||{});
 const r=await fetch(path,opts);let d={};try{d=await r.json()}catch(e){}
 if(r.status===401)throw new Error('bad token');if(!r.ok)throw new Error(d.error||('HTTP '+r.status));return d}
function esc(s){return String(s==null?'':s).replace(/[&<>"]/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;'}[c]))}
async function saveTok(){const v=$('tok').value.trim();if(!v)return;
 try{const r=await fetch('/admin-login',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({password:v})});
  if(r.ok){const d=await r.json();TOK=d.token;sessionStorage.setItem('zsgtok',TOK);loadAll();return}}catch(e){}
 TOK=v;sessionStorage.setItem('zsgtok',TOK);loadAll()}
async function loadAll(){try{
 const [health,models,keys]=await Promise.all([api('/health'),api('/models'),api('/keys')]);
 $('loginCard').style.display='none';$('app').style.display='block';
 $('health').textContent=health.ok?'running':'?';$('health').className='badge ok';
 $('deflt').textContent='Default: '+((models.default||{}).provider||'?')+'/'+((models.default||{}).model||'?');
 const el=$('models');el.innerHTML='';
 const provs=models.providers||{};
 Object.keys(provs).sort().forEach(p=>{const pv=provs[p];
  const h=document.createElement('div');h.className='mono';h.style.fontWeight='600';h.style.marginTop='8px';h.textContent=p;el.appendChild(h);
  if(pv.baseUrl){const b=document.createElement('div');b.className='mut mono';b.textContent=pv.baseUrl+'  ·  key: '+(pv.apiKey||'(none)');el.appendChild(b)}
  (pv.models||[]).forEach(m=>{const row=document.createElement('div');row.className='row';
   row.innerHTML='<div class="grow"><span class="mono">'+esc(m.id)+'</span> <span class="mut">ctx '+(m.contextWindow||'?')+' · out '+(m.maxTokens||'?')+'</span></div>';
   el.appendChild(row)})});
 if(!Object.keys(provs).length)el.innerHTML='<div class="mut">No providers in config.</div>';
 const ke=$('keys');ke.innerHTML='';
 (keys.keys||[]).forEach(k=>{const row=document.createElement('div');row.className='row';
  row.innerHTML='<div class="grow"><span class="mono">'+esc(k.env_name)+'</span> <span class="badge '+(k.set?'ok':'no')+'">'+(k.set?'set':'not set')+'</span></div>';
  ke.appendChild(row)});
 if(!(keys.keys||[]).length)ke.innerHTML='<div class="mut">No provider keys referenced.</div>';
}catch(e){$('loginCard').style.display='block';$('app').style.display='none';if(TOK)toast(e.message)}}
async function restartGw(){try{await api('/restart',{method:'POST',body:'{}'});toast('Restarting…')}catch(e){}
 for(let i=0;i<20;i++){await new Promise(r=>setTimeout(r,1500));try{const h=await fetch('/health').then(r=>r.json());if(h.ok){toast('Gateway back up');loadAll();return}}catch(e){}}toast('Gateway did not answer yet')}
if(TOK){$('tok').value=TOK;loadAll()}
</script></body></html>
"""


# ---- HTTP -----------------------------------------------------------------
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _auth(self):
        if urlparse(self.path).path == "/health":
            return True
        return self.headers.get("Authorization") == "Bearer " + TOKEN

    def _json(self, code, obj):
        body = json.dumps(obj).encode()
        try:
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            # The page polls fast and often navigates away mid-request; a dead
            # client is not an error worth a traceback in the log.
            pass

    def _html(self, body):
        try:
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-cache")
            self.end_headers()
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def _body(self):
        n = int(self.headers.get("Content-Length") or 0)
        if not n:
            return {}
        try:
            return json.loads(self.rfile.read(n) or b"{}")
        except Exception:
            return {}

    def do_GET(self):
        p = urlparse(self.path).path
        if p in ("/pi", "/pi.html"):
            try:
                with open(os.path.join(BASE, "zs.html"), "rb") as f:
                    return self._html(f.read())
            except Exception:
                return self._html(b"<!doctype html><meta charset=utf-8><title>Zerostack Chat</title>"
                                  b"<p style='font:15px system-ui;padding:24px'>zs.html is missing "
                                  b"on the box. Copy it to /root/pi-bridge/zs.html and reload.")
        if p == "/admin":
            return self._html(ADMIN_HTML.encode())
        if not self._auth():
            return self._json(401, {"error": "unauthorized"})
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/health":
            return self._json(200, {"ok": True,
                                    "sessions_running": sum(1 for s in SESSIONS.values() if s.running),
                                    "backend": "zerostack"})
        if u.path == "/stats":
            return self._json(200, stats_snapshot())
        if u.path == "/sessions":
            return self._json(200, sessions_list())
        if u.path == "/models":
            return self._json(200, models_payload())
        if u.path == "/keys":
            return self._json(200, {"keys": provider_key_status()})
        if u.path == "/switch":
            return self._json(200, {"backend": "zerostack",
                                    "url": ZS_PUBLIC_URL or "/pi",
                                    "pi_url": PI_PUBLIC_URL})
        if u.path == "/settings":
            cfg = zs_config()
            return self._json(200, {"defaultProvider": cfg.get("provider"),
                                    "defaultModel": cfg.get("model")})

        m = re.match(r"^/sessions/([^/]+)/status$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            return self._json(200, {"id": m.group(1),
                                    "running": bool(s and s.running),
                                    "last_assistant_text": (s.last_assistant if s else None)})
        m = re.match(r"^/sessions/([^/]+)/events$", u.path)
        if m:
            sid = m.group(1)
            since = int(q.get("since", ["0"])[0])
            s = SESSIONS.get(sid)
            evs = s.events[since:] if s else []
            if not s:
                try:
                    with open(os.path.join(SESS_DIR, sid + ".events.jsonl")) as f:
                        evs = [json.loads(l) for l in f.readlines()[since:]]
                except Exception:
                    pass
            return self._json(200, {"events": evs, "next": since + len(evs)})
        m = re.match(r"^/sessions/([^/]+)$", u.path)
        if m:
            sid = m.group(1)
            s = SESSIONS.get(sid)
            if not s:
                return self._json(404, {"error": "not found"})
            with s.lock:
                msgs = list(s.messages)
            user = sum(1 for x in msgs if x.get("role") == "user")
            asst = sum(1 for x in msgs if x.get("role") == "assistant")
            return self._json(200, {
                "id": sid,
                "messages": {"data": {"messages": msgs}},
                "stats": {"data": {"userMessages": user, "assistantMessages": asst}},
            })
        return self._json(404, {"error": "unknown route"})

    def do_POST(self):
        if urlparse(self.path).path == "/admin-login":
            now = time.time()
            st = LOGIN_FAILS.get("state", {"count": 0, "until": 0})
            if st["until"] > now:
                return self._json(429, {"error": "too many attempts — try again in a few minutes"})
            body = self._body()
            pw = (body.get("password") or "").encode()
            digest = hashlib.sha256(pw).hexdigest()
            if ADMIN_PASS_SHA256 and hmac.compare_digest(digest, ADMIN_PASS_SHA256.strip().lower()):
                LOGIN_FAILS["state"] = {"count": 0, "until": 0}
                return self._json(200, {"token": TOKEN})
            st["count"] = st.get("count", 0) + 1
            if st["count"] >= 5:
                st = {"count": 0, "until": now + 600}
            LOGIN_FAILS["state"] = st
            return self._json(401, {"error": "wrong password"})
        if not self._auth():
            return self._json(401, {"error": "unauthorized"})
        u = urlparse(self.path)
        body = self._body()

        if u.path == "/prompt":
            text = (body.get("message") or "").strip()
            if not text:
                return self._json(400, {"error": "message required"})
            sid = body.get("session_id")
            s = SESSIONS.get(sid) if sid else None
            if sid and not s:
                return self._json(404, {"error": "unknown session"})
            if not s:
                s = new_session(cwd=body.get("cwd") or WORK,
                                model=body.get("model") or "",
                                name=body.get("name") or "")
            ensure_worker(s)
            with s.lock:
                s.mtime = time.time()
                if body.get("model"):
                    s.model = body["model"]
                was_running = s.running
            s.q.put(text)
            if not was_running:
                # give the worker a moment to flip running so the page shows Stop
                time.sleep(0.05)
            save_session(s)
            return self._json(200, {"session_id": s.id, "accepted": True,
                                    "disposition": "queued" if was_running else "sent",
                                    "response": {"success": True}})

        m = re.match(r"^/sessions/([^/]+)/prompt$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            if not s:
                return self._json(404, {"error": "unknown session"})
            ensure_worker(s)
            text = (body.get("message") or "").strip()
            if not text:
                return self._json(400, {"error": "message required"})
            with s.lock:
                s.mtime = time.time()
                was_running = s.running
            s.q.put(text)
            save_session(s)
            return self._json(200, {"success": True,
                                    "disposition": "queued" if was_running else "sent"})

        m = re.match(r"^/sessions/([^/]+)/stop$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            if not s:
                return self._json(404, {"error": "unknown session"})
            # ACP has no session/cancel: the only stop is ending the process.
            was = s.running
            kill_proc(s)
            with s.lock:
                s.running = False
                s.turn = None
                s.needs_preamble = True
                s.mtime = time.time()
            drop_empty_turn(s)
            if was:
                log_event(s, {"type": "agent_end"})
            save_session(s)
            return self._json(200, {"success": True, "stopped": was,
                                    "note": "ACP has no cancel; the session process was ended"})

        m = re.match(r"^/sessions/([^/]+)/rename$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            if not s:
                return self._json(404, {"error": "unknown session"})
            name = (body.get("name") or "").strip()
            if not name:
                return self._json(400, {"error": "name required"})
            with s.lock:
                s.name = name
                s.mtime = time.time()
            save_session(s)
            log_event(s, {"type": "session_info_changed"})
            return self._json(200, {"success": True, "name": name})

        m = re.match(r"^/sessions/([^/]+)/model$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            if not s:
                return self._json(404, {"error": "unknown session"})
            model = (body.get("model") or "").strip()
            if not model:
                return self._json(400, {"error": "model required"})
            with s.lock:
                s.model = model
                s.mtime = time.time()
                s.needs_preamble = True
            # the running process is pinned to the old model: end it, the next
            # prompt respawns with the new one and gets the history digest
            kill_proc(s)
            save_session(s)
            log_event(s, {"type": "session_info_changed"})
            return self._json(200, {"success": True, "model": model,
                                    "note": "process restarted on next prompt"})

        m = re.match(r"^/sessions/([^/]+)/compact$", u.path)
        if m:
            s = SESSIONS.get(m.group(1))
            if not s:
                return self._json(404, {"error": "unknown session"})
            with s.lock:
                s.replay_from = len(s.messages)
                s.needs_preamble = False
                s.mtime = time.time()
            save_session(s)
            return self._json(200, {"success": True,
                                    "note": "earlier turns are no longer replayed into new processes; transcript kept"})

        if u.path == "/switch":
            # The Pi | Zerostack switcher: the page asks the gateway it is
            # currently served by where the other backend lives, then navigates.
            # Each backend then serves its own page and its own API same-origin,
            # so the Pi side keeps talking to the Pi gateway exactly as today.
            want = (body.get("backend") or "").strip().lower()
            if want == "pi":
                return self._json(200, {"backend": "pi", "url": PI_PUBLIC_URL})
            if want == "zerostack":
                return self._json(200, {"backend": "zerostack", "url": ZS_PUBLIC_URL or "/pi"})
            return self._json(400, {"error": "backend must be 'pi' or 'zerostack'"})

        if u.path == "/restart":
            # The tmux session runs zs-supervise.sh, a loop that respawns this
            # script when it exits. Killing ourselves from inside the tmux
            # session would also kill the `tmux new` subprocess we spawned, so
            # the reliable restart is simply to exit and let the loop do it.
            def _restart():
                time.sleep(1.5)
                for s in list(SESSIONS.values()):
                    s.closed = True
                    kill_proc(s, quiet=True)
                    save_session(s)
                log("restart requested — exiting for the supervisor loop")
                os._exit(0)
            threading.Thread(target=_restart, daemon=True).start()
            return self._json(200, {"ok": True, "restarting": True})

        return self._json(404, {"error": "unknown route"})

    def do_DELETE(self):
        if not self._auth():
            return self._json(401, {"error": "unauthorized"})
        m = re.match(r"^/sessions/([^/]+)$", urlparse(self.path).path)
        if not m:
            return self._json(404, {"error": "unknown route"})
        sid = m.group(1)
        s = SESSIONS.pop(sid, None)
        if s:
            with s.lock:
                s.closed = True
                s.deleted = True
            try:
                s.q.put(None)
            except Exception:
                pass
            # A worker can be mid-spawn or mid-turn when DELETE lands. Keep
            # sweeping briefly so no process survives that nobody can reach,
            # then remove the files last (the tombstone stops writes).
            for _ in range(6):
                kill_proc(s, quiet=True)
                time.sleep(0.1)
        for suffix in (".json", ".json.tmp", ".events.jsonl", ".log"):
            try:
                os.remove(os.path.join(SESS_DIR, sid + suffix))
            except FileNotFoundError:
                pass
        return self._json(200, {"ok": True})


def idle_reaper():
    """Release the zerostack process of sessions that have gone quiet."""
    while True:
        time.sleep(IDLE_SWEEP_SECS)
        now = time.time()
        for s in list(SESSIONS.values()):
            try:
                if s.running or s.proc is None or s.proc.poll() is not None:
                    continue
                if now - s.mtime < IDLE_REAP_SECS:
                    continue
                kill_proc(s, quiet=True)
                with s.lock:
                    s.needs_preamble = True
                log("session %s: idle, process released" % s.id)
            except Exception as e:
                log("idle reap %s failed: %s" % (s.id, e))


if __name__ == "__main__":
    load_sessions()
    _seed_stats()
    threading.Thread(target=idle_reaper, daemon=True).start()
    log("zs-gateway listening on %s:%s (zerostack ACP, %d stored sessions)"
        % (HOST, PORT, len(SESSIONS)))

    class Srv(ThreadingHTTPServer):
        request_queue_size = 128
        daemon_threads = True

    Srv((HOST, PORT), H).serve_forever()
