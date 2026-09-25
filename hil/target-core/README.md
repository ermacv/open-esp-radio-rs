# HIL target core

`open-esp-radio-hil-target-core` holds the HIL target logic that does not
depend on the chip: transport workload pieces, memory-benchmark data
conditioning, console serialization, network-boundary counters and IPv4
policy, and the Bluetooth fixed-key Host policy, command pump and secure-GATT
evidence. The [ESP32-S31 runtime](../targets/esp32s31/README.md) composes these
modules with its radio, executor and console. Host tests exercise the same
compiled code the firmware links.

| Feature | Modules |
| --- | --- |
| none | `console`, `memory_benchmark`, `network::progress`, `traffic` |
| `bluetooth` | `bluetooth::{command_pump, security}` |
| `secure-gatt` | `bluetooth_gatt::secure` |
| `upstream-network` | `network::{checksum, ipv4, sockets}` on original Embassy and Xarxa |
| `embassy-network` | `network::{embassy_ipv4, sockets}` on released `embassy-net` |
| `owned-network` | `network::{embassy_ipv4, sockets}` on the owned Embassy and Xarxa fork |

Select at most one network feature, as the runtime does; the crate fails to
compile otherwise. Stack pins and features match the HIL target workspace, so
host tests observe the stack configuration the firmware uses. The chip MIC-fault
hooks and the HCI reply exchange stay in the runtime around
`bluetooth::security`.

Run the tests for each configuration:

```console
cargo test -p open-esp-radio-hil-target-core
cargo test -p open-esp-radio-hil-target-core --features secure-gatt
cargo test -p open-esp-radio-hil-target-core --features upstream-network
cargo test -p open-esp-radio-hil-target-core --features embassy-network
cargo test -p open-esp-radio-hil-target-core --features owned-network
```
