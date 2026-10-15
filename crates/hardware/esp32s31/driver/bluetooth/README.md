# ESP32-S31 Bluetooth hardware engine

This crate owns chip hardware sequencing and affine radio publication states
below the Link Layer: Controller HAL and time, the BLE PHY, interrupts,
the modem low-power timer and the hardware scheduler with its event executor,
list-zero hardware execution and finished lists. It knows no LE role. The
[radio role](../../../../roles/esp32s31/bluetooth/radio/) lowers the portable
LE radio contract onto the executor, and the
[radio runtime](../../../../runtime/esp32s31/bluetooth/) drives it. The
portable [LE Controller core](../../../../protocols/bluetooth/le/controller/)
serves HCI over that runtime.

Portable HCI codecs and LE Link Layer codecs live in
[`crates/protocols/bluetooth`](../../../../protocols/bluetooth/).

The module table below is the engine API map. `FEATURES.md` links current
limitations to the generated qualification view; it is not a second readiness
inventory.

| Module under `src/` | Responsibility |
| --- | --- |
| `controller_hal` | Controller HAL component initialization on the HAL's clocked Bluetooth client of the shared radio |
| `controller_time` | Event-driven Controller-time latch and scheduler-epoch projection |
| `ble_phy` | BLE PHY engine initialization, activation into the runtime owners and the retained PHY membership and BLE PHY/DF graph |
| `phy` | Entry into and exit from the shared PHY domain and BTBB |
| `interrupt` | Chip interrupt state and hardware handling |
| `modem_timer` | Controller modem low-power timer task over the published timer owner |
| `scheduler` | Event executor (idle and live insertion, cancellation, list deletion, completion, stopped state), ordered hardware-list mirror, raw windows, timing policy, list-zero hardware execution and finished lists |
| `timed_preparation` | Controller-time preparation shared by timed scheduler admissions |
| `resources` | Task and inactive interrupt owners split from the clocked client |
| `runtime_resources` | Interrupt-shared event cells reused by every epoch, and the workers and list-zero publications of one powered epoch; the powered task endpoint performs executor steps, starts the idle scheduler, stops it, publishes the global receive chains and retires its task owner |
| `emergency` | Independent affine emergency authority, counted task borrows and permanent quarantine |
| `shutdown` | Retirement of a stopped epoch (Controller output, PHY and BTBB exit) and the Controller reset that returns the clocked client and the BLE PHY allocations |
| `memory` | Controller-SRAM storage owners and completion values from the memory crate |

Role crates reach hardware only through these typed primitives. The powered
task endpoint publishes an address only after it resolves to a submitted item
of the caller's scheduler item space, whose pools bind `'static` storage. It
retains every list-zero publication from the action that creates it to the
action that ends it, so an action out of transaction order changes no
register. Each wait observation joins a fresh task-side read with a fresh
scheduler-state read from the stable interrupt owner; the scheduler wake only
tells the worker when to observe again. Operations
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

Physical event spans are non-empty and at most the signed raw-counter
half-range. The duration projection accepts a `RadioDuration<LeRadio>`, checks the full
`u64` value and fractional carry, and narrows only after proving the result
fits. Absolute scheduler timestamps keep their reviewed modular projection;
that representation does not authorize wrapping a portable radio instant.
First-receive and recurring waits validate their complete geometry before
encoding the controller-SRAM fields.

## Emergency quarantine

`separate_emergency` consumes the powered task and returns `PortController`
and `ControllerEmergencyStop`. The backend retains the latter outside the
unbranded driver. Ordinary task borrows and the complete ISR, including its
wake publications, enter the same installation-local atomic gate. Emergency
entry and its bounded waits never enter the ISR's critical section. The gate never
reopens after an emergency; forgetting a counted borrow fails closed.

`stop` closes admission, wakes the platform runner, then masks the bound CPU
routes in order 127 (timer work), 133 (opaque NRT source), 124 (primary dynamic
groups). The affine routes retain their target CPU; emergency masking works
from either hart and changes no Wi-Fi or IEEE 802.15.4 route. The emergency
owner holds a reference to the static routes, so neither lookup nor the
three matrix writes takes the route-storage critical section. An entered operation
that misses that join prevents every emergency register access.

After CPU exclusion the restricted PAC view samples scheduler BUSY, masks
the dynamic groups, disables the RUN-event source, waits for command readiness,
publishes the lifecycle request and waits for a stable BUSY-clear sample.
An initially idle scheduler skips that transaction, as the vendor does.
SOURCE(esp32s31): libbtdm_common[18.o]::r_sym_bt_74l62ZLsZuXg67pPHSd7;
libbtdm_common[18.o]::r_sym_bt_Ak3CRkSbyZRhUlneqclG;
libble_app[scan_2.o]::brk_sym_scan_fd4yzdFac8vdFTlQ4BPA.

Command readiness permits 10,000 attempts: the vendor increments its counter
then compares with 9999. BUSY permits 65,536 failed samples, the vendor's
0x10000 counter threshold. The vendor asserts and continues; the bounded
implementation returns `Unconfirmed`, omitting the vendor's extra sample
before its BUSY assertion. Diagnostic-pair stabilization uses the existing
finite diagnostic budget. The CPU join's 10,000 observations are an agent
policy using the command ceiling, not a vendor guarantee about ISR latency.
SOURCE(esp32s31): libbtdm_common[18.o]::r_sym_bt_B8fDByebTHuwRe8FZuRt;
libbtdm_common[18.o]::r_sym_bt_74l62ZLsZuXg67pPHSd7.

`Stopped` confirms the bounded scheduler observation. `Unconfirmed` names the
failed join, diagnostic sample, command wait or BUSY wait. Both permanently
quarantine all static controller-visible storage; neither can reunite the
ordinary owner or authorize DMA reuse. The runtime publishes permanent
poisoning and the fail-stop proof carries the hardware outcome; installation
is refused until chip reset. Dropping or forgetting
the driver cannot free the `Pin<&'static mut ...>` backing. An unconfirmed
installation can still have active hardware or a previously entered handler.

Emergency stopping acquires no `SharedRadioLease` and touches no common
reset, PHY membership, coex configuration or modem clock release. Recovery
without chip reset is a separate experimental path requiring hardware proof
of DMA completion and removal of the hardware BLE coex request. Blobray's
captured `coex_core_release` calls the indexed hardware-timer disable; that
helper clears enable in the timer's control word and acknowledges its status.
This observation does not establish that controller emergency shutdown
reaches it, that DMA writes ended, or that neighboring radios remain usable.
SOURCE(esp32s31): libcoexist[coexist_core.o]::coex_core_release;
libcoexist[coexist_hw.o]::coex_hw_timer_disable.
