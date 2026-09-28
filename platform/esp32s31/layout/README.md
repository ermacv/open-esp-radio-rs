# ESP32-S31 staged-boot layout

`oer-esp32s31-platform-layout` is the single definition of the staged-boot
address map and image contract for ESP32-S31-Function-CoreBoard-1. It is a
`no_std` library without hardware access, shared by the Flash bootstrap, the
stage-two runtime, the board profile, application and HIL build scripts, and
the host packer and image auditor in [`oer-esp32s31-firmware`](../../../tools/firmware/README.md).

| Module | Owns |
| --- | --- |
| `memory` | SRAM, Flash XIP and PSRAM regions, the second-stage loader bound, the bootstrap PSRAM probe page, the stage-two PSRAM window and stack sizes |
| `stage_two` | The little-endian image `Header`, its magic, ABI version and size, and the payload CRC-32 with the checksum field read as zero |
| `build` (feature `build`) | `configure_runtime` and `configure_bootstrap`, which link a binary with the scripts under [`linker`](../linker) and define every layout value as a `--defsym` symbol |

The linker scripts contain no addresses or header constants of their own; they
read `SRAM_ORIGIN`, `RUNTIME_PSRAM_ORIGIN`, `STAGE_TWO_MAGIC` and the other
symbols emitted by `build`. Stage two has one placement: code, data and task
stacks in PSRAM; interrupt entries, per-hart interrupt stacks, hot code,
critical state and DMA state in internal SRAM.

Region sizes describe this board's fitted memories and boot contract, not
universal chip capabilities. Changing a value changes the linked images and the
bootstrap validation together; the stage-two ABI version must change with any
incompatible header change.

The crate belongs to the root workspace so host tools and tests build it; the
platform workspace excludes it and consumes it by path. Run its tests with
`cargo test -p oer-esp32s31-platform-layout`.

## Layout seed

Code that runs from cached external memory is sensitive to where the linker
happens to place each function: an unrelated dependency change can re-sort
them and move a throughput figure by tens of percent. Setting
`OER_LAYOUT_SEED` to a nonzero decimal `u32` while building a runtime image
makes the linker shuffle the ordinary `.text.*` and `.rodata.*` input sections
with that seed. The same seed reproduces the same image; only the binary is
relinked. The entry, trap and instruction-stream sections and the explicitly
placed ISR, hot and critical sections keep their placement. A performance
comparison measures several seeds of the same source instead of one
accidental placement. Zero is rejected, because the linker would read it as a
random seed that no build record could reproduce.
