# hil/

Runs compiled production code on attached boards and records sealed evidence;
it never decides readiness. Routes for every task: [README](README.md#choose-a-route).

| Directory | Owns |
| --- | --- |
| `protocol/`, `schema/`, `observer/` | Host/target wire protocol; the evidence contract shared with qualification; the observer's build identity |
| `scenarios/` | Versioned scenario documents and their [roles](scenarios/README.md#roles) |
| `host/` | `cargo hil` (`cli/`), runner, the flash operation, image builder, families, fixtures ([host guide](host/README.md)) |
| `../stand/` | `cargo stand` and the stand packages: stand file, boards, arbiter, owners, journal, power, host files ([stand guide](host/stand.md), [stand file](../stand/file/README.md)) |
| `targets/<chip>/`, `agent/` | Target firmware workspaces; chip-independent agent logic |
| `evidence/<chip>/` | Tracked evidence shards (`cargo qualification hil-evidence`) |
| `evidence-shard/` | Shared vendor and HIL evidence shard format, validation, currency and reader/writer (root workspace) |
| `phy-calibration-capture/` | Chip-neutral PHY comparison observations and vendor console protocol (root workspace); chip comparisons live in `../crates/hardware/<chip>/calibration/` |

## Rules

- The stand is shared by every checkout: hardware commands queue in the
  [arbiter](host/stand.md). Never kill another holder; wait with
  `cargo hil wait JOB|RUN` or `cargo stand wait --service`, in the background.
- A scenario's `role` is derived from the programs: `qualification` when a
  program references it, otherwise `investigation`. Do not set it by hand
  against the programs.
- Runs write only below `target/hil/` and the shared run store, never tracked
  files. Recording evidence is optional; stale evidence is information, and
  qualification runs on a baseline the user chooses.
- Never read observer JSON (`observers/*.json`, megabytes on one line) or run
  bundles whole: `cargo hil runs show <run-id>` and `cargo hil runs why <run-id>`
  point at the artifacts that matter ([runs guide](host/runs.md#find-and-compare-runs)).
- A hardware-facing change runs the scenarios it needs to be trusted.

## Commands (in the background)

- `cargo hil scenario validate` and `cargo hil plan <scenario>`: no hardware.
- `cargo hil run <scenario> --enqueue` (returns a job id) then `cargo hil wait <job>`.
- `cargo stand queue`: holders, queue and board state.

Workflow: the `hil-run` skill.
