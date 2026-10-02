---
name: hil-run
description: Use when running, queueing, waiting for or investigating hardware-in-the-loop (HIL) scenarios on the shared ESP32 stand in this repository — cargo hil run/run-all/wait/queue/plan, image builds, leases and the arbiter, scenario roles, failed runs, or recording HIL evidence shards.
---

# Run HIL scenarios on the shared stand

Read first (about 3k tokens): [sharing the stand](../../../hil/host/stand.md),
[enqueued runs](../../../hil/host/runs.md#enqueued-runs) and
[scenario roles](../../../hil/scenarios/README.md#roles). Lab setup, once per
host: [hardware route](../../../hil/README.md#hardware-route).

## Checklist

1. **Pick scenarios.** `cargo hil scenario list` or
   `cargo qualification plan --manifest <program>`; `cargo hil plan <scenario>`
   shows image, repetitions and named checks without hardware.
2. **Run in the background, through the arbiter.** Every hardware command
   queues for its boards, fixtures and air; never kill or signal another
   holder and never look for processes by name.
   - one run: `cargo hil run <scenario>` with `run_in_background: true`;
   - a job: `id=$(cargo hil run <scenario> --enqueue)` returns at once, then
     `cargo hil wait "$id"` in the background (exit 0 passed, 1 failed, …);
   - several scenarios: one `cargo hil run a b c`, one lease. In zsh pass
     the names literally or as an array, not as one unquoted `$VAR`.
3. **Debug quickly.** `--repetitions N` (1–20) for a look; such runs are
   never pending evidence. Images build before the run queues.
4. **Untracked sources** must be named with `--source-include PATH` or
   `--include-untracked`; the runner refuses to guess.
5. **Investigate.** `cargo hil runs why <run-id>` and `cargo hil runs show
   <run-id>` name the failure and artifact paths; read those files, never an
   observer JSON or the whole bundle
   ([find and compare runs](../../../hil/host/runs.md#find-and-compare-runs)).
6. **Stand state.** `cargo hil queue` shows holders and boards; wait for a
   board in service with `cargo hil wait --service`.
7. **Evidence (optional).** From a clean tree, `cargo hil evidence record`
   writes shards to `hil/evidence/<chip>/`; commit them with the change they
   qualify. Stale evidence is information; qualification runs on a baseline
   the user chooses.

## Roles

A scenario referenced by a program is `qualification`; every other one is
`investigation` and never satisfies a requirement. Diagnostic images
(`diagnostic-*`) and profiling scenarios are always `investigation`.
