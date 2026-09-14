# ESP32-S31 Bluetooth ownership

This crate owns chip hardware sequencing and affine radio publication states.
Portable HCI policy and LE Link Layer codecs live in
[`crates/protocols/bluetooth`](../../../../protocols/bluetooth/). Concrete Embassy waiting and session
execution live in the [runtime](../../../../runtime/embassy/esp32s31/bluetooth/),
and final storage and hardware composition live in
[integration](../../../../composition/esp32s31/embassy/bluetooth/).

## Choose a reading path

- Application integration starts at the composition's `BluetoothSystem` and
  `BluetoothRunners`; the [Embassy runtime](../../../../runtime/embassy/esp32s31/bluetooth/)
  explains how those finite owners are polled.
- Host command classification, response order, event masks and ACL credits are
  portable contracts in
  [`oer-bluetooth-hci`](../../../../protocols/bluetooth/hci/).
- Over-the-air PDU and role policy belongs to the portable
  [LE Link Layer](../../../../protocols/bluetooth/le/ll/). This crate lowers
  that policy into scheduler, DMA and interrupt mechanisms.
- The module table below is the chip API map. `FEATURES.md` links current
  limitations to the generated qualification view; it is not a second
  readiness inventory.

## Bounded LE peripheral lifecycle

One peripheral connection is a sequence of owner transfers, not a linear
"start then close" call:

| Stage | Completion and retained ownership |
| --- | --- |
| Initialization | Composition reserves final static storage, claims all memory graphs, enables clocks, initializes/registers the shared PHY client and publishes one controller actor. Failures after reservation or a physical transition retain a fail-stop slot; HCI Reset cannot recover the cold owner. |
| Connectable advertising | HCI configuration stays in the portable reset-scoped owner. Set Enable becomes a deferred start; command success waits until the chip runner proves scheduler `RUN`, rather than merely accepting the Host packet. |
| Connection admission | An accepted `CONNECT_IND` transfers the advertising graph into the first-event peripheral owner. A handle becomes Host-visible only with LE Connection Complete; six missed initial events produce status `0x3e` without allocating a handle. |
| Active recurrence | A scheduler reservation is only planned work. DMA-visible nodes and the IRQ/timer owner remain in the chip session through `RUN`, terminal completion, exact-head retirement and software unlink. A new event is not published when the guarded start or supervision deadline has already closed. |
| ACL flow control | Host-to-Controller input is owned through LL fragmentation, retransmission and acknowledgement; its HCI credit returns after acknowledgement. Controller-to-Host packets and connection events retain their exact owned buffers while the Host FIFO is full. Radio supervision and command intake continue to be polled, but a successor `RUN` is admitted only when complete RX-batch capacity is reserved. |
| Disconnect or Reset | Peer/local termination first reaches its protocol terminal condition, then completes hardware stop if necessary, retires the exact head, unlinks the scheduler item and restores the graph. Disconnection Complete may remain owned behind Host backpressure. Reset uses the same retirement disciplines and starts a fresh bounded HCI bootstrap; it is not powered teardown. |
| Reuse | Idle command intake returns only after unlink/recycle and ordered Host events. The sole connection handle is not reusable until every Host-owned Controller ACL buffer for the old connection has returned its credit. |

The 40-second per-event progress budget covers completion, unlink and the next
Controller-time acquisition; it is not the Link Layer supervision timeout.
If live hardware must be stopped, that stop has its own 100-millisecond budget.
An expired stop/unlink budget or identity mismatch seals the owner in
fail-stop state: protocol termination does not imply hardware recovery.

Dropping an Embassy wait only abandons that waiter. Durable notifications and
the controller actor retain the response, packet, reservation or active owner
until the corresponding state transition consumes it. Controller-to-Host
backpressure therefore delays publication without authorizing buffer or handle
reuse. Detailed scheduler, peripheral, HCI and timing contracts live beside
their implementations under `src/`; the sections below preserve the reviewed
chip-specific limits and provenance.

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
`LL_FEATURE_RSP`; the used-feature octet intersects the peer mask with the
implemented LE Encryption, Peripheral-initiated Feature Exchange and LE Ping bits. Exact
`LL_PING_REQ` input queues one `LL_PING_RSP`; an incoming response cannot create
another exchange. Connectable `ADV_IND` also sets local ChSel support, and the
accepted connection uses CSA#2 only when the initiator sets ChSel as well. Host LE Read Remote
Features publishes Command Status before `LL_PERIPHERAL_FEATURE_REQ`, caches a
successful page-zero result for the connection, reports `LL_UNKNOWN_RSP` as
Unsupported Remote Feature and bounds the procedure with `connProcedureTimeout`.
Unsupported optional control requests receive `LL_UNKNOWN_RSP`. Two software
responses may wait behind one controller TX packet; that packet remains queued
until descriptor completion, including across recurring preparation cancellation.
The current TX descriptor remains pinned after its payload is reclaimed.
Connection RX retains its current descriptor and packet and rearms the other
allocation as one writable successor for each recurring event.
Version exchange queues at most one `LL_VERSION_IND` per connection and requires
an explicit `with_version_information` identity in the connection runtime config.
Host Read Remote Version Information publishes Command Status before a local
request, shares the response deadline with Feature Exchange, caches the peer
identity and retains the completion across Host backpressure. A missing local
identity rejects the Host command without inventing a Controller version.
The shared 40-second procedure deadline restarts whenever another LL Control
PDU enters the connection TX graph while either local procedure remains active.
The HIL image uses development company value `0xffff`, Core 5.4, subversion 1;
it does not report the vendor Controller identity.
The source-backed peripheral HIL workload asks the Linux central to issue both
Host commands and requires each successful Command Status before an exact
correlated completion. It checks feature mask `18:40:00:00:00:00:00:00` and
the development version identity above; no hardware run has recorded this
interoperability yet.
Peer termination retires the connection after event completion and scheduler
unlink. It cancels pending TX payloads, restores the exact graph and RX pool,
and returns to idle command intake after any earlier HCI response and the
Host-enabled Disconnection Complete event are published. The first event which
observes peer activity produces LE Connection Complete for the single supported
handle. Both events retain their exact packet across HCI queue backpressure;
radio completion continues independently while they wait. A successor RUN is
admitted only after the Controller-to-Host FIFO reserves space for the complete
two-node hardware RX batch. The CPU-owned completion boundary races real Host
capacity with periodic Controller-time checks, so supervision and active LL
procedure deadlines remain live while Host credits are exhausted. This also
ensures every packet which hardware can acknowledge has software storage before
RUN; mixed data/control batches are never parked behind an older full FIFO.
Before establishment, each missed event retains the entire initial transmit
window plus clock widening. After six events without a peer packet, the closed
connection publishes failed LE Connection Complete with status `0x3e` and no
allocated handle before restoring idle command intake.
Established-link supervision uses the hardware valid-RX timestamp, seeded
with absolute creation time. Anchor capture and delivered RX count do not
extend this deadline. A fresh controller-time check before the next RUN
expires the connection with reason `0x08`; a reservation starting at or beyond
the deadline waits without publication. Retirement releases that reservation
and restores the unlinked allocation. If executor latency closes an established
event's guarded start before RUN publication, the unpublished reservation is
cancelled and rebuilt directly at a later event counter with accumulated clock
widening. Recovery cannot cross an update instant; that closes the link with
reason `0x28`. The treatment of an initial establishment window never submitted
to radio remains an explicit policy gap. Abrupt RF-loss and CRC-error behavior
remain unqualified on hardware.

Every published peripheral event owns an absolute 40-second progress budget
covering completion, unlink and any following Controller-time acquisition. A
lost scheduler completion wake is covered by periodic absolute-time rechecks.
If RUN remains live at expiry, the owner executes the common hardware stop
sequence, validates and retires the exact head, then uses the normal software
unlink and recycle path. The stop itself has a 100-millisecond budget; a stalled
Controller-time request, stop/unlink expiry or identity mismatch remains a
sealed fail-stop.

Central Connection Update and Channel Map Update are validated, retained and
applied at their exact wrapping connection instants. Connection Update shapes
the instant event from the old interval, new offset and new window, resets the
supervision basis, and publishes LE Connection Update Complete when Host-visible
parameters change. A passed instant or incompatible instant procedure enters
acknowledged protocol termination; peer protocol errors do not enter hardware
fail-stop ownership. The source-backed peripheral recovery HIL scenario requests
an exact 120-ms interval and an exact two-channel map, requires matching
successful update completion at both Host boundaries, and completes a second
fragmented ACL round trip after the map applies; hardware evidence has not yet
been recorded. Host Disconnect and bidirectional ACL are composed.
The local-disconnect HIL source keeps the requested `0x13` reason on air and
requires local Host reason `0x16`; the local-reset source requires peer
supervision timeout and a fresh bounded HCI bootstrap before reconnect. Neither
has qualifying hardware evidence and HCI Reset is not powered teardown.
The RF-loss HIL source verifies that the Linux central is rfkill-blocked for at
least 2500 ms, then requires target supervision timeout and reconnect. It has
no recorded hardware evidence yet.
Host-to-Controller packets use legacy LL fragmentation and return their credit
after acknowledgement. Unencrypted packets carry at most 27 payload octets;
encrypted packets carry at most 23 plaintext octets plus the four-octet MIC.
Accepted peer LL
Data fragments enter a two-packet Controller-to-Host FIFO after Connection
Complete, fragment to the Host-declared ACL buffer length and retain HCI
transport backpressure. Host Number Of Completed Packets restores credits for
the sole live handle; command intake remains live while a Host ACL packet is
retained. Reset retires directly from the CPU-owned backpressure boundary;
Disconnect and locally initiated procedures may fill the retained TX graph
under a fresh Controller-time sample while recurrence awaits safe RX capacity.
After disconnection, idle restoration and handle reuse wait until all
Host-owned Controller ACL buffers return their credits. Only contiguous
connection events are scheduled. Local termination uses a fresh Controller sample immediately
before the first `LL_TERMINATE_IND` enters the TX graph and stops after its
acknowledgment or one connection supervision timeout. Generic LL procedure
response timers remain absent. Data Length Extension remains unavailable. Full
LLCP and ACL qualification remain open.

The portable LL security owner implements the Peripheral encryption
start/pause/restart sequences and software AES-CCM with independently owned
39-bit direction counters. The production composition supplies fresh SKDp and
IVp from the ESP32-S31 hardware RNG. A peer encryption request publishes the
standard masked LE Long Term Key Request event; the closed active classifier
accepts the positive or negative HCI reply only for that live request and
handle. An accepted new packet advances its counter once, while a radio
retransmission reuses the retained ciphertext. RX authentication and decryption
precede ordinary LL dispatch; TX control, ACL and empty acknowledgement packets
are encrypted while the session is active. Restart replaces session material
and resets both counters only after the encrypted pause response and the peer's
unencrypted response, then publishes Encryption Key Refresh Complete. Initial
start publishes Encryption Change. MIC or physical-channel sequence failure
retires the link with `0x3d`; a missing restart LTK sends reliable termination
reason `0x06`. Reset, disconnection and fault owners retain and destroy the
session keys with the connection. LE Encryption is advertised, and the two LTK
reply commands are present in Read Local Supported Commands. Pairing, SMP and
key persistence remain Host responsibilities. The software path does not use
the reviewed BLE encryption accelerator. Vendor comparison and on-air
interoperability/fault evidence remain absent.

The bootstrap `Read Local Supported Commands` response publishes the exact
closed command inventory used by production classification. State-dependent
commands such as Disconnect and Host Number Of Completed Packets remain in the
bitmap because the active connection owner implements them; unsupported
commands sharing their octets remain clear.
