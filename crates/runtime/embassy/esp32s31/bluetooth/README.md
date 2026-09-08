# Bluetooth execution with Embassy

`controller` drives the chip controller's finite transactions and command
ordering. `session::{dtm, advertising, scanning, peripheral}` drives the
corresponding radio sessions. `time` binds controller deadlines to Embassy.

`notification` owns waker registration and borrowed event waits. Durable
pending work remains in the chip controller and its mailboxes. A wait registers
before checking that state; cancelling the wait does not consume the event or
release the radio owner.

Runtime modules retain owners across awaits, cancellation and shutdown. The
hardware backend owns MMIO transitions and quiescence proofs; final composition
owns task storage and platform resources. Public types live in their owning
module, without root compatibility exports.

The active peripheral branch drives the chip-owned completion and contiguous
successor transaction. Scheduler/post-unlink readiness and controller-time
rechecks progress independently of pending HCI response capacity, with radio
readiness winning a tie. Every await borrows the session retained in the actor.
A progress boundary represents a newly published RUN; a radio failure retains
all owners in a terminal boundary. See the driver's
[peripheral timing limits](../../../../hardware/esp32s31/driver/bluetooth/FEATURES.md#peripheral-timing-limits)
for the required clock bound and unsupported link behavior.

Connectable advertising also reports progress at the first and recurring RUN
publication. Publishing an HCI response does not duplicate that observation.
The actor retains the most recent reclaimed no-connection item status for
diagnostics, including an opaque nonzero value. It does not interpret that
status as reception evidence. It also retains a checked cumulative count of
received PDUs rejected by portable connection-request admission and the last
rejected header with its admission reason. Empty events preserve that last
observation; ordinary scan requests also count as rejected connection requests.
Recoverable recurrence boundaries preserve their
semantic cause; storage-specific interrupt errors remain with the retained owner.
