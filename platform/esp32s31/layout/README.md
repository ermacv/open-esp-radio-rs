# ESP32-S31 staged-boot layout

`oer-esp32s31-platform-layout` is the single definition of the staged-boot
address map and image contract for ESP32-S31-Function-CoreBoard-1. It is a
`no_std` library without hardware access, shared by the Flash bootstrap, the
stage-two runtime, the board profile, application and HIL build scripts, and
the host packer and image auditor in [`oer-firmware`](../../../tools/firmware/README.md).

| Module | Owns |
| --- | --- |
| `memory` | SRAM, Flash XIP and PSRAM regions, the bootstrap PSRAM prefix, stage-two code/data windows, stack sizes and the validated `RuntimeProfile` |
| `stage_two` | The little-endian image `Header`, its magic, ABI version and size, and the payload CRC-32 with the checksum field read as zero |
| `build` (feature `build`) | `configure_runtime` and `configure_bootstrap`, which link a binary with the scripts under [`linker`](../linker) and define every layout value as a `--defsym` symbol |

The linker scripts contain no addresses or header constants of their own; they
read `SRAM_ORIGIN`, `RUNTIME_PSRAM_ORIGIN`, `STAGE_TWO_MAGIC` and the other
symbols emitted by `build`. A profile is either PSRAM code with PSRAM or SRAM
data, or Flash code with PSRAM data; PSRAM task stacks require PSRAM code and
data. `RuntimeProfile::STANDALONE` is the standalone example profile.

Region sizes describe this board's fitted memories and boot contract, not
universal chip capabilities. Changing a value changes the linked images and the
bootstrap validation together; the stage-two ABI version must change with any
incompatible header change.

The crate belongs to the root workspace so host tools and tests build it; the
platform workspace excludes it and consumes it by path. Run its tests with
`cargo test -p oer-esp32s31-platform-layout`.
