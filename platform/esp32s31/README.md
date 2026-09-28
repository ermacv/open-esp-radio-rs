# ESP32-S31 staged application boot

This platform composes ESP-HAL for ESP32-S31-Function-CoreBoard-1. Both the
standalone examples and HIL consume it; it has no HIL protocol, scenario,
radio role, executor or network-stack dependency.

| Component | Responsibility |
| --- | --- |
| `board` | Board-specific 16-MiB PSRAM at 250 MHz and 16-MiB Flash configuration; adoption of the live mapping |
| `bootstrap` | Flash entry, PSRAM initialization, image validation/CRC, relocation, Flash tuning and non-returning handoff |
| `runtime` | Stage-two entry, SRAM section initialization, mapping adoption, vector handoff and per-core interrupt stacks |
| `layout` | Address map, the stage-two placement and the stage-two header/checksum shared with the host packer and auditor |
| `linker` | Semantic code, data, DMA and stack sections over the `layout` symbols |
| `partitions` | Application partition layout |
| `stack.toml` | Frame budgets for standalone application composition |

The boot sequence is ROM → ESP-IDF bootloader → Flash bootstrap → application.
The ROM image uses DIO at 80 MHz; ESP-IDF enables QIO for the application.
`xtask` extracts and checks the ROM image from the installed `espflash` image
resources, then writes it, the partition table, the audited QIO application
and the ota_0 selector as raw flash segments through one connection, skipping
segments whose flash contents already match; the selector goes last. It
preserves NVS and other application partitions. Passing QIO to a single
`espflash flash` invocation would also change the ROM image header and
prevents this board from booting.
The runtime is linked separately. Its header supplies the entry, payload and
initialization ranges; the host packs the checksum before embedding it in the
bootstrap. Bootstrap copies and verifies PSRAM code before transferring control.
It does not return or carry Rust peripheral owners across the image boundary.

Stage-two assembly initializes application data and SRAM interrupt/DMA sections
before entering `runtime_main`. The application calls `esp_hal::init`, then
unsafely adopts the bootstrap mapping through `oer_esp32s31_platform_runtime::adopt_psram`
with its unique PSRAM token. This also reinitializes vectoring and installs the
per-core SRAM interrupt stack. Global interrupts remain disabled until the
application binds its timers and executor handlers; it then calls the unsafe
`oer_esp32s31_platform_runtime::enable_interrupts_after_handoff` once per hart. PSRAM
must not be reset or remapped after handoff.
The bootstrap configures the brownout detector as the ESP-IDF application
does (`esp_hal::rtc_cntl::brownout::configure` with its defaults): the
analog mode-1 reset the bootloader enables is turned off, and mode 0 at level
7 suspends flash, powers down RF and resets the system after its wait. The
non-interrupt variant is used, so no brownout handler runs on the stack-switched
vectors.
Every CPU interrupt enters through the runtime's stack-switching vector entry
and esp-hal's shared dispatcher, so a handler that esp-hal binds directly to a
vector slot does not run: images do not use `esp_hal::interrupt::ipc`
(CLINT software interrupt 3). esp-hal's PMP region setup
(`ESP_HAL_CONFIG_ENABLE_PMP`) is off in the root Cargo configuration, because
its single no-execute data region would span stage two's PSRAM code.

Every image uses one placement: PSRAM for code, ordinary data and a 192-KiB
CPU0 task stack; SRAM for interrupt entries, hot code, critical state, DMA
storage and two 32-KiB interrupt stacks. HIL also selects a 16-KiB CPU1 task
stack through the `multicore` runtime feature; its CPU startup policy and
second-core application entry remain in HIL. These board sizes are not
universal chip capabilities.

Because code runs from PSRAM, the adopted mapping keeps the PSRAM function
clock, and through it MPLL, referenced in ESP-HAL's clock tree for the life of
the image. A driver that releases MPLL therefore never powers it down under
PSRAM.

## Recovery after a lockup

An application can leave LP/PMU state behind that survives a watchdog reset,
the USB-Serial-JTAG (RTS) reset and a reflash: on 2026-09-28 a CPU lockup with
MPLL powered down left the ESP-IDF bootloader looping on
`rst:0x7 HP_SYS_HP_WDT0_RESET` after `Multicore bootloader`. A system reset
through the built-in USB-JTAG (OpenOCD `reset run`, reported by the ROM as
`rst:0x3 SW_SYS_RESET`) cleared it; this board has no EN line wired to the
host. The HIL runner applies that escalation itself (see the
[runner's boot-loop recovery](../../hil/host/README.md)); a board that none of
its resets clears is quarantined for a person.

Each dedicated IRQ stack is painted once during its first installation, before
IRQ admission. Reinstalling vectors preserves the paint. Thread-mode callers
can sample only their current hart through
`stacks::current_hart_interrupt_stack_free_bytes`; it masks local interrupts
for the bounded SRAM scan and restores their prior enable state. It rejects
sampling from the IRQ stack or before initialization. Measurements describe
observed writes, not the maximum possible depth or unwritten stack reservations.

From the repository root:

```console
cargo xtask build firmware monitor
cargo xtask build firmware station
cargo xtask build firmware access-point --flash --monitor --port /dev/ttyACM0
```

Select `station`, `access-point`, `monitor` or `thread`. Application
credentials remain environment configuration of the example; HIL credentials
remain lab configuration. Each successful invocation retains a separate bundle
under `target/firmware/esp32s31-<example>/<network-or-none>/build-<id>/`:
`application.bin`, ROM `bootloader.bin`, partition/OTA images, packed runtime,
`runtime.elf`, `bootstrap.elf`, both resolved lockfiles and
placement/stack reports. The build rejects invalid placement and oversized
frames before flash. Frame budgets and boundary watchpoints do not prove the
maximum aggregate depth of every possible call chain.

The host [firmware library](../../tools/firmware/README.md) supplies packing and
structural checks to `xtask` and HIL. HIL retains its image classification,
observer placement requirements, stack budgets and sealed evidence. Source or
image checks alone do not establish RF qualification.
