# Portable Wi-Fi station policy

This crate contains the station, independent of a chip, executor and network
stack. It is sans-IO: `machine` is one state machine for the whole lifecycle
(scan, join, WPA2 four-way handshake, connected, background scan, group
rekey, leave) that takes the port's events, the channel arbiter's answers and
notices, commands and time as values and leaves the port's and the arbiter's
actions as values. A thin service loop over the lower-MAC port feeds it
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6128926225)).

Produced actions, transmissions and pending response tokens are `must_use`:
the consumer performs or forwards them before waiting again. Rust can still
explicitly drop an affine value; the annotation diagnoses accidentally
discarded results and does not replace terminal resource handling.

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
- `pmksa`: a unit-tested prototype of an owned SAE PMKSA cache. It is outside
  the production API until the station's SAE path consumes it;
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
vectors. The SSID bound comes from MAC's `MAX_SSID_LEN`. Scan channels form
a bounded, contiguous vector of checked `SupportedChannel` values, so a
hole cannot terminate a scan before its remaining channels.
WMM priorities and TWT flow IDs select exact arrays without numeric
indexing; an unpublished data return retains its checked frame description.
Data writes an open MAC header directly into its proven span, so it fits an
exact-size buffer without a temporary CCMP gap or a second payload move.
The PM owner retains a NonZero beacon period through its divisions. Vendor
timer arithmetic keeps its original wrapping width; diagnostics saturate.
Each modem-sleep input returns a fresh `PmActions` batch. Before changing
state, its helpers receive fixed arrays of affine effect places: writing
consumes a place, so neither overflow nor overwriting another effect is
possible. TBTT reserves the largest batch: 1 normalization + 1 RF wake +
13 coexistence + 1 interval + 8 parameter effects, totaling 24. Other inputs
reserve fewer places. Consumers execute each returned batch in order;
separate inputs cannot append into the same batch. The TX offer returns its
hold decision alongside the effects, including when the frame must wait.
The station owns `Counter`, the saturating diagnostic value its data path
and service loop report. It carries no lower-MAC contract, resource capacity
or protocol-state decision.

The crate uses tidy's canonical strict lint block. Compile-time checks use
private functions scoped inside const expressions, with the explicit
`expect(clippy::panic, reason = "evaluated at build")`. Those functions cannot
be called from the runtime path; their checked values remain in closed types.


## Transmit ownership

The interface configuration carries `OwnAddress`; an Open Authentication
request takes it separately from the peer's `Bssid`. Both wrap the MAC
crate's checked `IndividualAddress`, so a request cannot swap the local
transmitter and BSS identities. The test access point admits a request only
after checking its subtype and all three header addresses.

`Station::new` requires one to eight tickets, small buffers and group-key
slots at compile time. Authentication and Association reserve an owned
ticket/buffer credit before beginning a request or consuming its sequence
number. EAPOL (including the QoS rekey response), Null and deauthentication
also reserve credit before consuming a sequence. A missing credit waits for
the admitted operation's terminal; the
association's state deadline remains active, while its overdue retry time
does not cause repeated polling. Every four-way handshake step retains the
same response deadline while waiting for TX, a key slot or installation.
Expiry leaves once and reclaims late returns through the normal teardown.
After reserving credit, encoding can fail only with `EncodeError`. Null
transmission waits only when credit is occupied; an encoding failure leaves
once and returns its ticket and buffer, and a missing connection is reported
as a changed link.

GTK wire IDs include zero. IEEE 802.11-2012, 6.3.19.1.2 and Figure 11-31,
define the two-bit ID separately from the key type; receive selection in
11.8.2.6 uses the frame's addressing to distinguish PTK and GTK. The pinned
[ESP-IDF parser](https://github.com/espressif/esp-idf/blob/4d59230ddff16327812782151ef0afef202dc6d7/components/wpa_supplicant/src/rsn_supp/wpa.c#L1064)
also accepts all four IDs. `GroupKeyId::select` addresses four protocol
records; physical slots come from the backend's pool. S31 STA accepts ID zero
in its dedicated group slot. S31 AP excludes it because its `1 + key_id`
layout would collide with the station group slot. That hardware restriction
does not narrow the portable WPA2 receive contract. Legacy WPA/TKIP and mesh
key-selection policies are outside this station's supported WPA2-CCMP mode.

`poll_action` drains internal transitions until it produces an action or
reaches an external wait. The Associated action is a join step, delivered
before Connected or handshake work. Disconnect waits for an absence to end
before deauthentication, and waits for TX credit instead of discarding that
frame. The bounded transition graph is documented beside `poll_action`.
An unsuccessful or refused scan follows the same Leave path and disables
reception before its terminal notice. A lease terminal received during Leave
preserves pending key removals, deauthentication and configuration; it ends
Leave only when the machine already waits for that lease terminal.

Every `StaTransmit` or `DataTransmit` carries an owned context alongside its
buffer and ticket. The service puts that context in the port attempt and
returns it through `TxDone` or the local unsent path. A station context
names the connection epoch and purpose (authentication, handshake message,
PM transition, probe or rekey); the current state never guesses which
operation a terminal ended. Connected control admission remains one at a
time; its flags describe pending policy transitions, not completion identity.
The complete `TxOutcome` reaches the station's state transition. Power save
then interprets a Null's outcome as Delivered or Unconfirmed, keeping one of
three states: Awake, Asleep or AwakeUnconfirmed. An unconfirmed PM=0 only
needs reconciliation through TIM when an earlier PM=1 may have reached the
access point. The Null retry budget is a `NonZeroU8` from its constant
definition through the service's retry-plan limit.

A data context keeps the link epoch, original TID, order, checked
`SizedFrame<AckHeader, K>`, access category and protection. The immutable
frame description preserves its header/length proof through an unpublished
return; no constructor or frame-byte inspection runs again. `DataTransmit`
has closed fields and consumes into context, ticket and buffer. Frame and
access category occur only in the context; the admission permission holds
only its pool bit. `DataProtection` records Open or CCMP at encoding and
borrows the matching link's key at submission. A changed epoch or protection
returns `TxRefusal::LinkChanged` with every resource, before preparation;
a protected encoding cannot fall through to an open transmission. A submitted
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
installed owners occupy private reserved records in installation order. A
pending install owns a vacant record; returning it cannot overflow a pool
of retiring keys. With a free physical slot the new GTK installs first and
the superseded key retires after Group Message 2. A full pool uses
`replace_group_key`, selecting the same ID or the oldest non-current owner;
only a one-slot backend replaces its sole current key for a new ID.

The service retains `PendingGroupReplacement` and matches the port's
`GroupKeyReplaced` terminal by slot identity. Until that terminal RX uses
the previous epoch; the successor starts at KeyRSC there, even when its PN
is lower. Identical ID and material produce a response without reinstalling
or changing replay. Hardware rejection poisons the port; rollback is a
driver detail, never a portable success result. The model verifies the RX
ordering and returned slot; the S31 runtime migration is still separate.

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
