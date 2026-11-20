# session-recall

Recall any session/chat or lesson instantly, for few tokens, in tiny space.
The rule: **never load a full transcript to remember something. Read the
index, then a digest. The transcript is ground truth, not the recall unit.**

## Where it lives

- On the box: `/root/work/recall/`
- Local copy: `~/workspace/altare-pi-setup/recall/` (synced to the repos)
- CLI: `recall/recall.py` (run from the recall dir, or `python3 recall.py`)

## Recall a session (3 steps, in order)

1. **Find** — grep the index (one small file, milliseconds):
   ```
   grep -i "<keyword>" /root/work/recall/index.jsonl
   # or: python3 /root/work/recall/recall.py find <keyword>
   ```
   Each line is one session: id, name, model, msg count, date, topics, one-line summary.

2. **Read the digest** — the condensed record (~200-400 tokens):
   ```
   cat /root/work/recall/digests/<id>.md
   # or: python3 /root/work/recall/recall.py digest <id|name>
   ```
   The digest has: purpose, decisions, facts, outcomes, open items, and a
   `full:` line pointing at the transcript. This is the answer. Stop here
   unless the digest is not enough.

3. **Full transcript ONLY if needed** — read the `full:` path. This is
   10k+ tokens. Say so when you do it, because it is expensive. Never load
   a transcript just to "check" — the digest is the check.

## Recall a lesson

```
grep -i "<keyword>" /root/work/recall/lessons-index.md
# or: python3 /root/work/recall/recall.py lesson <keyword>
```
Then read that one lesson file (`lessons/<slug>.md`) only if it applies.
Before doing risky work (pkill, tunnel change, gateway restart, MCP config,
browser work), grep the lessons index for the thing you are about to do.

## Write a digest

When a session finishes important work (or once a day for a long session),
write/update `digests/<id>.md`. Keep it to these lines, short:

```
# <name>
- id: <id>, model: <model>, msgs: <n>, updated: <date>
- purpose: <one line>
- decisions: <what was decided, short>
- facts: <what was learned, short>
- outcomes: <what is now live/done, short>
- open: <what is still outstanding>
- full: /root/pi-bridge/zs-sessions/<full-id>.json
```

No quotes, no full logs, no transcript. Then add/update its one line in
`index.jsonl`. Keep the line short (one-line `sum`).

## Write a lesson (the moment a mistake is found or fixed)

Do not wait. Do not bundle lessons into a big file. One file per lesson,
so grep finds it. Format is fixed and tiny:

```
# <slug>
- trigger: <what happened, one line>
- rule: <the rule, one line>
- applies: <where it applies, one line>
```

File: `lessons/<slug>.md`. Then add one line to `lessons-index.md`:
`<slug> — <one-line rule>`.

Write a lesson for: a mistake, a wrong assumption, a false positive, a
silent failure, a command that killed something it should not have, a
config that was silently ignored, a test that passed but was wrong.

## The three numbers

| need | how | number |
|---|---|---|
| instant recall | one small index file, grep | milliseconds |
| few tokens | digest only, ~200-400 tokens | ~50x less than a transcript |
| tiny space | digest 1-2 KB vs transcript 1-2 MB | ~1000x smaller |

## Rules

- Never paste a full transcript into a recall answer.
- Never write session data as plain normal text in the recall store — use
  the index + digest + lesson format.
- Never delete a transcript — it is the ground truth. The digest is a
  condensation, not a replacement.
- A digest is written from the transcript; a lesson is written the moment
  the mistake is found.
