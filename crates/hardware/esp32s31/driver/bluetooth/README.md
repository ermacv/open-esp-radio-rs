# ESP32-S31 Bluetooth hardware engine

This crate owns chip hardware sequencing and affine radio publication states
below the Link Layer: clocks, Controller HAL and time, the BLE PHY, interrupts,
the modem low-power timer and the hardware scheduler with its timeline,
finished lists, post-unlink handoff and single-item completion primitives. It
knows no LE role. No LE Controller currently composes these primitives, so no
Controller lifecycle, executor runtime or final integration exists.

Portable HCI policy and LE Link Layer codecs live in
[`crates/protocols/bluetooth`](../../../../protocols/bluetooth/).

The module table below is the engine API map. `FEATURES.md` links current
limitations to the generated qualification view; it is not a second readiness
inventory.

| Module under `src/` | Responsibility |
| --- | --- |
| `clock` | Bluetooth clock and reset sequencing over the platform lease |
| `controller_hal` | Controller HAL component initialization after clock setup |
| `controller_time` | Event-driven Controller-time latch and scheduler-epoch projection |
| `ble_phy` | BLE PHY engine initialization, timing authority and restart parts |
| `phy` | Common PHY power/readback, registration, Bluetooth-client acquisition and initial tracking |
| `interrupt` | Chip interrupt state and hardware handling |
| `modem_timer` | Controller modem low-power timer task over the published timer owner |
| `scheduler` | Event executor (idle and live insertion, cancellation, list deletion, completion, stopped state), ordered hardware-list mirror, raw windows, timeline, finished lists, post-unlink handoff and single-item completion |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Stopped aggregate, platform lease and runtime owner leases |
| `runtime_resources` | Durable software queues and wake cells of one powered epoch |
| `memory` | Controller-SRAM storage owners and completion values from the memory crate |

Role crates reach hardware only through these typed primitives. Operations
that discharge an `unsafe` obligation, such as publishing an interrupt owner
or preparing Controller output, are safe functions of this crate. The
`test-support` feature exposes validation constructors to dependent crates'
host tests; it is not a production feature.

The separate [`memory`](memory/) crate retains controller-SRAM codecs.

## Controller time

Controller time uses the HAL period's software conversion selector. The
standalone profile has two raw ticks per microsecond; its hardware divider is
a separate setting. Scheduler epoch updates retain fractional raw ticks, and
earlier fractional timestamps round down before deadline projection.
