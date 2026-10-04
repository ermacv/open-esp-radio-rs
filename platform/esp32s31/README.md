# ESP32-S31 staged application boot

This platform composes ESP-HAL for ESP32-S31-Function-CoreBoard-1. Both the
standalone examples and HIL consume it; it has no HIL protocol, scenario,
radio role, executor or network-stack dependency.

| Component | Responsibility |
| --- | --- |
| `board` | Board-specific 16-MiB PSRAM at 250 MHz and 16-MiB Flash configuration; adoption of the live mapping |
| `bootstrap` | Flash entry, PSRAM initialization, image validation/CRC, relocation, Flash tuning and non-returning handoff |
| `runtime` | Stage-two entry, SRAM section initialization, mapping adoption, vector handoff, per-core interrupt stacks and the image's one panic entry |
| `layout` | Address map, the stage-two placement and the stage-two header/checksum shared with the host packer and auditor |
| `linker` | Semantic code, data, DMA and stack sections over the `layout` symbols |
| `partitions` | Application partition layout |
| `stack.toml` | Frame budgets for standalone application composition |

The runtime owns every image's `#[panic_handler]`. It writes a bounded record
of the panic to `.rtc_fast.persistent` without formatting (the end of the
location's file path, line, column, the start of the message when it is a
static string, hart and, on an interrupt stack, the interrupted PC), so the
panic path stays a short leaf in every stack bound. An image with
`panic-hook` defines `oer_platform_panic_hook`, which the entry calls after
the record to record more of its own state the same way; it returns. Every
image then resets, and its next boot reads and clears the record with
`panic::take_previous` and prints it.

Every image of this platform, standalone or HIL, is compiled with the flags
of one owner, [`oer-esp32s31-firmware`'s `compiler`
module](../../tools/firmware/README.md); no `.cargo/config.toml` adds Rust
flags.

The boot sequence is ROM → ESP-IDF bootloader → Flash bootstrap → application.
The ROM image uses DIO at 80 MHz; ESP-IDF enables QIO for the application.
`xtask` extracts and checks the ROM image from the installed `espflash` image
resources, then writes it, the partition table, the audited QIO application
and the ota_0 selector as raw flash segments through one connection, skipping
segments whose flash contents already match; the selector goes last. It
preserves NVS and other application partitions. Passing QIO to a single
`espflash flash` invocation would also change the ROM image header and
prevents this board from booting.
`partitions/calibration-slots.csv` is a second layout with two application
slots, `ota_0` and `ota_1` of 8 MiB each, for captures that alternate two
firmwares by the OTA selection alone:
`oer_esp32s31_hil_board::Staged::flash_slot` writes a firmware into a slot once,
`select_slot` selects the slot and optionally erases `phy_init` and `nvs`,
leaving the chip for the caller's own reset, `booted_slot` reads which slot a
boot's console shows the bootloader loading, and `boot_slot` does both around
its own reset. The bootstrap finds its
payload through the flash MMU, so an application boots from either slot. A HIL
run writes `applications.csv` again.
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
The runtime is the only owner of each hart's active vector table (MTVT):
every CPU interrupt enters through its stack-switching vector entry and
esp-hal's shared dispatcher. The esp-hal fork only vectors the
inter-processor call's line (CLINT software interrupt 3) and runs the posted
function from that dispatcher, instead of writing its own handler into the
slot; `enable_interrupts_after_handoff` panics when any hardware-vector slot
differs from the runtime's entries, so a later direct binding cannot run on
the interrupted stack. HIL's `system-ipc-call` calls each core.

Each image declares its peripheral interrupt sources once with
`oer_esp32s31_platform_runtime::interrupt_table!` (source, handler, level,
core; see [`oer-interrupt-table`](../../crates/runtime/interrupt-table/README.md)):
the macro links each handler into its source's slot of esp-hal's
`__EXTERNAL_INTERRUPTS` and creates one token per source; the image hands its `INTERRUPT_TABLE` to
`adopt_psram`. Each 12-byte entry holds the `Interrupt` (`u16`), the
`Priority` (`u8`), the `Cpu` (a 4-byte C enum at offset 4) and the handler
address at offset 8 (zero for an entry a `cfg` leaves out); compile-time
assertions in `interrupts.rs` keep that layout, which the stack analysis
reads. The runtime's placement audit requires every entry's slot
symbol and handler function in SRAM. Installing a hart's
interrupt stack silences every source of that hart's entries and checks the
slots; `enable_interrupts_after_handoff` checks again that every slot holds its
handler and every table source is silent or routed to its level. An owner
routes its source with `oer_esp32s31_soc_esp_hal::interrupt_table::enable` and
its token, on the table's core only, and silences it with `disable`; the
matrix lives in that adapter so that owners in `crates/` reach it. The stack analysis does not
yet fail a table entry whose `enable` no root of the image reaches. HIL's
`system-interrupt-table` checks that a source stays silent until enabled and
after it is disabled. esp-hal's PMP region setup
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

## Boot state on the board

The [boot-state probe](../../verification/esp32s31/hardware/boot-state-probe/README.md)
reads what the bootloader leaves and what esp-hal relies on:

- The `espflash`-bundled ESP-IDF bootloader already programs PMA entry 7 as
  the locked 64 MiB NAPOT external-memory aperture at `0x5000_0000`, the
  value esp-hal's `pre_init` writes; entry 15 covers the flash window
  `0x4000_0000` (512 MiB), entries 12-14 the internal memories, and entry 8 is
  unused. Newer ESP-IDF bootloaders that put the flash window top in entry 7
  would conflict with esp-hal's locked write.
- A Flash MMU entry implements bits 0-10 and 13 of the content word: the
  page-number field the PAC publishes as 10 bits (`PADDR`, with bit 10 as
  `ACCESS_SPIRAM`) is the 11-bit value mask of ESP-IDF's `ext_mem_defs.h`;
  a read-back cannot tell which meaning bit 10 has, and the production
  translation reads 10 bits, enough for 64 MiB of flash.
- `PMU.IMM_HP_CK_POWER_1` reads zero after its tie-high and tie-low bits are
  written: they are write-only pulses, not state.
- After ESP-IDF's `rng_ll_enable` sequence, which pulses the block reset
  after setting it, `TRNG.DATE.CLK_EN` reads zero while the TRNG still
  produces changing output; ESP-IDF's `rng_ll_is_enabled` checks only the
  LP clock and reset, and so does `entropy::source_status`.
- Disabling the TIMG watchdog through esp-hal holds: no reset arrives past
  the armed timeout.

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
