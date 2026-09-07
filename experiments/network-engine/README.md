# Experimental network datapath

This crate contains a synchronous network engine and a physical batch
materializer for radio research. Host tests compose its physical owners with
the shared STA encoder and retry path. A product supervisor or native HIL
target does not compose this engine.

The current engine is allocation-free and synchronous. It owns bounded
general-memory UDP/control work, parses Ethernet/ARP/IPv4/ICMP/UDP, reports
durable `EgressDemand`, and writes only radio-selected work into a caller-owned
`ReservedTxBatch`. No complete Ethernet-frame staging tier is required for
normal UDP TX.

## Payload ownership

`ResearchNetworkEngine<FLOWS, WORK, PAYLOAD_CAPACITY, Payload>` selects the
UDP storage owner at compile time. `Payload` implements `AsRef<[u8]>`; it can
be a lease into an application-owned pool in general memory. The engine does
not allocate or decide where that pool lives.

- `enqueue_udp` is available with the default `InlinePayload<PAYLOAD_CAPACITY>`.
  It copies caller bytes into canonical work, then final construction copies
  the payload into SRAM: two explicit payload copies from the caller.
- `enqueue_udp_owned` transfers the payload owner into the queue. With an
  external pool lease, admission transfers only the lease; final construction
  copies the payload once into the reserved frame. Passing an inline array
  still moves that array and does not promise copy-free admission.

Payload contents and length must remain stable while queued. Every admission
failure returns `TxEnqueueFailure { error, payload }` with the original owner.
Physical backpressure retains it. A failed writer retains the current source
and reports the committed prefix; a detected payload-length change also fails
before writing. A successful final-frame write releases the payload owner.
The physical owner then retains the separate SRAM buffer through radio retry
and completion. Returning either memory owner does not establish delivery to
the peer. Dropping an engine releases all remaining queued payload owners.

`PAYLOAD_CAPACITY` bounds the default copied UDP storage and inline ICMP echo
replies. An external UDP owner is bounded by the shared frame-length contract,
not by this inline capacity. It must also fit the concrete physical batch's
frame limit. `FillStopReason::FrameTooLong` retains the head work and reports
that destination mismatch without consuming a physical credit. The caller
must bound admission to its physical frame limit or supply a larger destination;
waiting for more slots of the same size cannot help. Per-datagram cancellation
is not exposed; dropping the engine releases its entire retained backlog. No IP fragmentation or path-MTU discovery is
provided. ICMP work
still occupies inline storage in the shared work enum, so selecting a small
UDP lease alone does not remove that storage from every queue slot. Work and
flow capacities are compile-time bounds; changing active queue occupancy does
not release statically reserved memory.

## Borrowed receive path

`receive` accepts a contiguous Ethernet frame. `receive_parts` accepts the
destination and source `MacAddress`, EtherType in host byte order, and a
borrowed payload after the Ethernet header. Both take a timestamp, radio route
classifier and synchronous UDP callback. The contiguous entry delegates to the
parts entry; the latter does not assemble or copy an Ethernet frame.

The caller retains its receive buffer owner throughout the call. UDP delivery
borrows directly from that storage, and the caller may reuse or release it
after return. ARP and ICMP replies retain independent bounded work. Both
entries apply the same destination, length and checksum validation and update
the same counters; reported frame length includes the 14-byte Ethernet header.
EAPOL returns `Unsupported` and remains the radio security owner's
responsibility.

## Construction on radio demand

The common datapath's `SelectedTxSource` adapts an exclusive
`EgressWorkProvider` borrow and an empty reserved physical batch into a
`PhysicalTxSource`. Each physical take constructs at most one frame. A radio
consumer that stops early leaves all unrequested payload owners in the engine,
without constructing an unused physical prefix.

The caller authorizes a current radio key and chooses frame and byte budgets
before reserving storage. Matching includes interface, link epoch, peer
generation, TID and admission class. The provider borrow prevents queue
mutation during that synchronous turn; it does not validate or freeze the
radio's live generation or eligibility. Those checks remain with the radio
owner.

`finish` returns construction counts and the observed stop reason or writer
error. Counts describe final Ethernet frames and bytes, not radio delivery.
A source stops taking work after its first terminal outcome. Writer failure,
byte limits and destination limits retain the unconstructed head and tail.
Dropping or finishing the source returns unused reservations. Taken physical
owners can outlive it and retain their SRAM credits through radio completion.
If the consumer stops without encountering a terminal outcome, the report's
stop field is `None`.

The STA TX owner also exposes `start_request` over the common `TxRequestSource`
contract. A request need not contain Ethernet bytes. Busy and first-frame
materialization refusals return that request unchanged; after successful
materialization, the shared encoder, BA and retry owners govern the physical
frame. Host tests exercise this entry with the real research engine. The
product scheduler still requires complete software-frame requests; a native
network owner, its selection tickets and supervisor composition are not wired.

## Protocol and integration scope

Current source scope:

- resolved IPv4 UDP TX with software IPv4/UDP checksums;
- synchronous UDP RX delivery from contiguous frames or borrowed decoded parts;
- ARP request generation and ARP reply generation;
- ICMP echo reply generation;
- typed bulk/link-control admission;
- fixed per-radio-flow queues with exact-owner failure semantics;
- caller-selected UDP payload ownership without mandatory enqueue copying;
- transactional pinned-SRAM reservation and direct final-frame construction;
- construction on each requested physical take within a radio-selected budget;
- `PhysicalTxSource` transfer into the shared STA encoder/BA/retry owner,
  covered by a host regression with partial BlockAck and terminal credit return;
- no heap, executor, Xarxa, Embassy, PAC or hardware dependency.

Outside the implemented scope:

- ARP cache and unresolved datagram retention;
- fragments, IPv6, DHCP or TCP;
- a product supervisor wiring the engine into a fused ESP32-S31 runner or
  native HIL target;
- split-core batch SPSC transport;
- airtime scheduling or hardware completion feedback.

This crate exposes no executor wait or radio lifecycle. Its host tests establish
software ownership behavior; they do not qualify an integrated hardware path.
