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
cargo build --release
```

Network credentials are application build configuration. They are deliberately
absent from reusable driver crates and HIL configuration:

```console
ESP32S31_WIFI_SSID='your ssid' \
ESP32S31_WIFI_PASSPHRASE='your passphrase' \
cargo run --release
```

After scan, WPA2 and DHCP complete, the example exposes a small UDP echo
service on port 4321. This is application traffic through the normal
`embassy-net` device; it is not a HIL benchmark path.

If no matching AP is present, each complete 13-channel cold scan returns its
halted RX ring, waits 500 ms and prepares that same owner for the next scan.
It does not recreate descriptors or panic after the first `NoCandidate` pass.

This direct-to-flash example selects the compact internal-SRAM resource
profile: 16 staged RX owners, 8 network RX/TX slots and 8 TX A-MPDU members.
The qualified `high-throughput` profile uses 64/40/32/32 and requires the
product to place CPU-only owners in initialized PSRAM; the HIL target is the
reference composition for that linker contract.

The connected radio is a finite lifecycle epoch rather than a terminal task.
Peer loss or an application controller request returns the IRQ, staged-RX,
DMA, TX, sequence and CCMP-key owners, while the Embassy network stack and
sockets stay alive across link down/up. Before the next association, a finite
13-channel running scan temporarily owns only the quiesced hardware, stopped
RX and ordinary TX descriptor. It returns every owner and a fresh `ScanRecord`
before the stopped RX resources are split for reconnect. This permits a new
BSSID or channel to be selected; those two replacement cases still need a
controlled hardware qualification distinct from the same-AP smoke test.
