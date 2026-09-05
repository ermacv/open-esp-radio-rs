# ESP32-S31 SoC services

This adapter retains the `open-esp-radio-esp32s31-platform-pac` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services. Radio register
ownership remains in `driver/chips/esp32s31/pac`.

The private source modules follow the resources and operations they own:

- `cache/maintenance.rs`: PSRAM writeback before a DMA reader observes memory.
- `cache/performance.rs`: cache counter snapshots and the retained CACHE witness.
- `flash/mmu.rs`: flash MMU operations with the retained SPI0 witness.
- `dma/mem2mem/descriptor.rs`: descriptor images, burst sizing, chain construction,
  and descriptor validation; its host tests live in `descriptor/tests.rs`.
- `dma/mem2mem/registers.rs`: typed upstream AXI-GDMA register operations,
  interrupt source controls, and DMA visibility fences.
- `dma/mem2mem/transfer.rs`: the DMA channel witness and prepared/active transfer
  owners retaining exclusive payload and descriptor borrows, including cleanup.
- `dma/mem2mem/completion.rs`: the channel-zero interrupt handler, static waker,
  and Future polling with its completion recheck.

The transfer owner binds both channel interrupts when constructed. Register
operations and completion share only the private mem2mem module boundary;
callers continue to use the existing root types. Descriptor layout, linker
sections, feature selection, reset/start ordering, and owner retention remain
unchanged by this module split.

`axi-gdma-mem2mem` enables the hardware transfer path and implies `esp32s31`.
`psram-dma-diagnostic` additionally enables the existing blocking comparison
path. The descriptor sizing tests run on the host without those features.

CACHE maintenance and performance counters retain their existing distinct
contracts: writeback takes the affected mutable memory range, while performance
counters retain the CACHE witness. Their shared hardware serialization contract
is unresolved; this organization introduces no CACHE lease or coordinator.

Focused checks from the repository root:

```console
cargo test --locked --offline -p open-esp-radio-esp32s31-platform-pac
cargo check --locked --offline -p open-esp-radio-esp32s31-platform-pac --target riscv32imafc-unknown-none-elf --features axi-gdma-mem2mem
cargo check --locked --offline -p open-esp-radio-esp32s31-platform-pac --target riscv32imafc-unknown-none-elf --features psram-dma-diagnostic
```
