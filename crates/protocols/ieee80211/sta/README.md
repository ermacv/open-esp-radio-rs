# Portable Wi-Fi station policy

This crate contains the station, independent of a chip, executor and network
stack. It is sans-IO: `machine` is one state machine for the whole lifecycle
(scan, join, WPA2 four-way handshake, connected, background scan, group
rekey, leave) that takes the port's events, the channel arbiter's answers and
notices, commands and time as values and leaves the port's and the arbiter's
actions as values. A thin service loop over the lower-MAC port feeds it
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225)).

Module map:

- `machine`: the station, its tokens (`StaTokens`) and its host slice tests
  beside the real arbiter;
- `data`: the connected station's data machine beside it: MSDUs into QoS or
  non-QoS Data MPDUs in its own tickets and buffers, under the pairwise key
  the station's `LinkView` lends, and the BSS's data MPDUs into MSDUs; no
  A-MPDU, Block Ack, reorder, replay check or power save yet;
- `join`: Open System and SAE authentication and Association state machines
  and their retry policy;
- `association`: association capability selection from the scan record;
- `power_save`: what the access point knows of the station's power state and
  the TIM's decision;
- `station`: the application's vocabulary (lifecycle stages, fault reasons,
  reconnect policy) that `oer-radio` publishes;
- `modem_sleep`: the connected station's power manager, after the vendor's
  `pm.o` and `pm_coex.o`, with the vendor's timing constants;
- `sa_query`: the SA Query procedure of an association under management
  frame protection;
- `link_monitor`: beacon-loss decisions;
- `pmksa`: the owned SAE PMKSA cache a reconnect resumes through an exclusive borrow;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation. FTM local
  exchange timing carries a `NonZeroU8` dialog token from validation through
  replay tracking; terminal token zero cannot enter that state. The TWT wake
  plan is computed in `TsfInstant`s of one TSF generation the caller tracks;
  its intervals and guards are `TsfDuration`s. A guard before the epoch is
  `BeforeTsfEpoch`, and an endpoint beyond the epoch is `BeyondTsfRange`;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `crates/protocols/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `crates/roles/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.

The station's SSID and SAE bodies own their bounded lengths in heapless
vectors. WMM priorities and TWT flow IDs select exact arrays without numeric
indexing; an unpublished data return retains its checked frame description.
Data writes an open MAC header directly into its proven span, so it fits an
exact-size buffer without a temporary CCMP gap or a second payload move.
The PM owner retains a NonZero beacon period through its divisions. Vendor
timer arithmetic keeps its original wrapping width; diagnostics saturate.

The crate uses tidy's canonical strict lint block. Compile-time checks use
private functions scoped inside const expressions, with the explicit
`expect(clippy::panic, reason = "evaluated at build")`. Those functions cannot
be called from the runtime path; their checked values remain in closed types.


## Transmit ownership

Every `StaTransmit` or `DataTransmit` carries an owned context alongside its
buffer and ticket. The service puts that context in the port attempt and
returns it through `TxDone` or the local unsent path. A station context
names the connection epoch and purpose (authentication, handshake message,
PM transition, probe or rekey); the current state never guesses which
operation a terminal ended. Connected control admission remains one at a
time; its flags describe pending policy transitions, not completion identity.

A data context keeps the link epoch, original TID, order, checked
`SizedFrame<AckHeader, K>`, access category and protection. The immutable
frame description preserves its header/length proof through an unpublished
return; no constructor or frame-byte inspection runs again. A submitted
context owns an `AcPermit` from the pool sized by backend queue depth
(and bounded by the tickets granted to Data). Returning the context returns
that permission; there is no `at_port` counter, in-flight metadata FIFO or
TID decoding at completion. Voice keeps the same data depth bound, so a
station operation waits for at most that depth plus one on-air attempt. An unpublished return is inserted by its original order,
so a batch of returns preserves both per-TID and shared-AC order. An old
link's terminal returns its tokens without requeueing or affecting the new
link; the service checks the same epoch before updating rate selection.
`LinkView` still borrows sequence/replay/key state only for a synchronous
step. No queued context borrows the station or data machine.

Station alone issues `LinkEpoch`, before starting Connect. Its non-cloneable
`LinkEpochSource` consumes the install's unique `Slot<ConnectionEpochKind>`
from `oer-ieee80211-lower-mac`; there is no reset or slot extraction. The
backend supplies that slot, while Station constructs and owns its source. The epoch and contexts carry the invariant
install brand. Recreating Station therefore cannot restart its counter
while earlier operations can return. Issuance is total with a wrapping `u64`.
An outstanding operation must not survive an entire cycle of 2^64 connections;
this lifecycle condition is documented on `LinkEpochSource::begin`. The
ticket bound limits concurrent operations, not their age. Ending a link
closes its epoch without allocating another. Data takes this same epoch
from `LinkView`; temporary sleep or a channel absence does not close it.

Station and Data construct `ClassSizedFrame` while encoding into their class's
buffer, then retain that proof through the operation. The service only adds
the phase's rates; it cannot replace the checked header or length.
`TxRefusal::NoRate` belongs to service selection, while
`TxRefusal::Preparation` carries a pool refusal. Neither is a radio terminal.

Preparation refusal is `StaInput::Unsent { reason, .. }`, distinct from
`TxDone`. Refusing M2/M4 ends the handshake through key removal and the
normal leave path; an unsubmitted M4 cannot establish a connection. A
required join frame refused at preparation ends that join. Connected PM
frames retain the existing unsuccessful-send policy; refusing a group-key
response ends the handshake rather than retiring its keys as if it sent.
Data's `DataReturn::Refused` ends that MSDU and releases its permission and
tokens once, without retrying unchanged invalid input. The service reports
the typed reason to its application and continues; preparation refusals
never enter radio/rate statistics.

`StationResources` groups the epoch source, ticket and small-buffer pools,
channel places and key slots transferred into Station. Completion inputs
carry `ReturnedMpdu::Fresh` or `Sent`; service preserves that state. Only
the machine renews a published MPDU when retiring it, releasing its body
and returning fresh memory to its pool.

`StaData::returned` consumes a complete terminal or preparation refusal and
has no `Result`. The service exhaustively pairs `TxContext` with the closed
Small/Large fresh-or-sent buffer enum. A mismatched pair or an aggregate
terminal for this singles-only service is a backend contract violation:
it takes the Poisoned fail-stop path without a stray-terminal counter or
resource reuse. `Indeterminate` retires and counts the MSDU; Station treats
it as unconfirmed. Neither it nor `ContentionLimit` trains rate selection.

A gate withdrawal retains the MPDU at its original TID/order. Fresh memory
is prepared normally; Sent goes through retransmission with the same SN/PN.
KeyRemoved retires that MSDU and releases admission and resources. A barrier
waits only the current publication; rate selection ignores every withdrawal.


## Responses to lifecycle actions

`Station` owns keys and free slots across scan/join/handshake/leave
transitions. The pairwise cell is Free, Installing, Installed or Removing.
Group slots are fungible credits from the composition's declared pool;
there are no physical group cells, wrappers or Removing states. Installed
owners are held by protocol key ID, including a replaced GTK retained until
Group Message 2 ends and its normal retirement action runs.

Installation actions carry private, non-cloneable `PendingPairwiseKey` or
`PendingGroupKey`. The pairwise token names its one cell and link epoch;
the group token names the key ID and link epoch. Inputs cannot independently
relabel an installed group key. An old epoch's response takes normal removal.
A raw group slot returns to the free pool regardless of which key it held.
The full pool proves that every group installation/removal ended. Leaving
also waits for the pairwise slot before reporting. Removals can remain
outstanding concurrently and return in either order.

`Configure` carries `PendingConfiguration`. Station grants only one such
token until it returns in `Configured`; an old epoch's reply releases that
permission without completing the new transition. The service keeps only
this outstanding action token. Arbiter responses are already proved by
affine resources: Answer returns LeasePlace or Lease, AbsenceAnswer returns
AbsencePlace or Lease, and Exchanged consumes the old Lease and returns its
replacement. The service returns these synchronous answers in the same step.
None of these normal lifecycle paths adds fail-stop behavior.

The [owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6157356164)
also requires that a missing pending token be a compile-time error. Owning
rustdoc supplies that compile-fail test and its valid counterpart; module
tests exercise stale handshake/rekey replies, slot return order and stale
configuration replies.
