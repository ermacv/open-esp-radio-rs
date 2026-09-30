# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: transport workload pieces, memory-benchmark data
conditioning, console serialization, network-boundary counters and IPv4
policy, the reset-retained post-mortem record, task liveness slots for the
hang watchdog, the platform's own trace events and the secure-GATT evidence. The [esp32s31 HIL agent](../targets/esp32s31/README.md) composes these
modules with its radio, executor and console. Host tests exercise the same
compiled code the firmware links.

| Feature | Modules |
| --- | --- |
| none | `console`, `liveness`, `postmortem`, `profile`, `trace` |
| `wifi` (default) | `network::progress`, `traffic` |
| `system` (default) | `memory_benchmark` |
| `secure-gatt` | `bluetooth_gatt::secure`, with the protocol's `bluetooth` family |
| `owned-network` | `network::{embassy_ipv4, sockets}` on the owned Embassy and Xarxa fork |

Each feature enables the matching radio family of the HIL protocol, so an
image that selects only its own families compiles only their protocol
modules.

Stack pins and features match the HIL target workspace, so
host tests observe the stack configuration the firmware uses.

Run the tests for each configuration:

```console
cargo test -p oer-hil-agent
cargo test -p oer-hil-agent --features secure-gatt
cargo test -p oer-hil-agent --features owned-network
```
