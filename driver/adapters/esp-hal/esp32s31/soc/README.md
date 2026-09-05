# ESP32-S31 SoC services

This adapter retains the `open-esp-radio-esp32s31-platform-pac` package name
and its public root imports. It uses the pinned esp-hal peripheral witnesses
and upstream register accessors for non-radio SoC services. Radio register
ownership remains in `driver/chips/esp32s31/pac`.

The private source modules follow the resources and operations they own:

- `cache/maintenance.rs`: validates the borrowed PSRAM range and delegates
  writeback to HAL before a DMA reader observes memory.
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
callers use the public root types. Prepared and active owners retain the
payload and descriptor borrows until completion or cleanup.

`axi-gdma-mem2mem` enables the hardware transfer path and implies `esp32s31`.
`psram-dma-diagnostic` additionally enables the existing blocking comparison
path. The descriptor sizing tests run on the host without those features.

HAL owns the cache sync engine and serializes writeback with its DMA and
executable-PSRAM cache operations. The adapter does not maintain a second
register sequence or lock for that engine.

CACHE maintenance and performance counters have distinct contracts:
writeback takes the affected mutable memory range, while performance
counters retain the CACHE witness. There is no shared CACHE lease or
coordinator between those APIs; the adapter does not establish that simultaneous use is safe.

Focused checks from the repository root:

```console
cargo test --locked --offline -p open-esp-radio-esp32s31-platform-pac
cargo check --locked --offline -p open-esp-radio-esp32s31-platform-pac --target riscv32imafc-unknown-none-elf --features axi-gdma-mem2mem
cargo check --locked --offline -p open-esp-radio-esp32s31-platform-pac --target riscv32imafc-unknown-none-elf --features psram-dma-diagnostic
```
