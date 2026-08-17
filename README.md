# open-esp-radio-rs

Source-only, `no_std` Embassy radio driver and verification workspace for
Espressif chips. The current implementation target is ESP32-S31 Wi-Fi STA,
single-client WPA2 AP and standalone normalized monitor. AP+STA, ESP32-C5,
Bluetooth/BLE, IEEE 802.15.4 and coexistence are future work, not placeholder
public APIs. The normal workspace and HIL
do not link `esp-wifi-sys`, vendor Wi-Fi
archives, or a radio/Wi-Fi ROM ABI. The isolated vendor-oracle workspace is the
only opt-in exception.

## Repository layout

| Path | Purpose |
| --- | --- |
| [`driver/`](driver/README.md) | All shipping driver code and its architecture map |
| `driver/radio` | Public requests and typed radio/Wi-Fi lifecycle |
| `driver/common/dma` | Shared audited DMA ownership primitives |
| `driver/wifi/` | Chip-independent Wi-Fi protocols and policy |
| `driver/wifi/softmac` | Executor-independent SoftMAC service, VIF and status contract |
| `driver/wifi/sta` | Chip/executor-independent STA MLME, scan/reconnect, beacon-loss and power-save policy |
| `driver/adapters/embassy-net` | Internal persistent `embassy-net-driver` frame ownership |
| `driver/adapters/embassy/esp32s31-platform` | ESP32-S31 Embassy executor/time platform binding |
| `driver/adapters/embassy/esp32s31-wifi` | Internal ESP32-S31 Wi-Fi Embassy implementation |
| `driver/adapters/esp-hal/esp32s31-wifi` | ESP32-S31 `esp-hal` peripheral binding |
| `driver/integration/esp32s31/embassy-wifi` | Production station/AP/monitor composition |
| `driver/chips/esp32s31/pac-raw` | Internal generated svd2rust backend |
| `driver/chips/esp32s31/pac` | Closed typed radio peripheral-access API |
| `driver/chips/esp32s31/hal` | Finite hardware operations and async boundaries |
| `driver/chips/esp32s31/phy` | PHY initialization and calibration state machines |
| `driver/chips/esp32s31/wifi/dma` | Audited ESP32-S31 descriptor, ring and DMA-storage leaf |
| `driver/chips/esp32s31/wifi/mac` | Safe ESP32-S31 Wi-Fi MAC backend, IRQ, RX/TX policy and rate control |
| `driver/chips/esp32s31/wifi` | Role-neutral ESP32-S31 Wi-Fi cold start and device composition |
| `driver/chips/esp32s31/wifi/sta` | Executor-independent ESP32-S31 station composition |
| [`hil/`](hil/README.md) | Hardware target/host infrastructure and typed HIL protocol |
| `hil/targets/esp32s31` | Test-only board, bootstrap, memory placement and end-to-end scenarios |
| `hil/targets/esp32s31/telemetry` | ESP32-S31 HIL counter and report implementations for production observation events |
| `hil/protocol` | Typed host/HIL command and telemetry protocol |
| `hil/host/runner` | Host build, flash, traffic and qualification scenario runner |
| `hil/host/linux-net` | Privileged Linux AP/monitor fixture used only by HIL |
| [`verification/`](verification/README.md) | Vendor comparison target packs and checked verification inputs |
| [`qualification/`](qualification/README.md) | Machine-readable readiness claims |
| `tools/blobray` | Blobray: compiled-binary analysis, reviewed models, publication and Rust verification |
| [`tools/`](tools/README.md) | Qualification checker, register model, Blobray and repository policy audits |
| `svd` | Published clean ESP32-S31 hardware descriptions and PAC binding indices |

Applications own board startup, credentials, `embassy-net::Stack`, DHCP and
sockets. The driver returns an `embassy-net-driver::Driver`; its eternal runner
owns PAC, DMA and ISR state. Shared cross-chip code is extracted only after a
second backend demonstrates the same semantic operation.

See the canonical [driver architecture](driver/README.md), the
[machine-checked qualification ledger](qualification/README.md), and the
[verification/qualification contract](docs/VERIFICATION_AND_QUALIFICATION.md).

## Verification

```console
cargo fmt --all -- --check
cargo test --workspace
cargo qualification check --manifest qualification/targets/esp32s31/wifi-sta.ledger
cargo blobray project configure \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
cargo blobray project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
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
Current cross-layer readiness, dependencies and stale-HIL gaps are tracked by
the [machine-checked qualification ledger](qualification/README.md). Stable
public API limits belong in [the driver architecture](driver/README.md), not a
second hand-maintained status matrix.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in the tracked repository.
