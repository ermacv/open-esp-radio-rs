# oer-hil-cli

`cargo hil`. The workspace alias runs this package's binary
(`run --quiet -p oer-hil-cli --`); it is not part of xtask, so a change to the
repository checks never rebuilds it and a stand command builds only the HIL
host packages it uses.

The binary handles the stand's own commands and forwards every other one to
the runner (`oer-hil-runner`), which `oer-hil-observer` builds and
`oer-hil-experiment` launches:

| Module | Owns |
| --- | --- |
| `command` | `HilCli`, the one clap definition of the stand's commands (parsed, dispatched by an exhaustive match and walked for `__command-tree`; any other command is the runner's, forwarded with the lease and job options the stand takes from it, `jobs::RunnerOptions`), dispatch, lease options (`--owner`), `lease`, `queue`, `owner`, `preempt`, `devices`, `wait`, `runs` and `perf` (calls into `oer-hil-analysis`), `profile`, `firmware`, the merged `__command-tree`, and `run`/`run-all`: a job, a launch through `oer-hil-experiment`'s `launch_run`, pending evidence and automatic pruning |
| `jobs` | The command line of jobs, whose records and tickets the arbiter owns (`oer_hil_arbiter::jobs`): `--enqueue` and `--after`, what a job is fixed with below `target/hil/jobs/cli/`, its detached process, `wait JOB` |
| `evidence` | `cargo hil evidence pending` and `dismiss`: the checkout's pending evidence (`oer_hil_run_bundle::store::pending`); `cargo qualification hil-evidence --pending` records it |
| `experiments` | The arguments of `ab` and `bisect`, run by `oer-hil-experiment` |
| `board`, `flash`, `stand`, `dashboard` | Arguments and calls only: board resets, consoles and soaks and the peer console through board I/O (`oer-hil-board`), manual flashing through the flash operation (`oer-hil-flash`), stand discovery, doctor and fixture probes through `oer-hil-stand-host`; the live page |
| `firmware_catalog` | `cargo hil firmware`: lists and builds the ESP-IDF firmware catalog (`hil/peers`, `hil/bootloaders`, `verification/<chip>/hil-vendor`), which `oer-image`'s `esp_idf::catalog` builds against the pinned ESP-IDF, and flashes it through `oer_hil_flash::catalog` |

Commands and flags are documented in the [host guide](../README.md), the
[stand guide](../stand.md) and the [runs guide](../runs.md); `cargo hil --help`
prints them.

## Processes

- A leading `--root PATH` names the checkout to act on; otherwise the binary
  acts on the checkout it was built from and refuses to run in another one.
  The installed `oer-stand` (`cargo xtask stand-install`) and a job's detached
  process pass it.
- The runner locates OpenOCD of the ESP-IDF tools itself
  (`oer_hil_board::openocd::Openocd::locate`) and builds a chip's catalog
  bootloader itself, through the image pipeline. A run that is a job carries
  the job's id to the runner, so the runner's requests are the job's tickets.
- Cancellation is forwarded to the runner's process group with up to five
  minutes for cleanup; the exit code is the runner's (or `128 + signal`).

## Tests

```console
cargo test -p oer-hil-cli
```

The integration tests run the real binary against a fixture checkout whose
`cargo` and runner are shell scripts.
