# ESP32-S31 monitor application

This example uses the same public constructor and hardware runner as STA,
then consumes `WifiIdle` to start an exclusive monitor epoch. The application
does not own PAC, DMA or ISR resources.

Captured frames are copied into bounded independent slots before DMA recycle.
The example prints periodic aggregate counters and selected metadata; it does
not serialize frames or PCAPNG on the embedded hot path. Capture transport and
PCAPNG serialization belong to HIL host tooling.

Run Cargo from this workspace so its embedded target configuration is used:

```console
cd examples/esp32s31-monitor
cargo check --release
```

The integration uses the same product memory profile as the
[station application](../esp32s31-station/README.md). Its persistent general-memory resources
require initialized PSRAM, while DMA-visible storage remains in SRAM. The
single-stage example linker does not provide the complete product placement
contract; the [HIL target](../../hil/targets/esp32s31/README.md) owns that board
and bootstrap composition. A source check is not a flashable-image or hardware
qualification claim.
