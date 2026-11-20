## What this is

How to run a long batch — many units of work through a model or a flaky API —
so that one hang, one 5xx or one killed process does not cost the whole run.
Four mechanisms, all required: **checkpointing, reconciliation, pacing, and a
per-call timeout**. A batch missing any one of them fails in a different way.

Applies to: bulk model calls, batch file transforms, crawl/fetch runs, bulk
imports, migrations, and any loop that shells out to `zerostack`, `pi`, or an
HTTP provider.

## The four mechanisms and the failure each one prevents

| Mechanism | Without it | Cost |
|---|---|---|
| Checkpoint after every unit | a crash at unit 900 restarts at 1 | the whole run |
| Reconcile output vs input | silent gaps and duplicates go unnoticed | wrong answers, undetected |
| Pace provider calls | 429 storms and a ban, not slow progress | hours of cooldown |
| Timeout on every call | one hung socket stalls the run forever | the run, silently |

## 1. Checkpointing

**Rule: one unit, one record, written before the next unit starts.**

Append-only JSONL is the default — a partial final line is recoverable, a
rewritten JSON array is not:

```python
import json, os
from pathlib import Path

CKPT = Path("/root/work/<run>/ckpt.jsonl")

def done_ids(path: Path) -> set[str]:
    """Read completed unit ids. A truncated last line is skipped, not fatal."""
    ids = set()
    if not path.exists():
        return ids
    for line in path.read_text().splitlines():
        try:
            ids.add(json.loads(line)["id"])
        except (json.JSONDecodeError, KeyError):
            continue          # torn write from a kill — the unit simply re-runs
    return ids

def record(path: Path, unit_id: str, result: dict) -> None:
    """fsync so a hard kill cannot lose a completed unit."""
    with open(path, "a") as fh:
        fh.write(json.dumps({"id": unit_id, "result": result}) + "\n")
        fh.flush()
        os.fsync(fh.fileno())
```

Rules that matter:

- **Write the checkpoint before doing anything irreversible** for the next unit.
  A checkpoint written at the end of the run is not a checkpoint.
- **Idempotent units.** Re-running a unit must be safe. If it is not (it posts,
  it charges, it sends), record an intent line *before* the call and reconcile
  the intent on resume.
- **`fsync` the append.** A buffered write is lost on `kill -9`, which is exactly
  the failure you are protecting against.
- **Resume by skipping, never by truncating.** `done_ids()` is the resume set.
- **Keep the checkpoint next to the output**, in the same directory, so a
  half-finished run is self-describing.
- Do not checkpoint into memory only and flush "every 100 units" — that is a
  100-unit loss window chosen for no reason.

## 2. Reconciliation

At the end of a run, and again after any resume, compare three sets:

```
expected = ids from the input
done     = ids in the checkpoint          (attempted, whatever the outcome)
written  = ids present in the actual output (files on disk, rows in the DB)
```

**`done` is not `succeeded`.** The checkpoint records outcomes, so split it before
comparing, or every recorded failure looks like an orphan:

```
succeeded = {r["id"] for r in records if r.get("status") == "ok"}
failed    = {r["id"] for r in records if r.get("status") in ("error", "timeout")}
```

| Difference | Meaning | Action |
|---|---|---|
| `expected - done` | never attempted | run them |
| `succeeded - written` | **claimed ok but not produced** | re-run; the checkpoint lied |
| `failed & written` | produced despite a recorded failure | investigate; the record is wrong |
| `written - expected` | extra output | investigate before keeping it |
| `done - (succeeded \| failed)` | a record with no status | treat as failed and re-run |

The identity to assert is `len(succeeded) + len(failed) == len(done)`, and
`len(done) + len(expected - done) == len(expected)`. When those two hold and
`succeeded - written` is empty, the run is accounted for — failures included.

`succeeded - written` is the one people skip, and it is the one that matters: a tool
call can return success while writing nothing, and a model can claim completion
after a failed write. **The output on disk is the only evidence.**

```python
missing = expected - done
orphaned = succeeded - written          # NOT done - written
if missing or orphaned:
    print(f"incomplete: {len(missing)} unrun, {len(orphaned)} claimed-but-absent")
```

Reconcile with the same code path every time — a bespoke check per run is how
runs disagree with each other.

## 3. Pacing

Providers on this box, measured 2026-11-20, and what each one needs:

| Provider | Behaviour seen | Pace |
|---|---|---|
| `100milionseek` (default) | intermittent `502 Bad Gateway` | retry with backoff; ~1 req/s |
| `tokenlab` | `429 model_concurrency_rate_limit_exceeded` under load | back off on 429, do not hammer |
| `moss` | steady — the reliable choice for batch work | ~1 req/s is fine |
| `howtofix` | `503 model_not_found` | unusable, do not batch on it |
| `xingchi` (`sol`, `astra`) | `403 client_not_allowed` for scripted clients | **never** use for batch |

Rules:

- **Serial by default.** Concurrency multiplies 429s; it rarely multiplies
  throughput on a capacity-limited free tier.
- **Concurrency above 1 only after measuring** the provider holding steady at 1.
- **Backoff on the status, not on a timer**: 429 and 5xx retry, 4xx (except 408)
  does not — a 403 or 404 will never succeed on retry.
- **Jittered exponential backoff**, capped: `min(cap, base * 2**attempt) * (0.5 + random())`.
- **Never retry a 400/401/403.** It burns the budget and hides the real error.
- Sleep between units even on success (`0.5–1 s`): free tiers count requests per
  minute, and a tight loop converts a 500/hour budget into an hour of 429s.
- Log every retry with the status and the attempt number. A run that silently
  retried 400 times looks identical to a clean run in the output.

## 4. Per-call timeouts

**Every** external call gets a timeout. Three layers, because they fail differently:

```python
import subprocess, urllib.request

# HTTP: connect and read separately
urllib.request.urlopen(req, timeout=30)          # total socket timeout

# subprocess: kill the whole tree, not just the parent
subprocess.run(cmd, timeout=300, capture_output=True, text=True)

# the run as a whole
DEADLINE = time.time() + 6 * 3600
```

- **A timeout is a failed unit, not a failed run.** Record it in the checkpoint
  as `{"id": ..., "error": "timeout"}` and continue. Only abort the run on a
  repeated pattern (e.g. 5 timeouts in a row = the provider is down).
- **Kill process trees.** A `subprocess.run(timeout=)` kills the child; a child
  that spawned its own grandchildren can survive. For shell-outs use a process
  group and kill the group.
- **Bound the whole run too.** A per-call timeout with no global deadline means a
  run that takes 40 hours instead of failing at 6.
- **Set the timeout from the measured p99, not from optimism.** Measure 20 units
  first, then set the timeout at ~3x the slowest.
- Distinguish **timeout** from **error** in the checkpoint. They need different
  retry policies: a timeout is worth one retry at a longer timeout; a 400 is not.

## The loop, assembled

```python
for unit in units:
    if unit.id in done:
        continue
    if time.time() > DEADLINE:
        print(f"deadline hit: {len(expected - done)} units remain")
        break
    try:
        result = process(unit, timeout=UNIT_TIMEOUT)
        record(CKPT, unit.id, result)
    except TimeoutError:
        record(CKPT, unit.id, {"error": "timeout"})
        consecutive_timeouts += 1
        if consecutive_timeouts >= 5:
            raise SystemExit("provider looks down — stopping, checkpoint intact")
    except Retryable as exc:
        backoff(attempt); continue          # do not record; retry the same unit
    except Permanent as exc:
        record(CKPT, unit.id, {"error": str(exc)})   # record and move on
    time.sleep(PACE)
reconcile(expected, done_ids(CKPT), written_ids(OUT))
```

Note what is *not* recorded: a retryable failure. Recording it would mark the
unit done and the resume would skip it. Only outcomes are recorded.

### Resume policy per recorded status

The resume set is not "everything in the checkpoint" — that silently retires a
unit that only timed out:

| Recorded status | On resume | Why |
|---|---|---|
| `ok` | skip | done, and `written` proves it |
| `error` (permanent) | skip | it will fail again; report it |
| `timeout` | **retry once**, at a longer timeout | a hang is not a verdict |
| `error` + `retries: n` where `n < max` | retry | only if you track attempts |

```python
SKIP = {"ok", "error"}                       # terminal
retry = {uid for uid, r in done.items() if r.get("status") == "timeout"}
```

Cap the retries (`max_attempts`, default 2) or a unit that always times out
will re-run on every resume forever. Record `attempts` in the checkpoint so the
cap survives a restart.

## Reporting

- One line per unit at most; a batch of 900 must not produce 900 chat messages.
- Report on **transitions**: start, every N units, first failure, finish.
- Final report: `done / expected`, `failed`, `timed out`, `skipped`, wall time,
  and the path to the checkpoint. Numbers from the reconciliation, not from the
  loop counter — they disagree exactly when it matters.

## Pitfalls

- **A "resume" that restarts from zero** because the checkpoint path was built
  from a timestamp. Make the path deterministic per run id.
- **Treating the checkpoint as a success list.** It records failures too; compare
  `succeeded` against the output, or every recorded failure shows up as an orphan.
- **Counting attempts as completions.** Track `attempted` and `completed`
  separately or the summary lies.
- **Buffered stdout.** Under a pipe, prints are lost on a kill; flush, or write
  progress to the checkpoint file and read that.
- **Retrying a non-idempotent unit.** The retry duplicates the side effect.
- **One giant try/except around the loop** — it turns a per-unit failure into a
  whole-run abort.
- **Trusting the model's "done".** Verify the artifact exists and is non-empty.
