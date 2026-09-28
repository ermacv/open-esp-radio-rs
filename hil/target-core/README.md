# HIL target core

`oer-hil-target-core` holds the HIL target logic that does not
depend on the chip: transport workload pieces, memory-benchmark data
conditioning, console serialization, network-boundary counters and IPv4
policy, the reset-retained post-mortem record, task liveness slots for the
hang watchdog, the platform's own trace events and the secure-GATT evidence. The [ESP32-S31 runtime](../targets/esp32s31/README.md) composes these
modules with its radio, executor and console. Host tests exercise the same
compiled code the firmware links.

| Feature | Modules |
| --- | --- |
| none | `console`, `liveness`, `memory_benchmark`, `network::progress`, `postmortem`, `trace`, `traffic` |
| `secure-gatt` | `bluetooth_gatt::secure` |
| `owned-network` | `network::{embassy_ipv4, sockets}` on the owned Embassy and Xarxa fork |

Stack pins and features match the HIL target workspace, so
host tests observe the stack configuration the firmware uses.

Run the tests for each configuration:

```console
cargo test -p oer-hil-target-core
cargo test -p oer-hil-target-core --features secure-gatt
cargo test -p oer-hil-target-core --features owned-network
```
