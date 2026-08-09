# ESP32-S31 standalone monitor

This example uses the same public constructor and eternal runner as STA, then
consumes `WifiIdle` to start an exclusive monitor epoch. No PAC, DMA or ISR
owner appears in the application.

Captured frames are copied into bounded independent slots before DMA recycle.
The example prints only periodic aggregate counters and selected metadata; it
does not serialize frames or PCAPNG on the embedded hot path. A host-side
capture transport and PCAPNG writer belong to the HIL tooling.

Build with:

```text
cargo build --manifest-path examples/esp32s31-monitor/Cargo.toml \
  --release --target riscv32imafc-unknown-none-elf
```
