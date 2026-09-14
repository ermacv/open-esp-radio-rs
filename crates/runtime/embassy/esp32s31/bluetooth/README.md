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

Peer termination, six events without establishment (`0x3e`), or established
supervision expiry (`0x08`) restore the connection allocations after unlink.
If executor latency closes an established event window before RUN publication,
the retained transaction advances directly to a later event; update instants
remain exact and supervision bounds that recovery.
The actor publishes successful or failed LE Connection Complete at the proven
establishment boundary and Disconnection Complete after an established link
ends. Pending command responses stay ahead of these unsolicited events, while
radio readiness can continue during output backpressure. Idle command intake
resumes only after each applicable Host-enabled event is published or masked;
`IdleRestored(PeripheralDisconnected { reason })` remains the separate runtime
diagnostic boundary.

When command order is ready, the same active branch waits for Host commands.
An older response and then an already-pending connection event take precedence.
A matching Disconnect for handle `0x0001` publishes Command Status before it
queues `LL_TERMINATE_IND`; retransmission retains that PDU until peer
acknowledgment or `T_terminate` reaches the connection supervision timeout, then
retirement reports reason `0x16`. Reset retains its exact command token while
any published RUN completes, retires the graph, and only then applies bootstrap
Reset. LE Read Remote Features publishes Command Status before admitting
`LL_PERIPHERAL_FEATURE_REQ`; radio progress, the 40-second response deadline,
the masked completion event and disconnect cancellation remain owned by the
same actor. Deadline expiry retires the connection directly with `0x22`,
without transmitting a voluntary termination PDU. Read Remote Version Information
uses the same ordered admission, deadline and completion path when the connection
configuration supplies an explicit local identity. The deadline restarts after
each later LL Control PDU enters the TX graph. A pending LE Long Term Key
Request keeps command intake live and accepts one matching positive or negative
reply for the current handle. The same active actor retains the encryption
handshake, procedure deadline, packet counters and masked Encryption Change or
Encryption Key Refresh Complete publication across every wait. Other classified commands use
the radio-active response policy. A matching Host ACL packet is copied out of transport storage,
fragmented into legacy 27-byte plaintext or 23-byte encrypted LL payloads,
retained across retransmission and
returns one Number Of Completed Packets credit after its final acknowledged
fragment. Accepted peer LL Data fragments are copied into a two-packet
Controller-to-Host FIFO after Connection Complete, split to the Host-declared
ACL buffer length and retained across HCI queue backpressure or exhausted Host
credits. Host Number Of Completed Packets restores credits for handle `0x0001`
without a success event. Flow-controlled output keeps its command intake live
even when the sole Host ACL owner is occupied; an older pending normal response
still retains causal order. A second Host ACL packet is consumed and completed
without displacing the retained packet. Central Connection
Update and Channel Map Update remain owned across recurring scheduler
cancellation and apply at their exact instants. Host-visible parameter changes
publish LE Connection Update Complete after Connection Complete; anchor-only
moves do not. Synchronous and isochronous frames retain the non-command boundary.

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
