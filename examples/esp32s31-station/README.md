# ESP32-S31 station firmware

This is the production-shaped, non-HIL Embassy application for the open radio
driver. It uses the ESP32-S31 PAC-backed PHY/MAC implementation and must not
depend on the HIL protocol, qualification telemetry or benchmark policy.

The application is intentionally a separate target workspace because it uses
the embedded RISC-V standard-library build and ESP-IDF-compatible linker
layout. Run Cargo from this directory so its target and linker configuration
is applied:

```console
cd examples/esp32s31-station
cargo check --release
```

The default build uses the pinned Git Embassy/Xarxa integration with explicit
packet pools. The same application source can be checked against released
Embassy/smoltcp network crates and the token-based compatibility adapter:

```console
cargo check --release --no-default-features --features compat-network
```

The original upstream Xarxa/Embassy integration is selected explicitly:

```console
cargo check --release --no-default-features --features upstream-network
```

The three network integrations are compile-time alternatives and cannot be
combined in one binary. All retain the pinned `esp-hal` and `esp-pacs`
hardware forks. Their stack-facing APIs and source guarantees are described
in the [network integration contract](../../docs/wifi-egress.md#network-dependency-contracts).

Network credentials are application build configuration. They are deliberately
absent from reusable driver crates and HIL configuration:

```console
ESP32S31_WIFI_SSID='your ssid' \
ESP32S31_WIFI_PASSPHRASE='your passphrase' \
cargo check --release
```

After scan, WPA2 and DHCP complete, the upstream variant exposes a UDP echo
service on port 4321 and replies to ICMP ping. The owned and released-compat
variants currently run their DHCP/network tasks without an echo workload.
Application socket storage and IP policy live in `src/network.rs`.

If no matching AP is present, each complete 13-channel cold scan returns its
halted RX ring, waits 500 ms and prepares that same owner for the next scan.
It does not recreate descriptors or panic after the first `NoCandidate` pass.

Build the complete application from the repository root:

```console
cargo xtask build firmware station
cargo xtask build firmware station --flash --monitor --port /dev/ttyACM0
cargo xtask build firmware station --no-default-features --features upstream-network
```

The [shared platform](../../platform/esp32s31/README.md) initializes PSRAM,
relocates the separately linked application and keeps DMA and interrupt storage
in SRAM. The command checks ELF placement and stack frames before packaging
or flashing. `cargo build` in this example produces only the stage-two ELF;
flash the complete image through `xtask`. Hardware readiness still requires
appropriate scenario evidence.

The upstream variant pins original Embassy and the exact Xarxa revision
selected by that Embassy manifest. Its only upstream Cargo source mapping
selects official `embassy-time-driver` 0.2.2 from crates.io to share one timer
ABI with the platform. No network implementation is forked or edited. See
the [upstream contract](../../docs/wifi-egress.md#original-upstream-integration)
for pool exhaustion, RX scheduling and ownership limits.

The connected radio is a finite lifecycle epoch rather than a terminal task.
Peer loss or an application controller request returns the IRQ, staged-RX,
DMA, TX, sequence and CCMP-key owners, while the Embassy network stack and
sockets stay alive across link down/up. Before the next association, a finite
13-channel running scan temporarily owns only the quiesced hardware, stopped
RX and ordinary TX descriptor. It returns every owner and a fresh `ScanRecord`
before the stopped RX resources are split for reconnect. A fresh `ScanRecord`
can select another BSSID or channel. Product readiness is determined by the
[qualification program](../../qualification/targets/esp32s31/wifi-sta.toml) and
current evidence, independently of this application's source check.

## Network implementation

Use the same explicit choice as HIL when building from the repository root:

```console
cargo xtask build firmware station --network upstream
cargo xtask build firmware station --network udp-backpressure
```

Both choices use the original Embassy wrapper and the upstream driver contract.
The [UDP backpressure composition](../../driver/network/adapters/xarxa/backpressure/README.md)
changes only the pinned Xarxa stack. The builder checks dependency pins, archives
the effective locks and restores the tracked upstream catalog. Without
`--network`, the example retains its `owned-network` default. Direct Cargo builds
can select `--no-default-features --features upstream-network` for the control.
