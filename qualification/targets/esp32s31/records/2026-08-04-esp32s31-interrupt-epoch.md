# ESP32-S31 connected interrupt epoch qualification

Qualification ID: `HIL_ESP32S31_INTERRUPT_EPOCH_2026_08_04`

Scenario: `radio` / `open-radio-hil`
Profile: `psram-code-psram-data`
Device: ESP32-S31 revision 0.0
Runtime CRC32: `02cbd34c`
Application image: 1,203,712 bytes

This cell requalifies repeated connected station epochs after CPU routing,
stable ISR register storage, bounded hard-handler service and task-side wake
drain moved out of `radio_hil.rs`.

The executor-neutral MAC crate now exposes `MacInterruptRoute`. The Embassy
integration owns `Esp32s31MacInterruptEpoch`: an inactive epoch contains the
unique `MacInterruptSetup`; activation lends that owner to a platform route;
quiescence first recovers the same owner and only then drains MAC RX/TX and
power-event publications. Activation failure restores the setup token and
quiescence failure leaves the active route frontier intact.

The ESP-HAL adapter now owns the concrete route. It stores the disjoint MAC
and power PAC values at stable addresses, publishes active pointers before
binding either CPU route, disables both same-core routes before recovery, and
masks/clears both peripheral banks through `MacInterruptSetup`. Its reusable
hard-handler services run at most 32 nonzero snapshots. LTO inlined both
services into the HIL `.rwtext` handlers; placement and autonomous source-
graph audits passed.

HIL retains only the two `esp_hal::handler` entry points, an observation sink
which records RX timing/classification, and executor task-stop signals. It no
longer owns ISR `StaticCell` storage, active/storage `AtomicPtr` values, PAC
activation/deactivation, CPU route disable, or stale Embassy wake drain.

All 123 Embassy-integration host tests and all 79 ESP32-S31 MAC tests passed.
The application image is byte-for-byte the same size as the preceding
connected-transition image and still occupies 38.26% of its application
partition. No DMA/SRAM placement changed.

The image was flashed through `/dev/ttyACM0`, then:

```text
cargo hil station reconnect --serial /dev/ttyACM0 --cycles 3 --timeout-seconds 120
```

completed all three requested cycles. Each cycle observed production runner
stop, interrupt quiescence/drain, protocol owner return, control shutdown,
RX-DMA stop, idle TX/resource return, a 13-channel running scan, fresh Open
Authentication/Association/WPA2 and connected re-entry. The RX descriptor
base remained `0x2f03ea10`; every stopped epoch reported zero queued protocol
and RX frames. One or more stale RX wakes were deliberately drained in later
epochs and did not leak into the subsequent epoch.

The transient UART evidence had SHA-256
`32d88cda0b0bde9d68e8cd553ecdd4960c36b28d0338819a9227472494cc8532`.
Credentials were runtime-provisioned and are absent from the image/report.

Executor task allocation, core placement and stop signals remain HIL fixture
policy. Their current acknowledgement waits are not finite. A prototype
timeout added less than 2 KiB of useful code but crossed the current PSRAM-code
image placement boundary and increased the encoded application by 56,464
bytes, so it was not retained. The next recovery slice must introduce a
compact bounded acknowledgement/reset frontier without carrying the large
stopped protocol owner through another async select state.
