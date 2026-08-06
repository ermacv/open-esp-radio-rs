# open-esp-radio-rs

Source-only, `no_std` radio research and driver workspace for Espressif chips.
The current implementation target is the ESP32-S31 Wi-Fi station path; the
qualification ledger distinguishes historical hardware results from qualification
of the current tree. Future chips and Bluetooth/BLE, IEEE 802.15.4 and
coexistence work are part of the intended scope. The normal workspace and HIL
do not link `esp-wifi-sys`, vendor Wi-Fi
archives, or a radio/Wi-Fi ROM ABI. The isolated vendor-oracle workspace is the
only opt-in exception.

## Repository layout

| Path | Purpose |
| --- | --- |
| [`driver/`](driver/README.md) | All shipping driver code and its architecture map |
| `driver/radio` | Feature-selecting `open-esp-radio` facade and re-exports |
| `driver/common/dma` | Shared audited DMA ownership primitives |
| `driver/wifi/` | Chip-independent Wi-Fi protocols and policy |
| `driver/wifi/softmac` | Executor-independent SoftMAC service, VIF and status contract |
| `driver/wifi/sta` | Chip/executor-independent STA MLME, scan/reconnect, beacon-loss and power-save policy |
| `driver/adapters/` | Reusable network, runtime and ecosystem adapters |
| `driver/adapters/embassy-net` | Executor-neutral `embassy-net-driver` frame ownership |
| `driver/adapters/embassy/esp32s31-platform` | ESP32-S31 Embassy executor/time platform binding |
| `driver/adapters/embassy/esp32s31-wifi` | ESP32-S31 Wi-Fi/Embassy runtime composition |
| `driver/adapters/esp-hal/esp32s31-wifi` | Optional `esp-hal` Wi-Fi singleton adapter |
| `driver/chips/esp32s31/pac` | Generated peripheral-access crate |
| `driver/chips/esp32s31/registers` | Handwritten typed radio register transactions |
| `driver/chips/esp32s31/hal` | Finite hardware operations and async boundaries |
| `driver/chips/esp32s31/phy` | PHY initialization and calibration state machines |
| `driver/chips/esp32s31/wifi/dma` | Audited ESP32-S31 descriptor, ring and DMA-storage leaf |
| `driver/chips/esp32s31/wifi/mac` | Safe ESP32-S31 Wi-Fi MAC backend, IRQ, RX/TX policy and rate control |
| `driver/chips/esp32s31/wifi/sta` | Executor-independent ESP32-S31 station composition |
| [`hil/`](hil/README.md) | Hardware target/host infrastructure and typed HIL protocol |
| `hil/targets/esp32s31` | Test-only board, bootstrap, memory placement and end-to-end scenarios |
| `hil/targets/esp32s31/telemetry` | ESP32-S31 HIL counter and report implementations for production observation events |
| `hil/protocol` | Typed host/HIL command and telemetry protocol |
| `hil/host/runner` | Host build, flash, traffic and qualification scenario runner |
| `hil/host/linux-net` | Privileged Linux AP/monitor fixture used only by HIL |
| [`verification/`](verification/README.md) | Vendor comparison target packs and checked verification inputs |
| [`qualification/`](qualification/README.md) | Machine-readable readiness claims |
| `tools/vendor-code-validator` | Compiled vendor/Rust analysis, reference generation and verification workflows |
| [`tools/`](tools/README.md) | Qualification checker, PAC generator and repository policy audits |
| `svd` | Editable ESP32-S31 radio register description |

Chip package names follow `open-esp-radio-<chip>-<layer>`; protocol-specific
hardware inserts the protocol before the layer, as in
`open-esp-radio-esp32s31-wifi-mac`.

The core workspace does not own board startup, PSRAM/flash placement or a
network executor. Reusable adapters live under `driver/adapters/`; concrete
board policy and the real `embassy-net`/smoltcp test application live under
`hil/`. The source tree remains chip-first (`driver/chips/esp32s31/phy`) so one chip's
PAC, radio PHY and protocol backends evolve together. A cross-chip PHY core
will be extracted only after another backend establishes a concrete shared API.

See [the architecture guide](docs/ARCHITECTURE.md) for dependency direction
and ownership boundaries, [the naming and layout contract](docs/NAMING_AND_LAYOUT.md)
for canonical terminology and the target tree, and [the documentation index](docs/README.md)
for current status, reference material and archived migration reports.

## Verification

```console
cargo fmt --all -- --check
cargo test --workspace
cargo qualification check --manifest qualification/targets/esp32s31/wifi-sta.ledger
cargo vendor-code-validator registers validate \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --deny-unreviewed
cargo vendor-code-validator registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --check --deny-unreviewed
tools/audit-source-only.sh
(cd examples/esp32s31-station && cargo check --release)
```

All workspaces and generated PAC code use Rust edition 2024, Cargo resolver 3
and its formatting style. The current ESP32-S31 platform branch sets the
workspace MSRV to Rust 1.97.1. The repository toolchain is pinned to that
stable patch release so host, generated-code and embedded checks agree.
The source-only audit additionally needs the stable embedded target and
`llvm-nm`. It validates generated PAC reproducibility, the compiled PHY artifact's
external symbols and its dependency tree. It deliberately does not inspect
Rust source text for required or forbidden function names.

Hardware workflows are documented in [the ESP32-S31 HIL README](hil/targets/esp32s31/README.md).
Current cross-layer readiness and stale-HIL gaps are tracked by the
[machine-checked qualification ledger](qualification/README.md); the detailed PHY
and MAC matrix remains in [the feature status](docs/ESP32S31_WIFI_FEATURE_STATUS.md).

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in the tracked repository.
