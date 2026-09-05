# Repository tooling

Tools are grouped by the contract they own. Blobray and memory-report are
independent reusable tools; `repo` contains this repository's build and policy
checks. A utility does not need its own Cargo package.

| Path | Inputs and result |
| --- | --- |
| [blobray](blobray/README.md) | Generic compiled-binary analysis, reviewed models, comparison and register publication; callers select projects/providers |
| [memory-report](memory-report/README.md) | Generic ELF memory and stack analysis; the consumer supplies placement policy |
| [repo](repo/README.md) | Cargo/source/architecture checks and their regression tests |

The [qualification evaluator](../qualification/README.md) belongs to its
readiness domain. [HIL](../hil/README.md) owns hardware execution and fixtures;
[vendor projects](../verification/README.md) own investigation composition.
Neither producer decides product readiness. Register model/publication inputs
have a separate [source map](../registers/esp32s31/README.md).

The `blobray-run` limiter stays with Blobray and bounds actual analyses.
`cargo xtask check blobray-standalone` checks independent extraction; the
extracted tool has no dependency on repository orchestration.
Launcher tests live in `blobray/tests/launcher.rs`. The repository checks
invoke those tests without assuming ownership of the launcher implementation.
