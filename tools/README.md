# Repository tooling

Tools are grouped by the contract they own. Blobray and memory-report are
independent reusable tools; `repo` contains this repository's build and policy
checks. A utility does not need its own Cargo package.

| Path | Inputs and result |
| --- | --- |
| [blobray](blobray/README.md) | Captured binary research, reviewed knowledge and bounded concrete comparison |
| [memory-report](memory-report/README.md) | Generic ELF memory and stack analysis; the consumer supplies placement policy |
| [process](process/README.md) | Host child-process ownership, cancellation and bounded cleanup shared by xtask and HIL |
| [firmware](firmware/README.md) | Firmware image operations and shared serial-device leases |
| [repo](repo/README.md) | Cargo/source/architecture checks and their regression tests |

The [qualification evaluator](../qualification/README.md) belongs to its
readiness domain. [HIL](../hil/README.md) owns hardware execution and fixtures;
[vendor projects](../verification/README.md) own investigation composition.
Neither producer decides product readiness. Register model/publication inputs
have a separate [source map](../registers/esp32s31/README.md).

Blobray uses its built-in Linux supervisor for analysis. `cargo xtask check
blobray-standalone` extracts and tests the shipping crate graph independently.
Register publication uses the [register tool](registers/README.md).
