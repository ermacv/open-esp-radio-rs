---
name: hil-run
description: Use when running, queueing, waiting for or investigating hardware-in-the-loop (HIL) scenarios on the shared ESP32 stand in this repository — cargo hil run/run-all/wait/queue/plan, image builds, leases and the arbiter, scenario roles, failed runs, judging a run (`cargo qualification verdict`), or packing cited runs into the evidence archive.
---

# Run HIL scenarios on the shared stand

Read first (about 3k tokens): [sharing the stand](../../../hil/host/stand.md),
[enqueued runs](../../../hil/host/runs.md#enqueued-runs) and
[scenario roles](../../../hil/scenarios/README.md#roles). Lab setup, once per
host: [hardware route](../../../hil/README.md#hardware-route).

## Checklist

1. **Pick scenarios.** `cargo hil scenario list` or
   `cargo qualification plan --manifest <program> --archive DIR` (a fetched
   copy of the pinned evidence archive; without it every supported scenario
   reads as `run`); `cargo hil plan <scenario>`
   shows image, repetitions and named checks without hardware.
2. **Run in the background, through the arbiter.** Every hardware command
   queues for its boards, fixtures and air; never kill or signal another
   holder and never look for processes by name.
   - one run: `cargo hil run <scenario>` with `run_in_background: true`;
   - a job: `id=$(cargo hil run <scenario> --enqueue)` returns at once, then
     `cargo hil wait "$id"` in the background (exit 0 passed, 1 failed, …);
   - several scenarios: one `cargo hil run a b c`, one lease. In zsh pass
     the names literally or as an array, not as one unquoted `$VAR`.
3. **Debug quickly.** `--repetitions N` (1–20) for a look. Images build before the run queues, as image
   bundles of the one pipeline (`oer-image`); `cargo hil image build <class>`
   builds one without the stand, and `cargo fw flash --device MAC|PORT <bundle>`
   writes a bundle by hand (device lock, receipted write, start).
4. **Untracked sources** must be named with `--source-include PATH` or
   `--include-untracked`; the runner refuses to guess.
5. **Investigate.** `cargo hil runs why <run-id>` and `cargo hil runs show
   <run-id>` name the failure and artifact paths; read those files, never an
   observer JSON or the whole bundle. A repetition's typed results are its
   `observations.json`
   ([find and compare runs](../../../hil/host/runs.md#find-and-compare-runs)).
6. **Stand state.** `cargo stand queue` shows holders, jobs and boards; wait
   for a board in service with `cargo stand wait --service`. A flash made
   outside `cargo hil` is journaled with `cargo stand lease --board B --flashed
   IMAGE --application FILE --device B -- COMMAND`.
7. **Verdict and evidence.** `cargo qualification verdict --run PATH` judges
   a run against every program's HIL requirements; it is not published and
   not evidence. Evidence is a run a decision cites, packed into the evidence
   archive (`cargo hil archive ref|pack|publish`; `unpin` drops the pins of a
   selection or a version never published) and pinned in
   `evidence/archive.toml` in `qualification/`; `pack`, `publish` and
   `fetch` reach the network. Qualification reads only the pinned version:
   a run outside it is no evidence, whatever it shows.

## Roles

A scenario referenced by a program is `qualification`; every other one is
`investigation` and never satisfies a requirement. Diagnostic images
(`diagnostic-*`) and profiling scenarios are always `investigation`.
