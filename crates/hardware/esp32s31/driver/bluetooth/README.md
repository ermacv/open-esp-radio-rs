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
| Runtime ownership | The first runtime split grants the command actor an exclusive lease of task-side HAL and its controller-time worker. Software queues remain borrowed from stable storage. Successful HCI retirement moves the actual task hardware out of its slot; rejection retains it for the same actor. A repeated split rejects before touching HCI. The same barrier extracts the registered PHY client, calibration cache and BLE PHY/DF allocation graph through a second exclusive lease. A separate platform lease leaves command states unchanged and joins only its own retired HCI epoch before extracting the reservation. A third lease groups all five role-memory owners; their complete idle state and absence of controller-time requests/orphans/faults are checked before closing HCI and extracting any owner. Extraction does not revoke hardware pointers, run platform Drop or release PHY. |
| Connectable advertising | HCI configuration stays in the portable reset-scoped owner. Set Enable becomes a deferred start; command success waits until the chip runner proves scheduler `RUN`, rather than merely accepting the Host packet. |
| Connection admission | An accepted `CONNECT_IND` transfers the advertising graph into the first-event peripheral owner. A handle becomes Host-visible only with LE Connection Complete; six missed initial events produce status `0x3e` without allocating a handle. |
| Active recurrence | A scheduler reservation is only planned work. DMA-visible nodes and the IRQ/timer owner remain in the chip session through `RUN`, terminal completion, exact-head retirement and software unlink. A new event is not published when the guarded start or supervision deadline has already closed. |
| ACL flow control | Host-to-Controller input is owned through LL fragmentation, retransmission and acknowledgement; its HCI credit returns after acknowledgement. Controller-to-Host packets and connection events retain their exact owned buffers while the Host FIFO is full. Radio supervision and command intake continue to be polled, but a successor `RUN` is admitted only when complete RX-batch capacity is reserved. |
| Disconnect or Reset | Peer/local termination first reaches its protocol terminal condition, then completes hardware stop if necessary, retires the exact head, unlinks the scheduler item and restores the graph. Disconnection Complete may remain owned behind Host backpressure. Reset uses the same retirement disciplines and starts a fresh bounded HCI bootstrap; it is not powered teardown. |
| Reuse | Idle command intake returns only after unlink/recycle and ordered Host events. The sole connection handle is not reusable until every Host-owned Controller ACL buffer for the old connection has returned its credit. |

Event completion is bounded by the admitted sequencer interval. Stop, unlink
and each fresh Controller-time acquisition have separate 100-millisecond
budgets, retained across rechecks and cancelled waits. Host-credit backpressure
does not consume a future acquisition's budget. These progress bounds are
independent of Link Layer supervision and procedure deadlines; see the
[peripheral timing contract](#peripheral-timing-and-recovery).

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
| `le/peripheral` | First HCI handoff, connection owner, completion and contiguous active recurrence; private HCI order retains the shared first/active response epoch, active HCI coordinates endpoint intake/publication, and active lifecycle steps normal/Reset radio retirement |
| `scheduler` | Shared scheduler resources and single-item completion |
| `controller` | Shared controller bootstrap and hardware lifecycle |
| `controller/boot/publication` | Atomic IRQ-owner publication, lossless rejection, stable interrupt service and the one-time hardware/HCI endpoint split |
| `controller/boot/modem_timer` | Source-127 task state, borrowed readiness and timer-owner exchange with stable ISR storage |
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

The [composed peripheral host tests](src/scheduler/core/peripheral_connection/recurring/transaction/tests/lifecycle.rs)
drive the production completion spine, LL recurrence, ACL owner, HCI credit
transport and maintenance/deadline admission together. They cover delayed and
unfinished scheduler wakes, post-unlink waits, saturated ACL queues, Instant
protection, cancelled maintenance proposals, clock wrap and late credit returns
after disconnection. Run them with `cargo test -p oer-esp32s31-bluetooth lifecycle`.
Hardware observations are scripted; these tests do not execute the RISC-V
active-session coordinator or establish physical DMA/IRQ quiescence. Those
boundaries require target evidence independently.

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
ownership and prevent cold reunion until the physical release transition completes.

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
Connection RX has three physical packet nodes. It retains the hardware current
descriptor and packet and rearms the other two as writable successors. Both the
first event and every recurrence admit at most two packets, matching the ACL
reservation. A control packet with MD set therefore does not consume the only
receive slot before a following `LL_TERMINATE_IND` in the same event.
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
correlated completion. It checks feature mask `19:40:00:00:00:00:00:00` and
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
### Peripheral timing and recovery

Established-link supervision uses the hardware valid-RX timestamp, seeded
with absolute creation time. Anchor capture and delivered RX count do not
extend this deadline. A fresh controller-time check before the next RUN
expires the connection with reason `0x08`; a reservation starting at or beyond
the deadline waits without publication. The shared
[protocol deadline gate](src/le/peripheral/deadlines.rs) checks termination,
procedure and supervision expiry before waiting for any future deadline.
For plaintext maintenance recovery, a fresh hardware valid-RX timestamp inside
the completed event's actual window also proves reception when empty or duplicate
PDUs have no delivered payload. A stale timestamp or scheduler anchor cannot
release recovery. This path supplies no initial or Instant acknowledgement and
is unavailable during encryption or its transitions, where packet validation
remains required.

The same gate runs while Controller-to-Host ACL delivery is backpressured.
Retirement releases the reservation and restores the unlinked allocation.
If executor latency closes an established
event's guarded start before RUN publication, the unpublished reservation is
cancelled and rebuilt directly at a later event counter with accumulated clock
widening. Recovery cannot cross an update instant; that closes the link with
reason `0x28`. The treatment of an initial establishment window never submitted
to radio remains an explicit policy gap. Abrupt RF-loss and CRC-error behavior
remain unqualified on hardware.

Each peripheral event retains a platform-clock completion deadline from its
fresh sequence sample through preparation, publication and RUN. The raw
remaining duration ends at the reservation end plus its sequence lead, matching
the sequencer's shifted start and full reserved duration. Conversion rounds up
fractional microseconds and does not equate Controller and platform epochs.
Sample-delivery and executor latency mean this watchdog is not proof of exact
protocol-deadline enforcement; supervision and procedure admission remain
separate checks.

Periodic absolute-time rechecks cover a missing scheduler wake. At expiry the
owner allows one final readiness observation, so an already-delivered completion
can advance; repeated empty polls cannot renew that allowance. A still-live RUN
then enters the common hardware stop, exact-head retirement, unlink and recycle
path. Stop, entry into post-unlink waiting and each new Controller-time request
start their own finite 100-millisecond budgets. Clock regression, arithmetic
exhaustion, operation expiry or identity mismatch preserves sealed fail-stop
ownership. Protocol termination never proves physical recovery.

Central Connection Update and Channel Map Update are validated, retained and
applied at their exact wrapping connection instants. Connection Update shapes
the instant event from the old interval, new offset and new window, resets the
supervision basis, and publishes LE Connection Update Complete when Host-visible
parameters change. A passed instant or incompatible instant procedure enters
acknowledged protocol termination; peer protocol errors do not enter hardware
fail-stop ownership. The source-backed peripheral recovery HIL scenario requests
an exact 120-ms interval and an exact two-channel map, requires matching
successful update completion at both Host boundaries, and completes a second
fragmented ACL round trip after the map applies. Peer Reset recovery accepts
remote-user termination or supervision timeout and retains the observed reason.
Host Disconnect and bidirectional ACL are composed.
The local-disconnect HIL source keeps the requested `0x13` reason on air and
requires local Host reason `0x16`; the local-reset source requires peer
supervision timeout and a fresh bounded HCI bootstrap before reconnect.
HCI Reset is not powered teardown; readiness is determined by qualification.
The RF-loss HIL source verifies that the Linux central is rfkill-blocked for at
least 2500 ms, then requires target supervision timeout and reconnect. Closing
the Linux user channel can terminate the connection before rfkill; that outcome
fails this strict timeout scenario and does not establish abrupt RF loss.
Host-to-Controller packets use legacy LL fragmentation and return their credit
after acknowledgement. One packet owns LL fragmentation at a time; further
packets retain their order in the existing HCI queue, which covers the declared
TX credits. Commands bypass blocked data without completing or discarding it.
The readiness wait uses the same capacity predicate, keeping command/credit
progress live without spinning on the data backlog. Unencrypted packets carry at most 27 payload octets;
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
precede ordinary LL dispatch; nonempty TX control and ACL packets are encrypted
while the session is active. Connection RX publication selects software-owned
control-PDU interpretation before exposing the RX head: ciphertext beginning
with `0x02` must not trigger the hardware's plaintext termination-opcode path.
The HAL reapplies this policy after PHY restoration and restores the default
policy when another RX role is published. Empty acknowledgements retain zero payload length,
carry no MIC and consume no encryption counter. The radio's ordinary empty TX
path also serves encrypted connections. RX empty acknowledgements neither
consume Host ACL credits nor advance the encryption handshake, including while
waiting for the Host LTK; they cannot revive a failed session. The
[`completion/security`](src/le/peripheral/connection/completion/security.rs)
boundary authenticates nonempty packets before dispatch and seals the session
on a MIC or forbidden-PDU failure. Diagnostic firmware can explicitly enable
`rx-fault-injection` to corrupt one copied active-data MIC before this same
authenticator. Its owner arms the request before admission and disarms it on
disconnect; the diagnostic snapshot counts actual injections. This optional
feature is absent from ordinary builds and does not simulate an RF error.
These rules follow Bluetooth Core Vol 6,
Part B [Data Physical Channel PDU and encryption procedures](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-54/out/en/low-energy-controller/link-layer-specification.html).
Restart replaces session material
and resets both counters only after the encrypted pause response and the peer's
unencrypted response, then publishes Encryption Key Refresh Complete. Initial
start publishes Encryption Change. MIC or physical-channel sequence failure
retires the link with `0x3d`; a missing restart LTK keeps ordinary data/control
blocked while admitting only the required reliable termination
reason `0x06`. Reset, disconnection and fault owners retain and destroy the
session keys with the connection. LE Encryption is advertised, and the two LTK
reply commands are present in Read Local Supported Commands. Pairing, SMP and
key persistence remain Host responsibilities. The software path does not use
the reviewed BLE encryption accelerator. The
[encrypted ACL HIL scenario](../../../../../hil/scenarios/bluetooth/bluetooth-peripheral-encrypted-acl.toml)
exercises fixed-key start, bidirectional fragmented traffic, connection/channel-map
updates, reconnect and cold release. The separate
[key-refresh scenario](../../../../../hil/scenarios/bluetooth/bluetooth-peripheral-key-refresh.toml)
requires a replacement LTK and a second echo on the same handle. Vendor comparison
and security-fault coverage remain incomplete; these scenarios do not establish SMP
or secure GATT readiness.

The bootstrap `Read Local Supported Commands` response publishes the exact
closed command inventory used by production classification. State-dependent
commands such as Disconnect and Host Number Of Completed Packets remain in the
bitmap because the active connection owner implements them; unsupported
commands sharing their octets remain clear.

### Physical Controller release

After HCI retirement, `try_release_controller_output` consumes the adapter's
post-route IRQ owner and checks actual scheduler inactivity, empty hardware
heads and primary fault status. A rejected transition retains hardware and
memory; it never clears a foreign head or fault to manufacture success.
`release_physical` requires the released output, drained timer and platform
reservation joined to the same HCI epoch. It releases the last Bluetooth PHY
client, executes the common target RF-close graph, powers down temperature,
resets the Controller/timer domains and restores clock leases and the captured
shared cold-power baseline. The complete operation returns `ControllerColdReleased`.

Keep the consuming future alive until a terminal result. Failures retain every
physical partition, SRAM allocation and the platform reservation. Successful
cold return keeps old software borrows in `ControllerRetiredStorage`; HCI stays
closed. `ControllerColdReleased::restart` uses the actual returned radio, original
BLE/DF allocations and original exclusive software leases for another powered
initialization. It restores both ISR owners atomically under the continuous
storage reservation; a second outer split or StaticCell claim remains forbidden.
A new HCI generation receives fresh bootstrap authority. Old Host commands,
event readers, ACL-credit senders and already-pending futures stay closed.
Failure retains its exact initialization frontier. `into_parts` is the terminal
alternative and exposes no software-storage reset.

### Quiescent PHY maintenance

`ControllerIdleCommandTask::maintain_phy` consumes the idle task, retired timer
and recovered unrouted IRQ bank, and borrows the matching platform and HCI
endpoint. Runtime workers, controller time, all five role allocations and
hardware BUSY/heads/faults must admit the window before shared-PHY access.
It evaluates the registered client's real deadline and runs the existing target
tracking executor only when due. A not-due result is explicit; inhibited
tracking remains visible in the returned outcome.

The idle owner can set the retained client's diagnostic tracking thresholds
through `set_phy_tracking_debug`. This changes the existing policy, not samples
or RF authority. The manual composition saves that policy and restores it after
successful maintenance; failed ownership remains sealed. A zero threshold
exercises the actual due calibration branch at the measured temperature.

Successful maintenance restores the same PHY owner and both original ISR
owners. HCI stays open, and counter epoch, software borrows, role generations
and memory publications remain unchanged. Host packets queued during the
window wait for the returned task. Failures retain an unrouted, non-runnable
frontier; cancellation requires external reset. The composition reanchors its
absolute recheck and binds routes only after this join succeeds.

The caller requests maintenance after returning the runner to idle. An optional
absolute tracking deadline is checked before execution and before ISR-owner
restoration; target execution also checks each poll and arms a deadline wake.
Failure retains the unrouted owner frontier, including an already-settled PHY
if the final time check fails. This rejects late tracking completion, not
blocking-poll duration or the complete routing/restoration interval; see the
[PHY timing contract](../../phy/src/tracking/README.md).

`prepare_peripheral_connection_maintenance_candidate` forms a provisional
budget-selected successor from the completed ACL graph through the ordinary
recurrence planner. Its LL owner retains initial acknowledgement, confirmation
of acknowledgement for pending Instant procedures, protected Instant events
and the requirement for a valid accepted packet before another deliberate
miss. RX activity alone does not release that requirement before packet
validation. A rejected or cancelled proposal preserves the original owner,
anchor correction and widening reference. This proposal supplies no PHY grant:
active control/encryption state, fresh deadline margins, physical quiescence
and restoration remain separate admission obligations.

`PeripheralConnectionActiveSession::begin_phy_maintenance` implements a separate
active handoff at the unreserved successor. It observes the retained PHY's due
schedule, keeps the complete HCI/control/encryption/ACL state and acquires fresh
Controller time. `PeripheralMaintenanceBudget` reserves execution, restoration
and protocol margin; acquisition latency consumes the available gap. The next
event's complete reservation and margin must precede every protocol deadline.
The natural gap is tried first; an explicitly force-eligible request may then
try successive LL-admitted candidates until execution and restoration fit.
All omitted events are checked against pending Instants. Planning consumes a
fixed transaction budget; previews cannot renew it. The selected candidate
preserves anchor progression, event counter, channel selection and window
widening. Another forced pause requires a valid accepted peer packet.
Cancellation before physical access rebuilds
the contiguous successor without spending skip credit.

`PeripheralPhyMaintenanceReady::maintain_phy` consumes that window and the real
unrouted IRQ/timer owners through the same physical executor as idle maintenance.
It checks that the checked-out connection graph and RX pool belong to the task,
while the other four roles remain idle. Success returns a distinct restoration
owner with no preview-cancellation edge. The original restoration deadline stays
with the candidate through admission, fresh time and RUN publication. Late
restoration seals ownership; neither ordinary missed-anchor recovery nor the
establishment fallback may add another miss. Budgets need measured platform
bounds, and the storage/PHY delay implementations must share one monotonic clock.

Successful guarded publication emits `PeripheralMaintenanceRun` with the
original admission and restoration deadline, actual post-RUN time and event
counter. This observation accompanies the returned running owner; it cannot
recreate one or prove a valid peer exchange.

The runtime's configured `run_with_phy_maintenance` entry now yields these
active owners automatically and restores the same actor after the physical
join. Its policy keeps original due, force-eligible and hard times distinct;
no retry or configured re-entry renews a deadline. Active DTM remains
non-preemptible. Hard expiry terminates the entire shared-PHY epoch: all RF
admission must cease, not just Bluetooth command service. The current S31
backend has no proven local active-RF shutdown, so it closes HCI and escalates
to full SoC reset without emulating Host Test End or waiting for the Host.
A future proven local shutdown may preserve non-RF work without changing this
shared-RF policy. Ordinary recovery cannot revive the failed epoch.

The [runtime maintenance contract](../../../../runtime/embassy/esp32s31/bluetooth/README.md)
and canonical [periodic-maintenance source fact](../../../../../qualification/catalog/esp32s31/wifi-phy.toml)
describe the configured composition and remaining evidence limits. Active ACL
HIL exercises due physical transactions and connection continuity; it does not
establish every temperature-triggered calibration branch, measured worst-case
execution bounds, deliberate-skip hardware coverage or DTM deadline/RF-stop
qualification.
Source implementation and diagnostic scenarios do not by themselves qualify
RF behavior; qualification consumes independent evidence.
