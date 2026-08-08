# ESP32-S31 standalone monitor

This example starts the source-only radio through the public topology facade,
materializes the common MAC once, and hands a checked standalone-monitor plan
to the Embassy monitor builder. RX DMA, the CPU interrupt route, capture pool,
and PHY state remain in one finite owner graph.

Captured frames are copied into bounded independent slots before DMA recycle.
The example prints only periodic aggregate counters and selected metadata; it
does not serialize frames or PCAPNG on the embedded hot path. A host-side
capture transport and PCAPNG writer belong to the HIL tooling.

Build with:

```text
cargo build --manifest-path examples/esp32s31-monitor/Cargo.toml \
  --release --target riscv32imafc-unknown-none-elf
```
