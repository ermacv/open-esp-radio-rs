# ESP32-S31 station firmware

This is the production-shaped, non-HIL Embassy application for the open radio
driver. It uses the ESP32-S31 PAC-backed PHY/MAC implementation and must not
depend on the HIL protocol, qualification telemetry or benchmark policy.

The application is intentionally a separate target workspace because it uses
the embedded RISC-V standard-library build and ESP-IDF-compatible linker
layout. Run Cargo from this directory so its target and linker configuration
is applied:

```console
cd examples/esp32s31/station
cargo check --release
```

## Network selection

The example uses the owned Xarxa/Embassy stack (`owned-network`), the only
network implementation; the
[implementation guide](../../../docs/network-implementations.md) explains its
crates and the shared Wi-Fi boundary. Build a complete image from the
repository root:

```console
cargo xtask build firmware station
```

## Application behavior

Network credentials are application build configuration. They are deliberately
absent from reusable driver crates and HIL configuration:

```console
ESP32S31_WIFI_SSID='your ssid' \
ESP32S31_WIFI_PASSPHRASE='your passphrase' \
cargo check --release
```

After scan, WPA2 and DHCP complete, all four selections expose the same UDP
echo service on port 4321 and reply to ICMP ping. Released Embassy uses explicit
static UDP byte rings; Xarxa uses packet pools.
Application socket storage and IP policy live in `src/network.rs`.

If no matching AP is present, each complete 13-channel cold scan returns its
halted RX ring, waits 500 ms and prepares that same owner for the next scan.
It does not recreate descriptors or panic after the first `NoCandidate` pass.

Build the complete application from the repository root:

```console
cargo xtask build firmware station
cargo xtask build firmware station --flash --monitor --port /dev/ttyACM0
```

The [shared platform](../../../platform/esp32s31/README.md) initializes PSRAM,
relocates the separately linked application and keeps DMA and interrupt storage
in SRAM. The command checks ELF placement and stack frames before packaging
or flashing. `cargo build` in this example produces only the stage-two ELF;
flash the complete image through `xtask`. Hardware readiness still requires
appropriate scenario evidence.

See the [Wi-Fi network integration](../../../docs/wifi-egress.md#owned-tx-path)
for packet ownership, RX scheduling and admission limits.

The connected radio is a finite lifecycle epoch rather than a terminal task.
Peer loss or an application controller request returns the IRQ, staged-RX,
DMA, TX, sequence and CCMP-key owners, while the Embassy network stack and
sockets stay alive across link down/up. Before the next association, a finite
13-channel running scan temporarily owns only the quiesced hardware, stopped
RX and ordinary TX descriptor. It returns every owner and a fresh `ScanRecord`
before the stopped RX resources are split for reconnect. A fresh `ScanRecord`
can select another BSSID or channel. Product readiness is determined by the
[qualification program](../../../qualification/targets/esp32s31/wifi-sta.toml) and
current evidence, independently of this application's source check.
