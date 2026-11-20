<!-- Installed for Zerostack on the Altare box, 2026-11-20, from Pi's pi-delegation (rewritten for Zerostack). -->
<!-- Zerostack has no skills directory: this file IS the skill, as a named prompt. -->
<!-- Load it with `/prompt pi-delegation`, `.pi-delegation <msg>`, or `zerostack --load-prompt pi-delegation`. -->
# pi-delegation

> Delegation skill, rewritten for Zerostack: open one session per topic through the zs-gateway, report progress, and recover a dead session by respawning and replaying the transcript digest.

## What this is

How work gets delegated **to Zerostack** on the Altare box. It replaces the old
Pi delegation skill: Pi is now the fallback, Zerostack is the target.

The transport is the **zs-gateway** (`/root/pi-bridge/zs-gateway.py`, `127.0.0.1:8766`).
Never drive a bare `zerostack` process for delegated work — the gateway owns the
transcript store, and that store is the only thing that survives a process death.

## Why the gateway, not the process

Zerostack's ACP server (1.8.6) exposes only `initialize`, `session/new` and
`session/prompt`. There is **no `session/load`, `session/list`, `session/resume`
or `session/cancel`**, and sessions live in an in-process `HashMap`. Consequences
that shape every rule below:

| Fact | What you must do |
|---|---|
| `session/prompt` answers only at turn end | never block on it; poll `/sessions/{id}/status` |
| No `session/cancel` | stop = kill the process (`/sessions/{id}/stop`); the transcript is kept |
| Sessions die with the process | the gateway replays a text digest on respawn — keep the session's transcript clean |
| Only **text** is replayed, tail-capped at 12 000 chars | a long tool-heavy history is *lost* on respawn; write durable state to files |
| MCP is disabled in ACP mode | vault tools are **TUI-only**; never plan a delegated task that needs them |
| One client per process | one process per session; do not multiplex |

## One session per topic

Standing topics on this box (created 2026-11-20, ids in
`/root/work/zerostack-bridge/zerostack-sessions.json`):

| Topic | Session name | Use for |
|---|---|---|
| `vault` | `vault` | cf-d1-vault Worker, keys, GitHub reads (TUI only for the MCP tools) |
| `setup repo` | `setup-repo` | 0xSatwik/muse-pi-setup, Pi/Zerostack setup files, gateway and search scripts |
| `general work` | `general-work` | ad-hoc coding, research, maintenance — the default target |

Rules:

1. **Reuse the topic session. Do not open a new one per task.** A topic session
   accumulates context that is replayed after a respawn; a fresh session per task
   throws that away and burns tokens re-establishing it.
2. Open a **new** session only for a topic that is genuinely not one of the three.
   Name it after the topic, not the task (`redis-migration`, never `fix-1234`).
3. Work that needs a different working directory gets its own session with
   `--cwd`, because the project slug for memory and the tool's relative paths
   both derive from the session's cwd.
4. **Never run two tasks in the same topic session at once.** The gateway queues
   a second prompt (`disposition: queued`) and the two turns interleave badly in
   the transcript. One in-flight task per session.

## Delegating a task

On the box, with `bash`:

```bash
# 1. find or create the session
python3 /root/work/zerostack-bridge/zs.py list

# 2. send the task, do not wait for the whole turn in one blocking call
python3 /root/work/zerostack-bridge/zs.py ask general-work "<task>" --wait 0

# 3. poll until it stops running
python3 /root/work/zerostack-bridge/zs.py wait general-work --secs 900

# 4. read the tail of the transcript
python3 /root/work/zerostack-bridge/zs.py show general-work --tail 4
```

Off the box (Muse), the same three calls over HTTP with the gateway bearer token
read from the environment — never inline the token, never pass it as an argument
(it lands in `ps`):

```
POST /prompt                      {"message": "...", "name": "<topic>", "cwd": "/root/work"}
POST /sessions/<id>/prompt        {"message": "..."}
GET  /sessions/<id>/status        -> {"running": bool, "last_assistant_text": "..."}
GET  /sessions/<id>/events?since=N
GET  /sessions                    -> [{id, name, cwd, messages, running, live}]
POST /sessions/<id>/stop
```

## Write the task so it survives a respawn

The replay is **text only and tail-capped**, so a task brief must not depend on
the transcript. Every delegated task carries:

1. **Goal** — one sentence, what "done" means.
2. **Inputs** — absolute paths, not "the file we discussed".
3. **Outputs** — the exact file(s) to write, and their format.
4. **Checkpoint file** — a path the worker appends progress to, e.g.
   `/root/work/<topic>/PROGRESS.md`. This is what a respawned session reads first.
5. **Constraints** — read-only vs may-edit, which services must not restart.
6. **Done signal** — the literal last line the worker must print, so the caller
   can tell completion from a stall: `DONE: <one-line summary>`.

A brief that says "continue what we were doing" is a broken brief: after a
respawn there is nothing to continue from.

## Progress reporting

- The worker appends to the checkpoint file **after each unit of work**, not at
  the end: `- [x] <unit> — <result/where>`.
- The caller polls `status` every 10–20 s; do not poll faster, each poll is cheap
  but the turn is not.
- Report to the user on **state changes only**: started, unit completed, blocked,
  done. Not on every poll.
- `last_assistant_text` is the last *complete* assistant message. Mid-turn it is
  stale by design — treat it as "last known", never as "current".
- Silence for longer than the turn timeout (3600 s) is a stall, not progress.

## Recovery from a dead session

Symptom → cause → action:

| Symptom | Cause | Action |
|---|---|---|
| `status.running` true forever, `last_assistant_text` frozen | provider stall or a hung tool call | `/sessions/<id>/stop` (kills the process), then re-send the task with a resume instruction |
| `live: false`, `running: false`, transcript intact | idle reaper (900 s) released the process — **normal, not a failure** | just send the next prompt; the gateway respawns and replays the digest |
| `/sessions/<id>` 404 but `zs-sessions/<id>.json` exists | gateway restarted and did not reload the session | re-create the session with the same name and replay the brief; the JSON file is still on disk for salvage |
| Process died mid-turn, no assistant text | crash | re-send; the brief's checkpoint file is the resume point |
| Replay is missing early context | 12 000-char tail cap | re-state the essentials in the new prompt; do not trust the digest for anything older |

**Recovery procedure, in order:**

1. Read the checkpoint file. It is the only trustworthy record of what completed.
2. `zs.py status <session>` — is it running, and what was the last complete text?
3. If running and the checkpoint has not moved in >10 minutes: `zs.py kill <session>`.
4. Re-send a **resume brief**: the goal, the checkpoint path, what is done, what
   is next, and "do not redo completed units". Never re-send the original brief
   verbatim — that repeats finished work.
5. Verify the outputs on disk, not from the transcript. A session can claim
   success after a tool failure.
6. If the same session dies twice on the same unit, split the unit or move the
   task to a new session named `<topic>-2`; do not retry a third time.

## Do not

- Do not delegate a task whose answer depends on vault MCP tools through the
  gateway — they are not there. Run it in the Zerostack TUI, or do the vault call
  from a harness that has MCP.
- Do not `zerostack --resume` a session the gateway owns; two owners of one
  transcript corrupt it.
- Do not restart the gateway, the tunnel, or the Pi watcher to "fix" a stuck
  session. Kill the session's process, not the service.
- Do not treat the gateway's `accepted: true` as "the task ran". It means queued.
