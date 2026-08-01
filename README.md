# open-esp-radio-rs

Source-only, `no_std` radio research and driver workspace for Espressif chips.
The current implementation target is the ESP32-S31 Wi-Fi station path; the
capability ledger distinguishes historical hardware results from qualification
of the current tree. Future chips and Bluetooth/BLE, IEEE 802.15.4 and
coexistence work are part of the intended scope. The normal workspace and HIL
do not link `esp-wifi-sys`, vendor Wi-Fi
archives, or a radio/Wi-Fi ROM ABI. The isolated vendor-oracle workspace is the
only opt-in exception.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/radio` | Application-facing `open-esp-radio` facade |
| `crates/wifi/` | Chip-independent Wi-Fi protocols |
| `crates/integration/` | Reusable network, runtime and ecosystem adapters |
| `crates/integration/esp32s31/wifi-embassy` | ESP32-S31 Wi-Fi/Embassy runtime composition |
| `crates/integration/esp32s31/wifi-esp-hal` | Optional `esp-hal` Wi-Fi singleton adapter |
| `crates/esp32s31/svd` | Generated register-access crate |
| `crates/esp32s31/pac` | Radio register ownership and transactions |
| `crates/esp32s31/hal` | Finite hardware operations and async boundaries |
| `crates/esp32s31/phy` | PHY initialization and calibration state machines |
| `crates/esp32s31/wifi/mac` | ESP32-S31 Wi-Fi MAC, RX/TX and rate control |
| `hil/esp32s31` | Test-only board, bootstrap, memory placement and end-to-end scenarios |
| `tools` | Capability/PHY validators, PAC generator, HIL runner and source-only artifact audit |
| `svd` | Editable ESP32-S31 radio register description |

Chip-wide packages follow `open-esp-radio-esp32s31-<layer>`. Protocol-specific
hardware adds the protocol before its layer, for example
`open-esp-radio-esp32s31-wifi-mac`. Directory names stay short because their
hierarchy already supplies the project, chip and protocol context.

The core workspace does not own board startup, PSRAM/flash placement or a
network executor. Reusable adapters live under `crates/integration/`; concrete
board policy and the real `embassy-net`/smoltcp test application live under
`hil/`. The source tree remains chip-first (`crates/esp32s31/phy`) so one chip's
PAC, radio PHY and protocol backends evolve together. A cross-chip PHY core
will be extracted only after another backend establishes a concrete shared API.

See [the architecture guide](docs/ARCHITECTURE.md) for dependency direction
and ownership boundaries, and [the documentation index](docs/README.md) for
current status, reference material and archived migration reports.

## Verification

```console
cargo fmt --all -- --check
cargo test --workspace
cargo capability-ledger check --manifest capabilities/esp32s31-wifi-sta.ledger
cargo pac-gen --check
tools/audit-source-only.sh
```

All workspaces and generated PAC code use Rust edition 2024, Cargo resolver 3
and its formatting style. The current ESP32-S31 platform branch sets the
workspace MSRV to Rust 1.97.1. The repository toolchain is pinned to that
stable patch release so host, generated-code and embedded checks agree.
The last command additionally needs the stable embedded target and `llvm-nm`.
It validates generated PAC reproducibility, the compiled PHY artifact's
external symbols and its dependency tree. It deliberately does not inspect
Rust source text for required or forbidden function names.

Hardware workflows are documented in [the ESP32-S31 HIL README](hil/esp32s31/README.md).
Current cross-layer readiness and stale-HIL gaps are tracked by the
[machine-checked capability ledger](capabilities/README.md); the detailed PHY
and MAC matrix remains in [the feature status](docs/ESP32S31_WIFI_FEATURE_STATUS.md).

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in the tracked repository.
