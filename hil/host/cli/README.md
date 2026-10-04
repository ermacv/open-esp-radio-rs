# oer-hil-cli

`cargo hil`. The workspace alias runs this package's binary
(`run --quiet -p oer-hil-cli --`); it is not part of xtask, so a change to the
repository checks never rebuilds it and a stand command builds only the HIL
host packages it uses.

The binary handles the stand's own commands and forwards every other one to
the runner (`oer-hil-runner`) it builds:

| Module | Owns |
| --- | --- |
| `command` | Dispatch, lease options (`--owner`), `lease`, `queue`, `owner`, `preempt`, `devices`, `wait`, `runs`, `perf`, `profile`, `firmware`, the merged `__command-tree`, and the launch of the runner with its receipt, job and evidence bookkeeping |
| `observer` | Building the runner from Cargo's artifact messages, its content-addressed copy under `target/hil/observers/` and the receipt; `cargo xtask hil-observer` calls it |
| `jobs` | Enqueued runs and experiments (`--enqueue`, `--after`), what a job is fixed with below `target/hil/jobs/cli/`, `wait JOB` |
| `runs`, `store` | The shared run store: list, why, compare, history, pin, prune |
| `evidence` | Pending evidence and its tracked shards (`evidence record`) |
| `perf`, `ab`, `bisect` | Gated measurements and baselines, A/B experiments, bisection |
| `board`, `flash`, `jtag`, `stand`, `fixtures`, `dashboard` | Board resets, consoles and soaks, the peer console, manual flashing, OpenOCD, stand discovery and doctor, host fixture probing, the live page |
| `firmware_catalog`, `vendor_firmware` | The ESP-IDF firmware catalog (`hil/peers`, `hil/bootloaders`, `verification/<chip>/hil-vendor`) and its builds against the pinned ESP-IDF; `cargo xtask vendor-firmware` calls the build |

Commands and flags are documented in the [host guide](../README.md), the
[stand guide](../stand.md) and the [runs guide](../runs.md); `cargo hil --help`
prints them.

## Processes

- A leading `--root PATH` names the checkout to act on; otherwise the binary
  acts on the checkout it was built from and refuses to run in another one.
  The installed `oer-stand` (`cargo xtask stand-install`) and a job's detached
  process pass it.
- The runner learns this binary's path from `OER_HIL_CLI`
  (`oer_hil_image::CLI_ENV`) and runs `firmware bootloader CHIP` to get a
  chip's catalog bootloader, which only this package builds. OpenOCD of the
  ESP-IDF tools reaches it through `oer_hil_arbiter::control::OPENOCD_ENV`.
- Cancellation is forwarded to the runner's process group with up to five
  minutes for cleanup; the exit code is the runner's (or `128 + signal`).

## Tests

```console
cargo test -p oer-hil-cli
```

The integration tests run the real binary against a fixture checkout whose
`cargo` and runner are shell scripts.
