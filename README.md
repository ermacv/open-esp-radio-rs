# open-esp-radio-rs

Source-only, `no_std` Embassy radio driver and verification workspace for
Espressif chips. The current implementation target is ESP32-S31 Wi-Fi STA,
bounded AP, same-channel STA+AP and standalone normalized monitor, with exact
Open or WPA2-Personal security selection on the implemented STA/AP source
paths. The source tree also contains typed ESP-NOW, legacy Wi-Fi power-save,
TWT, monitor hopping/injection admission, HE20 and advanced-rate boundaries;
their exact implemented and deliberate fail-closed limits are documented in
[`driver/README.md`](driver/README.md). These source capabilities are not
qualification or HIL claims. The ESP32-S31 IEEE 802.15.4 path currently
reaches source-reviewed digital clocks, private MAC reset, an interrupt-masked
static policy, and isolated DMA/IRQ semantic leaves plus a pure fail-closed MAC
control model. It now exposes serialized, route-detached polled ED and CCA
commands with exact selected `ED_DONE` recovery; their RSS result is explicitly
uncalibrated. PHY/RF qualification, active IRQ, RX/TX dataplanes and operational
MAC readiness remain incomplete. ESP32-C5 and on-air-qualified Bluetooth/BLE
support remain future work. The ESP32-S31 Bluetooth tree now also carries an explicit
[BLE feature frontier](driver/chips/esp32s31/bluetooth/FEATURES.md) and a
portable bounded asynchronous HCI transport. Its source-connected restricted
passive LE scanner accepts standard `bt-hci` commands and emits standard legacy
advertising reports; this is target-build and host-model evidence, not an
on-air readiness claim. The standalone Host-bootstrap table still rejects
Link-Layer operations when no chip Controller backend owns them.
Host tests also run the released Trouble Runner through the same raw Controller
endpoint and bootstrap state exposed to a future hardware session runner; this
is software integration evidence, not an ESP32-S31 task or hardware capability.
The normal workspace and HIL
do not link `esp-wifi-sys`, vendor Wi-Fi
archives, or a radio/Wi-Fi ROM ABI. The isolated vendor-oracle workspace is the
only opt-in exception.

## Repository layout

| Path | Purpose |
| --- | --- |
| [`driver/`](driver/README.md) | All shipping driver code and its architecture map |
| `driver/radio` | Public requests and typed radio/Wi-Fi lifecycle |
| `driver/bluetooth/hci` | Portable allocation-free async `bt-hci` transport, fail-closed Host bootstrap and affine Controller endpoints |
| `driver/memory` | Stable-memory proofs and affine buffer handoff |
| `driver/network/interface` | Stack-neutral interface, link and error values |
| `driver/ieee80211/` | Chip-independent Wi-Fi protocols and policy |
| `driver/ieee80211/softmac` | Executor-independent SoftMAC service, VIF and status contract |
| `driver/ieee80211/sta` | Chip/executor-independent STA MLME, scan/reconnect, beacon-loss and power-save policy |
| `driver/network/adapters/embassy/owned` | Internal persistent `embassy-net-driver` frame ownership |
| `driver/adapters/embassy/esp32s31/runtime` | ESP32-S31 Embassy executor/time platform binding |
| `driver/runtime/embassy/esp32s31/ieee80211` | Internal ESP32-S31 Wi-Fi Embassy implementation |
| `driver/runtime/embassy/esp32s31/bluetooth` | ESP32-S31 Bluetooth controller and session execution under Embassy |
| `driver/adapters/esp-hal/esp32s31/ieee80211` | ESP32-S31 `esp-hal` peripheral binding |
| `driver/integration/esp32s31/embassy/ieee80211` | Production station/AP/monitor composition and explicit ESP-NOW hooks |
| `driver/chips/esp32s31/pac/raw` | Internal generated svd2rust backend |
| `driver/chips/esp32s31/pac` | Closed typed radio peripheral-access API |
| `driver/chips/esp32s31/hal` | Finite hardware operations and async boundaries |
| `driver/chips/esp32s31/phy` | PHY initialization and calibration state machines |
| `driver/chips/esp32s31/ieee802154/dma` | Fixed-frame storage and RX-buffer ownership for the ESP32-S31 IEEE 802.15.4 MAC |
| `driver/chips/esp32s31/ieee802154/irq` | Quiesced IRQ masks, source identity and pure dispatch-order contract |
| `driver/chips/esp32s31/ieee802154/mac` | Pure fail-closed operation plans and sampled-event state transitions |
| `driver/chips/esp32s31/ieee802154/runtime` | Executor-neutral affine owner that executes typed MAC plans through a sealed hardware boundary |
| `driver/ieee802154` | Allocation-free, chip-independent IEEE 802.15.4 frame, command, event and capability contracts |
| `driver/chips/esp32s31/ieee80211/dma` | Audited ESP32-S31 descriptor, ring and DMA-storage leaf |
| `driver/chips/esp32s31/ieee80211/mac` | Safe ESP32-S31 Wi-Fi MAC backend, IRQ, RX/TX policy and rate control |
| `driver/chips/esp32s31/ieee80211` | Role-neutral ESP32-S31 Wi-Fi cold start and device composition |
| `driver/chips/esp32s31/ieee80211/sta` | Executor-independent ESP32-S31 station composition |
| [`hil/`](hil/README.md) | Hardware target/host infrastructure and typed HIL protocol |
| `hil/targets/esp32s31` | Test-only board, bootstrap, memory placement and end-to-end scenarios |
| `hil/targets/esp32s31/telemetry` | ESP32-S31 HIL counter and report implementations for production observation events |
| `hil/protocol` | Typed host/HIL command and telemetry protocol |
| `hil/host/runner` | Host build, flash, traffic and qualification scenario runner |
| `hil/host/linux-net` | Privileged Linux AP/monitor fixture used only by HIL |
| [`verification/`](verification/README.md) | Reusable chip knowledge and concrete vendor comparison projects |
| [`qualification/`](qualification/README.md) | Capability programs and independent readiness evaluator |
| `tools/blobray` | Blobray: compiled-binary analysis, reviewed models, publication and Rust verification |
| [`tools/`](tools/README.md) | Reusable analysis tools and repository policy checks under `tools/repo` |
| [`registers/`](registers/esp32s31/README.md) | Reviewed hardware models, PAC publication policy and generated SVD/bindings |

Applications own board startup, credentials, `embassy-net::Stack`, DHCP and
sockets. The driver returns an `embassy-net-driver::Driver`; its eternal runner
owns PAC, DMA and ISR state. Shared cross-chip code is extracted only after a
second backend demonstrates the same semantic operation.

See the canonical [driver architecture](driver/README.md), the
[machine-checked qualification specification](qualification/README.md), and the
[verification/qualification contract](docs/VERIFICATION_AND_QUALIFICATION.md).

## Verification

```console
cargo fmt --all -- --check
cargo test --workspace
cargo qualification validate --manifest qualification/targets/esp32s31/wifi-sta.toml
cargo blobray project configure \
  --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --check
# Artifact-backed: requires local.toml and current generated/findings.
cargo blobray project publish \
  --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --check
cargo test -p blobray-esp32s31 --test cli_contract -- --ignored
cargo xtask check source-only
(cd examples/esp32s31-station && cargo check --release)
```

All workspaces and generated PAC code use Rust edition 2024, Cargo resolver 3
and its formatting style. The current ESP32-S31 platform branch sets the
workspace MSRV to Rust 1.97.1. The repository toolchain is pinned to that
stable patch release so host, generated-code and embedded checks agree.
Repository automation lives in `tools/repo` and runs through `cargo xtask`;
see [its command map](tools/repo/README.md). Linux/OpenWrt fixture operations
remain under HIL.
The source-only audit additionally needs the stable embedded target and
the pinned Rust toolchain’s `llvm-tools` component for LLVM bitcode. It
validates generated PAC reproducibility, the compiled PHY artifact's
external symbols and its dependency tree. It deliberately does not inspect
Rust source text for required or forbidden function names.

Hardware workflows are documented in [the ESP32-S31 HIL README](hil/targets/esp32s31/README.md).
Current cross-layer readiness, dependencies and stale-HIL gaps are tracked by
the [machine-checked qualification specification](qualification/README.md). Stable
public API limits belong in [the driver architecture](driver/README.md), not a
second hand-maintained status matrix.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in the tracked repository.
