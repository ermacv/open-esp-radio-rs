# ESP32-S31 hardware-in-the-loop platform

This is a private embedded workspace for qualifying the public driver crates.
It is deliberately excluded from the root host workspace: normal
`cargo test --workspace --all-targets` must not build target-only binaries.

The authoritative performance profile is `psram-code-psram-data`. Its image
has two stages:

1. a Flash/SRAM bootstrap initializes and verifies external memory;
2. a separately linked runtime executes code and ordinary data from PSRAM,
   while its stack, DMA objects and interrupt closure remain in internal SRAM.

Board electrical settings, credentials, traffic generation and PASS/FAIL
reporting belong here. PHY/MAC/STA behavior belongs in `../../crates` and must
be moved there before a new behavior is entered in the canonical feature
ledger.

The closed vendor oracle is never a dependency of this workspace. It is an
explicitly excluded sibling workspace at `../vendor-oracle/esp32s31` and is
reachable only through `cargo hil oracle ...`.

Common commands from the repository root:

```text
cargo hil doctor
cargo hil build radio
cargo hil flash bidirectional --port /dev/ttyACM0
cargo hil traffic bidirectional <device-ip> --phy he20
cargo hil oracle verify
cargo hil oracle build
cargo hil oracle flash --port /dev/ttyACM0
```
