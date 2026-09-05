# ESP32-S31 access-point application

This example composes the public Wi-Fi integration with application-owned
static IPv4, DHCP, UDP echo and TCP echo services. The radio driver owns DMA,
interrupts, associations and WPA2 keys; it does not implement those IP services.

The application requests WPA2-Personal on channel 6 with 20 MHz bandwidth and
uses `192.168.4.1/24`. DHCP leases are drawn from `192.168.4.100..=114`; both
echo services use port 7. `AP_CLIENT_LIMIT` in `src/main.rs` selects the admitted
peer count, currently four, within the request type's 1..=15 resource boundary.
That capacity is not a hardware qualification result. TCP echo preserves the
peer's half-close: pending replies and FIN are drained before socket reuse.
Transport errors or a two-second close deadline trigger abort and another
bounded reset drain; if reset cannot drain, the old socket is retired.

The application service lifecycle has host regression tests, included in
`cargo xtask check examples`.

Run Cargo from this workspace so its embedded target configuration is used:

```console
cd examples/esp32s31-access-point
ESP32S31_AP_SSID=open-radio \
ESP32S31_AP_PASSPHRASE=replace-this-password \
cargo check --release
```

Build the complete application from the repository root:

```console
cargo xtask build firmware access-point
cargo xtask build firmware access-point --flash --monitor --port /dev/ttyACM0
```

The [shared platform](../../platform/esp32s31/README.md) initializes PSRAM,
relocates the separately linked application and keeps DMA and interrupt storage
in SRAM. The command checks ELF placement and stack frames before packaging
or flashing. `cargo build` in this example produces only the stage-two ELF;
flash the complete image through `xtask`. Hardware readiness still requires
appropriate scenario evidence.
