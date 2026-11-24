# Drift report — MEMORY.md vs recall digests

Run once a month (or on demand). Compare the box MEMORY.md against the recall
digests and report where they disagree. This catches facts that changed but
were never written back to MEMORY.md, and facts in MEMORY.md that are now
wrong.

## Inputs

- Box MEMORY.md: `/root/.local/share/zerostack/agent/memory/MEMORY.md`
- Recall digests: `/root/work/recall/digests/*.md` (one per session, plus
  `muse-main.md`)
- Recall index: `/root/work/recall/index.jsonl`

## What to compare

- A fact in a digest that is newer than the same fact in MEMORY.md (a value,
  a date, a status, a decision that changed). Report the digest date and the
  MEMORY.md wording.
- A fact in MEMORY.md that no digest supports any more (superseded, reversed,
  or the session that held it is gone). Report it as stale.
- A fact that appears in two digests with different values (the digests
  disagree with each other). Report both.
- A session whose digest is missing (the session file is gone but the digest
  remains, or vice versa). Report it as a recall gap.

## What NOT to do

- Do not edit MEMORY.md. Report only. A human (Muse) applies the fixes.
- Do not delete a digest or an index line. Deleting a session's memory is a
  user decision, not a drift-report action.
- Do not treat a digest being older than MEMORY.md as drift — the digest is
  a snapshot, MEMORY.md is the living file.

## Output

One line per drift item, each with: the fact, where it lives (MEMORY.md line
or digest file + date), what disagrees, and a severity (wrong / stale /
missing). End with a count and a one-line verdict: MEMORY.md is current, or
it needs a refresh (and which sections).

## Cadence

Monthly. A lightweight cron checks whether MEMORY.md is more than 7 days old
or any digest is newer than it, and logs the result. The full comparison
(this prompt) runs on demand when the check flags drift.
