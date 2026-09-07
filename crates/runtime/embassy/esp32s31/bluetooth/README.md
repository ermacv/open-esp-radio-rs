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
