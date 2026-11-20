# Recall — fast, tiny, cheap session memory

The problem: a session transcript is 1-2 MB of plain text. Loading it to
remember something costs 10k+ tokens, takes seconds, and eats disk. The
agent needs to recall any session in an instant, for almost no tokens, in
almost no space.

The rule: **never store or recall the full transcript. Store a dense
digest + a tiny index. The full transcript stays on disk untouched, and is
only read when explicitly asked for.**

## The three parts

### 1. Index — `index.jsonl` (one line per session, ~150 bytes each)
One JSON object per line. This is the only file a recall reads first.
Grep it and you find any session in milliseconds.

```json
{"id":"5f7642a3","name":"model-system","model":"100milionseek/deepseek-v4.1-flash-free","msgs":102,"updated":"2026-11-21","topics":["rust-gateway","admin","model-system"],"sum":"Rust gateway + Admin model system; per-provider load/add key; fallback order."}
```

Fields: `id` (first 8 chars of the session id), `name`, `model`,
`msgs` (message count), `updated` (date), `topics` (short tags),
`sum` (one line, what this session is about). No transcript, no quotes.

### 2. Digest — `digests/<id>.md` (~1-2 KB each, ~200-400 tokens)
The condensed record of a session. This is what a recall returns. It is
NOT the transcript — it is the decisions, facts, outcomes and open items,
in short lines. ~1000x smaller than the transcript.

```md
# model-system
- id: 5f7642a3, model: 100milionseek/deepseek-v4.1-flash-free, msgs: 102, updated: 2026-11-21
- purpose: Rust gateway + Admin model system + IDE redesign
- decisions: gateway is Rust (9 MB), Python kept as backup; fallback chain in gateway
- facts: provider /models works with stored key; rotation is round-robin; Zerostack has no built-in fallback
- outcomes: Rust gateway live on 8766; Admin has per-provider Load models + Add key; fallback order with Up/Down
- open: visual polish pass; Pi back-link needs second hostname
- full: /root/pi-bridge/zs-sessions/5f7642a3-5063-4b34-bf6a-0282a82775d6.json
```

The `full:` line points at the transcript. Read it ONLY when the digest
is not enough — and say so when you do, because it costs tokens.

### 3. Lessons — `lessons/<slug>.md` (~300 bytes each) + `lessons-index.md`
One file per lesson. A lesson is written the moment a mistake is found or
fixed, so it is never repeated. Format is fixed and tiny:

```md
# pkill-f-kills-ssh
- trigger: pkill -f "playwright-mcp" run inside an SSH command killed the SSH session itself
- rule: never pkill -f inside an SSH command; kill by port PID instead (ss -tlnp -> pid -> kill)
- applies: instabox, altare, any remote shell
```

`lessons-index.md` is one line per lesson: `slug — one-line rule`.
Read the index first; read a lesson file only when it applies.

## How recall works (the skill does this)

1. **Find**: `grep -i "<keyword>" recall/index.jsonl` — instant, one file.
2. **Read**: `cat recall/digests/<id>.md` — ~200-400 tokens, the answer.
3. **Full transcript only if needed**: read the `full:` path. This is
   expensive; avoid it. If you do, say so.
4. **Lessons**: `grep -i "<keyword>" recall/lessons-index.md`, then read
   that one lesson file.

## How to write

- **Digest**: when a session finishes important work (or once a day for a
  long session), write/update `digests/<id>.md` from the transcript. Keep
  it to decisions, facts, outcomes, open items. No quotes, no full logs.
- **Lesson**: the moment a mistake is found or fixed, write
  `lessons/<slug>.md` and add one line to `lessons-index.md`. Do not wait.
  Do not bundle lessons into a big file — one file per lesson, so grep
  finds it.
- **Index**: after writing a digest, add/update its one line in
  `index.jsonl`. Keep the line short.

## Why this satisfies all three

| need | how | number |
|---|---|---|
| instant recall | one small index file, grep | milliseconds |
| few tokens | digest only, ~200-400 tokens | ~50x less than a transcript |
| tiny space | digest 1-2 KB vs transcript 1-2 MB | ~1000x smaller |

The full transcript is never deleted — it is the ground truth. But it is
never the recall unit. Recall reads the index and the digest. That is the
whole architecture.

## Automatic updates — `auto_digest.py` (2026-11-21)

Digests used to go stale because they were only written when the agent
remembered to write one. Now they stay current on their own.

- **Runner:** `auto_digest.py`, in tmux session `recall-auto` on the box.
  It runs once at start, then every 6 hours (`sleep 21600` loop). No cron
  and no systemd exist in this container (PID 1 is `tini`), so the tmux
  loop is the scheduler. If the box restarts, restart the tmux session
  along with the gateway / relay / tailscaled ones.
- **What it does per run:** reads the session files in
  `/root/pi-bridge/zs-sessions/`, compares each message count to
  `digests/.cursor.json` (the count the digest already covers).
  - New messages: summarizes ONLY the new messages (the delta) with a
    cheap model (100milionseek, fallback tokenlab, then moss) and merges
    them into the digest. The model never sees the full transcript — a
    digest update costs a few hundred tokens.
  - New session file: adds it to the index and creates an initial digest.
  - Session file GONE (the user deleted the chat in /pi): deletes that
    session's digest AND its index line. Deleting a session deletes its
    memory. That is the rule, and it is tested (a fake session was added,
    then its file deleted, and the digest + index entry were removed).
  - Model call fails: the session is marked stale in the log and retried
    next run. Nothing is deleted or overwritten on a failure.
- **muse-main is NOT auto-updated by this script.** That digest covers the
  Muse main-chat work, which lives in the Muse conversation, not in a
  Zerostack session file. Muse updates it itself (AGENTS.md standing rule:
  after finishing important work, update `digests/muse-main.md`).
- **Log:** `auto_digest.log` (one line per action). Manual run:
  `python3 /root/work/recall/auto_digest.py`.

