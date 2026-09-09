# PHY tracking contracts

See the [PHY architecture](../../README.md) for initialization, runtime
terminology and the ownership relationship with protocol runtimes and coex.

This module owns the source-derived tracking algorithms and read-only demand.
It does not grant RF access, stop protocol runtimes, or implement coexistence.
The registered PHY and physical-radio owner must remain coupled throughout
execution. A network queue credit, scheduler pause or CPU mutex alone is not
permission to mutate the shared PHY.

## Inspecting conditions

The registered radio and Wi-Fi owners expose `inspect_tracking(now_micros)`.
The [inspection contract](inspection/README.md) separates evaluation deadlines,
retained thermal conditions and physical admission. It reuses the executor's
predicates and does not replace its ordered, consuming decision process.

## Demand and execution

`PhyClientSnapshot::tracking_schedule_at(now_micros)` returns
`schedule::Schedule`:

- `Inactive`: no client requires a timer.
- `At(t)`: arm an absolute timer for the first due microsecond.
- `Due(demand)`: work is outstanding; hardware admission is still required.

Observation does not update source timestamps, temperature references or
calibration validity. Repeated observations after deferral retain the same
`due_since_micros`; they do not create additional execution obligations. This
instant reflects the source interval, not a measured maximum safe deferral.
BT and IEEE 802.15.4 share one tracking class but remain distinct clients.

Demand is copyable observation, not an ownership token. Re-observe the current
client set after waiting for admission: a client may have stopped while the
request was deferred. Do not execute an old copied demand as an RF grant.
The consuming `evaluate_immediate_tracking` transition retains its original
sample order and timestamp updates. It runs only when the caller commits to
execution; it must not be used merely to ask whether work is due. Its pending
owner remains pending until the selected target children complete.

`RegisteredPhyRadio::wait_for_tracking_demand` borrows the registered owner
while an absolute timer is pending. It returns demand without creating a
pending hardware transaction. Cancelling this wait or observing a clock error
leaves the physical owner intact; a later client release remains possible.
Cancellation after starting the consuming target operation is a different
boundary and still requires the documented failure/reset handling.

The Wi-Fi supervisor uses this read-only observation at completed role
boundaries. Its existing `WifiStopped::maintain_phy` path requires stopped DMA
and an inactive IRQ owner. A standalone connected station supports a MAC/RX/IRQ pause round trip,
including an explicit HIL request. This round trip withdraws the runtime
register owner from its arena, admits and releases PHY access, then republishes
the owner into the same arena before RX/IRQ resume. An explicit tracking request
consumes the registered PHY/platform owner while this access is held, reobserves
due work and restores MAC stop before release.
The station receive-policy snapshot must remain unchanged; RX resume separately
checks its saved descriptor cursor. Automatic periodic requests are not enabled.
Joint Wi-Fi/BLE/IEEE 802.15.4 grants are not implemented.

## Exclusive Wi-Fi access

HAL `owner::maintenance::WifiAccess` retains the existing
`RadioRuntimeOwner` and a closed interrupt-authority type: `MacInterruptSetup`
for a terminated epoch or `MacInterruptCheckpoint` for a paused one. It does
not create another PHY owner. `try_into_phy_maintenance` consumes those resources and checks inactive
MAC and disabled RX walker before exposing PHY operations. A failed admission
returns the original resources with an explicit reason and makes no writes.
The Wi-Fi composition treats a failed stopped precondition as a fault.

The checkpoint holds both returned MAC and WDEVPWR capabilities without
peripheral writes. The ESP-HAL route owns CPU detachment and same-core resume;
HAL owns the checkpoint representation. Checkpoint completion preserves the
interrupt-owner type: checked maintenance release cannot turn a paused route
into a cold setup. A timer request cannot implement the sealed
`InterruptAuthority` contract.

`RegisteredWifiPhy::maintain` consumes this access together with the registered
PHY across all awaits. An error retains both in an opaque failure; cancellation
does not return either owner. Success returns access for restoration, then
`try_release` rechecks MAC and RX walker before releasing register and IRQ
resources. Restoration failure retains the physical access and requires reset.
The driver distinguishes admission, PHY execution and restoration failures.

`WifiRoleOwner::maintain_phy` consumes the active role's logical PHY/platform
owner with already admitted physical access. It retains channel and startup
context and returns the same interrupt-authority type. It leaves restoration
to the paused role: the terminal `WifiStopped` path's disabling of both RX
policies is not appropriate for preserving an active station. The IRQ runtime's
`PausedInterruptEpoch::try_with_authority` retains the detached route and its
pending work while an owned operation holds the checkpoint. Failure and
cancellation expose no resumable epoch.

MAC restoration belongs to the Wi-Fi execution boundary. The terminal stopped
path disables both role receive policies and requests MAC stop after tracking.
The paused connected path preserves those policies and waits for MAC stop before
checking release. HAL checks the resulting hardware state; it does not silently
stop a live descriptor epoch to make admission succeed. The outer composition
must retain idle TX resources and either stopped RX ownership or the paused RX
owner with all outstanding leases preserved. Readback is not a substitute for
those ownership transitions.

The access contract is specific to the exclusive Wi-Fi route. The consuming
radio root makes other protocol owners unavailable; an application must not
infer this authority merely from a Wi-Fi client count or PTI value. This
exclusive route needs no invented coex acknowledgement. A future joint-radio
composition needs its own qualified admission contract, and cannot reuse this
permission while other physical clients remain active.
Coexistence timer programming does not certify a grant, and no equivalent
BLE/IEEE 802.15.4 maintenance admission is exposed. Low-level `SharedPhyHal`
continues to be a narrow register borrow, not an independently valid runtime
maintenance permission.

## Operations and hardware effects

The following effects are implemented source operations. They are not a list
of operations qualified for concurrent radio use. Child operations are
ordered inside the outer transition; completing one child does not release
the outer owner's exclusive access.

| Operation | Selection and effects | Commit / restoration boundary |
| --- | --- | --- |
| [Outer parameter tracking](parameters.rs) | Registered policy selects RFPLL, BT/154 power and calibration, Wi-Fi I2C/power/calibration, and temperature children. Inhibited policy skips hardware children. | Critical-section actions bracket the graph. The current target port acknowledges them logically; the caller supplies actual exclusion. Inhibited completion does not mean calibration data was refreshed. |
| [Power compensation](power.rs) | Computes a temperature decision; changed enabled gain publishes the selected class's gain through BBPLL calibration enable/disable edges. | Gain/temperature outcome follows the child and BBPLL tail. An unchanged decision does not publish gain. Live gain publication atomicity is not qualified. |
| [Wi-Fi I2C tracking](i2c.rs) | A temperature-band change executes two bounded masked analog-I2C writes. | The new band is committed only after both writes. No band change means no writes. The analog bus must have one transaction owner. Safe overlap with active TX/RX is not established. |
| [Temperature sampling](../analog/temperature.rs) | Reads PHY-I2C range and sensor code; may also write a new range. This is not an unconditional read-only operation. | The sensor transition carries matching completions. Concurrent sensor/range access is not qualified. |
| [RFPLL capacitor tracking](../analog/rfpll.rs) | Optional `RfpllCapTrackingTransition`: disables hardware frequency control, applies selected capacitor correction through analog I2C/memory, then enables hardware frequency control. This child is distinct from full `RfpllFrequencyTransition`. | The capacitor child and control tail precede completion. This does not certify RF lock, uninterrupted reception or a bounded exclusive radio interval. Continuous-radio execution is not qualified. |
| [Common calibration](calibration.rs) | A temperature threshold selects DCODE, RX-gain calibration and channel restoration. | The branch restores MAC baseband and TX gain compensation before committing the common reference or entering class calibration. |
| [Class calibration](calibration.rs) | Wi-Fi or BT/154 threshold selects software frequency control, forced gain/TX-RX state, TXDC/PWDET and class gain publication. | The graph restores its baseband/control and gain-compensation state. This is not proof that every protocol's MAC, key, TSF or DMA context is unchanged. No selected branch means no hardware actions in this child. |

Full cold registration and cache replay belong to
[calibration/registration](../calibration/registration.rs), not the periodic
scheduler. A serializable calibration snapshot does not certify hardware
restoration.

`PhyCalibrationTrackingParameters::decision(request)` is the single read-only
thermal decision used by the calibration transition. It exposes independent
common and selected-class TX demand, their reference samples, absolute deltas
and thresholds in sensor units. Equality is due; a zero diagnostic threshold
forces both branches. It does not commit references, certify sample freshness
or provide RF authority. The common branch can obtain a new temperature during
channel restoration. TX demand is evaluated again after that restoration and
gain cleanup, so it can become due or cease to be due. An initial decision is
not an immutable list of jobs for an entire maintenance pass.

```mermaid
flowchart TD
    retained[Retained temperature and independent references] --> common{Common demand due?}
    common -- yes --> rx[DCODE and RX gain calibration]
    rx --> restore[Restore channel and obtain temperature]
    restore --> cleanup[Restore baseband and gain compensation]
    cleanup --> tx{Reevaluate selected TX demand}
    common -- no --> tx
    tx -- yes --> transmit[TXDC/PWDET and gain publication, then cleanup]
    tx -- no --> outcome[Return typed calibration outcome]
    transmit --> outcome
    outcome --> owner[Parent accepts completion and commits references]
```

## Timing observations

[Observation](observation.rs) names outer tracking calls and expensive
calibration steps. The executor emits `Started` before awaiting the port and
`Completed` only after accepting its completion. Port errors or rejected
completions emit `Failed`. Cancellation emits no fabricated terminal event.
Default observers require no clock or storage.

The allocation-free recorder accepts caller-provided monotonic timestamps.
It retains attempt/completion/failure counts, total terminal duration and
maximum duration per operation. Nested durations are inclusive: parent and
child totals must not be summed as exclusive CPU or RF time. Clock reversal,
overflow or malformed event pairing marks timing invalid. A selected call can
complete without performing calibration; committed branch flags retain that
separate meaning.

DCODE, RX gain and TX DC/PWDET also expose poll count, total time inside polls,
maximum poll duration, the number of Pending returns, and total/maximum gaps
from a Pending return to the following poll entry. Each completed child has
one final Ready poll; cancelled children cannot manufacture that completion.
Gaps are measured within each invocation, never between successive invocations.
The target wraps the actual child future without
adding wakes, polls or timers. These intervals include interrupts and observer
overhead, so they are not CPU cycle measurements. Operation time minus poll
time includes suspension, executor scheduling and outer transition overhead.
The measured Pending-to-poll gaps exclude the initial poll-entry delay and
outer transition overhead, but do not reveal when the awaited hardware or
timer actually became ready. Neither these gaps nor the remaining time may be
labelled requested hardware delay or CPU idle time. The recorder
and HIL validator reject malformed intervals instead of accepting them as a
successful measurement. Other operations have no poll breakdown.

The Wi-Fi Embassy adapter interprets `PhyAsyncDelay` as a minimum elapsed
duration. It captures an absolute deadline when creating the delay and checks
it before polling the Embassy timer. An already elapsed deadline completes
without registering an extra wake; a future deadline uses ordinary timer wake
registration. There is no busy wait, and the target's hardware edge limits and
settling durations remain unchanged. This is not a cooperative execution budget:
several completed waits may permit more hardware steps in one poll. The maximum
poll observation must therefore be considered independently of total pause time.
Bluetooth owns its separate time binding.

Diagnostic connected Wi-Fi keeps the recorder beside the parked runtime,
outside nested PHY futures. Callbacks borrow it briefly, with no borrow across
a wait and no formatted output. Successful pause reports include a snapshot.
Physical failures return their existing failure stage without a timing
snapshot. Diagnostics-off images have no timing observer.

## Physical ownership and errors

The existing stopped Wi-Fi path is a conservative admission contract for that
composition. It does not establish that all tracking operations physically
require disabling MAC, DMA and IRQ, nor that a smaller exclusion scope is
safe. No active-radio operation is admitted based only on its name or on a
similar driver for another chip.

A CPU critical section does not stop DMA. The async target executor must not
hold a CPU critical section across timer waits. The outer owner instead
retains the required hardware exclusion until terminal completion or failure.
A failed or abandoned tracking epoch cannot yield a ready owner without the
required reset/recovery.

A PHY operation may restore baseband enable bits. The caller must establish
its required postcondition after execution before publishing descriptor or
interrupt ownership. Existing `RxRingHalted` teardown rebuilds a new descriptor
epoch; it is not a transparent pause for outstanding upper-layer RX leases.

Source timestamps record the scheduler's execution request. They are not
independent evidence of RF quality, completed calibration branches or an
externally granted radio interval. Temperature-reference commits and typed
child outcomes remain the algorithm's completion authority.

`PhyParamTrackingOutcome::calibration` reports only committed common and
per-class calibration branches. Invoking a calibration child below its thermal
threshold leaves the corresponding flag false. Wrong-class or incomplete child
completions cannot publish this progress. These flags do not measure RF quality.

`WifiPhyMaintenanceRequest::Calibrate` selects a zero calibration threshold for
one due maintenance pass. It uses the existing common and Wi-Fi child graph
with real sensor readings; the stored policy and deadline remain unchanged.
It is an explicit diagnostic operation, not automatic periodic recalibration.
An early call still returns no work. Completion flags must confirm both branches
before a caller claims that calibration executed.

During a connected pause the non-runnable datapath remains in permanent cold
storage beside its worker mailbox while PHY futures execute. Only the restored
arena capability permits consuming this parked state. Failure or cancellation
retains it without hardware authority; no `RefCell` borrow spans an await.

Runtime Dcode passes an explicit delay factory through both its direct PHY-I2C
bindings and its nested RFPLL frequency transitions. The report keeps three
nonoverlapping groups: direct I2C, RFPLL I2C and explicit RFPLL settling/lock
delays. Each group retains completed wait counts, requested microseconds,
elapsed microseconds and maximum deadline lateness. I2C groups also count the
subset of waits caused by a busy bus at command start; remaining waits precede
transaction-completion reads. These counts are not hardware-ready timestamps.

The timer adapter supplies the same start/deadline used for its own wait.
Events run after polling the timer; the completion timestamp is sampled before
recorder work. This avoids making a one-microsecond wait expire merely by
recording its start before polling. The observer and its state are local to the
maintenance invocation; no global active-operation identifier or extra wake
source is used. Overflow, unsupported timing or unmatched events invalidate the
report. The diagnostic count bound does not change hardware retry limits.

PLL locked/unlocked counters observe existing analog `RFPLL_LOCK_STATUS`
read completions in Dcode's nested full frequency-calibration path, without
additional register reads. They are observations at sampling time, not the
moment lock was acquired or lost. They do not describe the fast channel-switch
path. The standalone tracking RFPLL operation timer remains distinct from
RFPLL work nested in Dcode.

Runtime TX DC/PWDET reports five separate timer groups: PBus, search settling,
tone arming, SAR triggering and root setup/cleanup. PBus waits distinguish a
busy command start from waiting before a completion-edge read. SAR ready and
not-ready counters consume existing status completions. Repeated not-ready
samples are synchronous polls in the finite calibration executor, not timer
waits; these counters reveal that work without adding reads or changing retries.

RX gain and cold registration retain unobserved delay factories. Diagnostics-off observers do
not request deadline measurements. Lateness includes delay-future construction,
executor resumption, timer polling and timestamp sampling; it is not a pure
scheduler-latency measurement. Instrumentation still has a cost, especially
for microsecond waits. Wait times lie inside the enclosing operation/poll/gap
intervals and must not be added to those parent totals.
