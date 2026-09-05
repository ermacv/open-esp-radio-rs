# Firmware boot/linker ownership decision

Date: 2026-09-05. Reviewed during the non-driver hierarchy migration.
Decision: retain the existing HIL boot/linker composition; no speculative
shared support crate or platform directory is introduced in this migration.

## Actual contract map

| Source | Contract it owns |
| --- | --- |
| [bootstrap entry](../../hil/targets/esp32s31/bootstrap/src/main.rs) | The stage-two header/ABI, source/destination CRC, PSRAM copy and BSS initialization, flash tuning, watchpoint release and final jump |
| [bootstrap build](../../hil/targets/esp32s31/bootstrap/build.rs) | Required `PSRAM_RUNTIME_BIN` packed input and bootstrap/ROM linker selection |
| [runtime build](../../hil/targets/esp32s31/runtime/build.rs) | Mutually exclusive code/data profiles, PSRAM task-stack admissibility and runtime linker symbols |
| [runtime linker](../../hil/targets/esp32s31/linker/runtime/sections.x) | Runtime header and load ranges; hot code, critical/IRQ stacks, DMA and general data placement; explicit unsupported RTC initialization |
| [board](../../hil/targets/esp32s31/board/src/lib.rs) | Function-CoreBoard-1 electrical limits and adoption of the already initialized 16-MiB PSRAM mapping |
| [station target configuration](../../examples/esp32s31-station/.cargo/config.toml) | Ordinary ESP-HAL `linkall.x` entry, without the custom stage-two load/header contract |

The apparent common subset is not a self-contained owner today. General
`.dma.*`/`.critical.*` placement depends on load addresses and copying by the
custom bootstrap/runtime pair. Interrupt stacks depend on the runtime entry
and vector setup. PSRAM mapping adoption assumes bootstrap initialization and
must not reset live memory. Removing HIL logging alone does not turn these
pieces into a reusable firmware contract.

The example currently has no second consumer of that stage-two ABI. Making it
one requires a new image-build composition, compatible reset/entry and memory
initialization, target linker integration and interrupt/stack validation.
That would extend the application's boot behavior rather than preserve the
existing implementation through a directory move.

## Consequence

HIL continues to own its complete reference image recipe and board policy.
The existing example limitation remains explicit: a target source check is
not proof of the full reference memory placement or a qualified boot image.
No HIL workload, UART protocol, telemetry or privileged fixture is imported
into examples or production driver to make reuse appear complete.

A future shared firmware owner is justified when a concrete application
adopts the stage-two contract, with both producer/consumer boundaries and
linked image evidence reviewed. This is an explicit outcome of stage 7's
conditional extraction review, not an unfinished path move. No new hardware
qualification is claimed by this decision.
