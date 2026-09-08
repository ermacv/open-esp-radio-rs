# ESP32-S31 Bluetooth ownership

This crate owns chip hardware sequencing and affine radio publication states.
Portable HCI policy and LE Link Layer codecs live in
[`crates/protocols/bluetooth`](../../../../protocols/bluetooth/). Concrete Embassy waiting and session
execution live in the [runtime](../../../../runtime/embassy/esp32s31/bluetooth/),
and final storage and hardware composition live in
[integration](../../../../composition/esp32s31/embassy/bluetooth/).

| Module under `src/` | Responsibility |
| --- | --- |
| `le/dtm` | Direct Test Mode commands, payloads, event timing, scheduler reservations and active/stopping transitions |
| `le/advertising/legacy` | Legacy advertising preparation, timing, completion and recurring execution |
| `le/advertising/connectable` | Connectable advertising activation, completion and recurring sequence/HCI/state |
| `le/scanning/passive` | Passive scanning activation, recurring execution and completion |
| `le/peripheral` | First HCI handoff, connection owner, completion and contiguous active recurrence |
| `scheduler` | Shared scheduler resources and single-item completion |
| `controller` | Shared controller bootstrap and hardware lifecycle |
| `phy` | Common PHY power/readback, registration, Bluetooth-client acquisition and initial tracking |
| `controller/hci` | HCI queue binding to the published controller epoch |
| `interrupt` | Chip interrupt state and hardware handling |
| `memory` | Controller-SRAM storage owners and completion values from the memory crate |

Public paths identify their owners: `controller`, `interrupt`, `scheduler`,
`resources` and `le::{dtm, advertising, scanning, peripheral}`. Internal event
and transition implementations stay private within those namespaces.
Publication, cancellation, reset and quiescence remain
explicit lifecycle terms. Shared controller/IRQ/scheduler code is not owned by
one LE role.

Boot/controller loops retain their state owners through hardware handoff,
waits and terminal quarantine. Feature gates apply to both the owners and
their unit suites in adjacent child files.
The separate [`memory`](memory/) crate retains controller-SRAM codecs.

Controller time uses the HAL period's software conversion selector. The
standalone profile has two raw ticks per microsecond; its hardware divider is
a separate setting. Scheduler epoch updates retain fractional raw ticks, and
earlier fractional timestamps round down before deadline projection.

DTM recurring reception uses the vendor's 85-microsecond role setup lead,
plus its 15-microsecond recurrence lead. This is separate from the common
40-microsecond scheduler admission guard. Production adds a 500-microsecond
preparation reserve for the open runtime with code/data in PSRAM, retaining
the 1-ms receive window. This reserve is an open implementation policy,
not a recovered vendor constant. Each completed preparation phase
allows one immediate observation of the next time request; only an observed
busy request waits for a cooperative recheck. The optional `dtm-diagnostics`
feature exposes cumulative RX recycle counters through
`le::dtm::diagnostics::snapshot()`: successful/failed events, the last opaque
failure status, empty events, counted packets, rejected packets and recurring
sequence checks/rejections with the last signed start lead in raw ticks. Snapshots
are read-only and do not acknowledge hardware or claim RF qualification.

The same diagnostic feature exposes advertising RX progress through
`le::advertising::diagnostics::snapshot()`. It observes the retained packet
nodes after scheduler removal and before recycling: event count, scan/connect
header counts, progress without header completion and the last nonempty
observation. These are SRAM observations; changed producer fields alone do not
prove a received PDU, and a connection header does not prove LL admission.

Test End and Reset share active scheduler cancellation. The finite sequence
masks dynamic scheduler interrupts, disables the run source, waits for the
reviewed command preamble, publishes the lifecycle request once and waits for
BUSY to clear. Stop waits use timed rechecks because their interrupt sources
are masked. Cancellation, stop and unlink share one absolute 100-ms budget;
expiry retains the complete owner in a fault state, without reclaiming memory
or reporting successful command completion. A stopped in-flight item is marked
`Aborted` in software and contributes no received packet. A real completion
racing stop retains its hardware status. Exact head retirement, software unlink
and memory/timeline release still precede the HCI response. The HIL scenario
checks repeated quiet RX/Test End and RX/Reset/RX; hardware qualification of
this stop path is pending.

`initialize_common_phy` executes the shared modem/PHY prerequisite before
registration: reset release, power-state clock maps, frontend/calibration
clocks and the 160 MHz PHY-I2C source are checked through semantic readback.
The task owner retains the I2C clock lease. `PhyInitializationError` separates
power-checkpoint failures from registration failures; both preserve fail-stop
ownership and prevent cold reunion until physical teardown is available.

See [FEATURES.md](FEATURES.md) for supported and incomplete paths; structural
organization does not extend hardware qualification.

The recurring peripheral lifecycle dispatches accepted connection RX packets
through the portable bounded control responder. Central `LL_FEATURE_REQ` receives
`LL_FEATURE_RSP` with no optional feature bits, consistent with bootstrap HCI.
Unsupported optional control requests receive `LL_UNKNOWN_RSP`. Two software
responses may wait behind one controller TX packet; that packet remains queued
until descriptor completion, including across recurring preparation cancellation.
The current TX descriptor remains pinned after its payload is reclaimed.
Connection RX retains its current descriptor and packet and rearms the other
allocation as one writable successor for each recurring event.
Version exchange queues at most one reply per connection and requires an
explicit `with_version_information` identity in the connection runtime config.
The HIL image uses development company value `0xffff`, Core 5.4, subversion 1;
it does not report the vendor Controller identity.
Mandatory connection updates, graceful peer termination and
ACL delivery are not implemented by this responder; mandatory transitions enter
fail-stop ownership. This is not full LLCP or ACL qualification.
