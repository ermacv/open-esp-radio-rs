# open-esp-radio-rs

Source-only, `no_std` Wi-Fi radio stack for Espressif chips, currently focused
on ESP32-S31. The normal workspace and HIL do not link `esp-wifi-sys`, vendor
Wi-Fi archives, or a radio/Wi-Fi ROM ABI. The isolated vendor-oracle workspace
is the only opt-in exception.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/radio` | Application-facing `open-esp-radio` facade |
| `crates/wifi/` | Chip-independent Wi-Fi protocols and network adapters |
| `crates/esp32s31/svd` | Generated register-access crate |
| `crates/esp32s31/pac` | Radio register ownership and transactions |
| `crates/esp32s31/hal` | Finite hardware operations and async boundaries |
| `crates/esp32s31/phy` | PHY initialization and calibration state machines |
| `crates/esp32s31/wifi/mac` | ESP32-S31 Wi-Fi MAC, RX/TX and rate control |
| `crates/esp32s31/wifi/esp-hal` | Optional `esp-hal` Wi-Fi singleton adapter |
| `hil/esp32s31` | Board, bootstrap and end-to-end hardware tests |
| `tools` | PAC generator, HIL runner and source-only artifact audit |
| `svd` | Editable ESP32-S31 radio register description |

Chip-wide packages follow `open-esp-radio-esp32s31-<layer>`. Protocol-specific
hardware adds the protocol before its layer, for example
`open-esp-radio-esp32s31-wifi-mac`. Directory names stay short because their
hierarchy already supplies the project, chip and protocol context.

See [the architecture guide](docs/ARCHITECTURE.md) for dependency direction
and ownership boundaries, and [the documentation index](docs/README.md) for
current status, reference material and archived migration reports.

## Verification

```console
cargo fmt --all -- --check
cargo test --workspace
cargo pac-gen --check
tools/audit-source-only.sh
```

All workspaces and generated PAC code use Rust edition 2024 and its formatting
style.
The last command additionally needs the stable embedded target and `llvm-nm`.
It validates generated PAC reproducibility, the compiled PHY artifact's
external symbols and its dependency tree. It deliberately does not inspect
Rust source text for required or forbidden function names.

Hardware workflows are documented in [the ESP32-S31 HIL README](hil/esp32s31/README.md).
Current qualified capabilities and their evidence are tracked in
[the feature status](docs/ESP32S31_WIFI_FEATURE_STATUS.md).

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in the tracked repository.
