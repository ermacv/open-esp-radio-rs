# HIL experiments

`oer-hil-experiment` launches runs and runs experiments over them.

| Module | Owns |
| --- | --- |
| `launch` | `launch_run(&Launch) -> Launched`, the one way a run is launched: the runner executable runs supervised, with cancellation forwarded and a five-minute shutdown grace, and its runs come back from its run receipt (`OER_HIL_RUN_RECEIPT`), never from the store's newest directories; `Runner::prepare` builds a checkout's runner with its observer receipt |
| `job` | The arbiter job a launch runs as: recorded, started after `--after`, finished with its runs' outcomes |
| `ab` | Two firmware variants on the same scenarios: arms in detached worktrees, source snapshots, rounds under whole-stand leases, the report |
| `bisect` | The first commit at which a scenario stops passing, a step's runner chosen by the revision's HIL wire; a step is judged by its scenario's outcome, a build failure making it broken |

Revisions are checked out with `oer_process::git::Worktree`, the one
worktree helper, which `cargo hil images compare` shares. Nothing here runs
`cargo hil`: a revision's own runner is built with `oer-hil-observer` and
launched directly.

```console
cargo test -p oer-hil-experiment
```
