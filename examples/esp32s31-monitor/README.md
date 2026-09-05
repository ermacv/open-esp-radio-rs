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

Build the complete application from the repository root:

```console
cargo xtask build firmware monitor
cargo xtask build firmware monitor --flash --monitor --port /dev/ttyACM0
```

The [shared platform](../../platform/esp32s31/README.md) initializes PSRAM,
relocates the separately linked application and keeps DMA and interrupt storage
in SRAM. The command checks ELF placement and stack frames before packaging
or flashing. `cargo build` in this example produces only the stage-two ELF;
flash the complete image through `xtask`. Hardware readiness still requires
appropriate scenario evidence.
