# ESP32-S31 access-point application

This example composes the public Wi-Fi integration with application-owned
static IPv4, DHCP, UDP echo and TCP echo services. The radio driver owns DMA,
interrupts, associations and WPA2 keys; it does not implement those IP services.

The application requests WPA2-Personal on channel 6 with 20 MHz bandwidth and
uses `192.168.4.1/24`. DHCP leases are drawn from `192.168.4.100..=114`; both
echo services use port 7. `AP_CLIENT_LIMIT` in `src/main.rs` selects the admitted
peer count, currently four, within the request type's 1..=15 resource boundary.
That capacity is not a hardware qualification result.

Run Cargo from this workspace so its embedded target configuration is used:

```console
cd examples/esp32s31-access-point
ESP32S31_AP_SSID=open-radio \
ESP32S31_AP_PASSPHRASE=replace-this-password \
cargo check --release
```

The example uses the same product memory profile as the station application.
Its single-stage ESP-HAL linker does not provide the complete PSRAM placement
and initialization contract required by that profile. See the
[station application](../esp32s31-station/README.md) for that boundary; a source
check does not establish that this image can be flashed and run. The
[HIL composition](../../hil/targets/esp32s31/README.md) owns the complete board,
bootstrap and placement contract used for measured workloads.
