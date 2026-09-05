# ESP32-S31 staged application boot

This platform composes ESP-HAL for ESP32-S31-Function-CoreBoard-1. Both the
standalone examples and HIL consume it; it has no HIL protocol, scenario,
radio role, executor or network-stack dependency.

| Component | Responsibility |
| --- | --- |
| `board` | Board-specific 16-MiB PSRAM at 250 MHz and 16-MiB Flash configuration; adoption of the live mapping |
| `bootstrap` | Flash entry, PSRAM initialization, image validation/CRC, relocation, Flash tuning and non-returning handoff |
| `runtime` | Stage-two entry, SRAM section initialization, mapping adoption, vector handoff and per-core interrupt stacks |
| `linker` | Shared load/run addresses and semantic code, data, DMA and stack sections |
| `partitions` | Application partition layout |
| `stack.toml` | Frame budgets for standalone application composition |

The boot sequence is ROM → ESP-IDF bootloader → Flash bootstrap → application.
The ROM image uses DIO at 80 MHz; ESP-IDF enables QIO for the application.
`xtask` extracts and checks the ROM image from the installed `espflash` image
resources, then writes it, the partition table, the audited QIO application
and the ota_0 selector as separate transactions. It preserves NVS and other
application partitions. Passing QIO to a single `espflash flash` invocation
would also change the ROM image header and prevents this board from booting.
The runtime is linked separately. Its header supplies the entry, payload and
initialization ranges; the host packs the checksum before embedding it in the
bootstrap. Bootstrap copies and verifies PSRAM code before transferring control.
It does not return or carry Rust peripheral owners across the image boundary.

Stage-two assembly initializes application data and SRAM interrupt/DMA sections
before entering `runtime_main`. The application calls `esp_hal::init`, then
unsafely adopts the bootstrap mapping through `oer_esp32s31_runtime::adopt_psram`
with its unique PSRAM token. This also reinitializes vectoring and installs the
per-core SRAM interrupt stack. Global interrupts remain disabled until the
application binds its timers and executor handlers. PSRAM must not be reset or
remapped after handoff.

Standalone applications use PSRAM for code, ordinary data and a 192-KiB CPU0
stack. DMA storage and two 32-KiB interrupt stacks remain in SRAM. HIL also
selects a 16-KiB CPU1 task stack through the `multicore` runtime feature; its
CPU startup policy and second-core application entry remain in HIL. The shared
linker also supports the HIL control profiles with SRAM data or inherited SRAM
thread stacks. These board/profile sizes are not universal chip capabilities.

From the repository root:

```console
cargo xtask build firmware monitor
cargo xtask build firmware station --no-default-features --features compat-network
cargo xtask build firmware bluetooth-controller --flash --monitor --port /dev/ttyACM0
```

Select `station`, `access-point`, `monitor` or `bluetooth-controller`. Application
credentials remain environment configuration of the example; HIL credentials
remain lab configuration. Each successful invocation retains a separate bundle
under `target/firmware/esp32s31-<example>/build-<id>/`:
`application.bin`, ROM `bootloader.bin`, partition/OTA images, packed runtime,
`runtime.elf`, `bootstrap.elf`, both resolved lockfiles and
placement/stack reports. The build rejects invalid placement and oversized
frames before flash. Frame budgets and boundary watchpoints do not prove the
maximum aggregate depth of every possible call chain.

The host [firmware library](../../tools/firmware/README.md) supplies packing and
structural checks to `xtask` and HIL. HIL retains its image classification,
observer placement requirements, stack budgets and sealed evidence. Source or
image checks alone do not establish RF qualification.
