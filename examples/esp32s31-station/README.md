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

The default build uses the optimized owned-packet Xarxa/Embassy fork. The same
application source can be checked against released Embassy crates and the
upstream-compatible copied-frame adapter:

```console
cargo check --release --no-default-features --features compat-network
```

The two network integrations are compile-time alternatives and cannot be
combined in one binary.

Network credentials are application build configuration. They are deliberately
absent from reusable driver crates and HIL configuration:

```console
ESP32S31_WIFI_SSID='your ssid' \
ESP32S31_WIFI_PASSPHRASE='your passphrase' \
cargo check --release
```

After scan, WPA2 and DHCP complete, the example exposes a small UDP echo
service on port 4321. This is application traffic through the normal
`embassy-net` device; it is not a HIL benchmark path.

If no matching AP is present, each complete 13-channel cold scan returns its
halted RX ring, waits 500 ms and prepares that same owner for the next scan.
It does not recreate descriptors or panic after the first `NoCandidate` pass.

The reusable product integration selects its fixed product ownership graph
and explicitly separates general-memory network payloads from the fixed
DMA-visible SRAM horizon. The HIL target owns the reference linker
and bootstrap contract which initializes PSRAM and maps those sections. This
standalone example's ordinary ESP-HAL linker configuration does not yet provide
that complete placement contract, so a successful source `cargo check` is not
by itself a flashable-image or hardware-qualification claim.

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
