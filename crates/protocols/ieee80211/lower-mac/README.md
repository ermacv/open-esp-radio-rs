# IEEE 802.11 lower-MAC port

`oer-ieee80211-lower-mac` declares the radio port between portable Wi-Fi MAC
logic and the backend that puts frames on the air, on the owner shape of
[the architecture](../../../../docs/architecture.md#radio-ports), with the
values its backends' cores take. It is sans-IO: it declares the port and its
values and never waits on it.

## The phase

The port's lifecycle is `oer-ieee80211-port`'s: a backend implements
`LowerMacBackend`, a `PortBackend` whose disabled-only settings are
`LowerMacSetup` (the channel, as a `SupportedChannel`; a backend whose
addresses change also implements `AddressSetup`). An enabled port lends one
running phase (`LowerMacPhase`, module `phase`): one object under `&mut`,
the body's handle over single-producer single-consumer queues to the
backend's driver, which alone owns the hardware state
([owner decisions on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560)).
Its event stream (`LowerMacEvent`) carries every token an operation took
back; its operations are synchronous and never wait:

- **Tickets and buffers.** A transmission takes a ticket (a place in
  flight and its terminal's place) and the buffers of its frames, each of a
  class sized by the composition (`TxBufferClass`, `ClassFrame`); its
  `TxCompleted` event gives all of them back with `PhaseTypes::Context`,
  chosen by the composition with `type Context: 'id` in `PhaseTypes<'id>`.
  The bound retains install-branded epochs and excludes a reference to a
  machine local to the generic `PortBody::run`. Portable contexts need not
  be `Send`; only a backend such as S31 crossing execution contexts requires
  it ([owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6155221933)).
  `PhaseTypes` also owns the single body type and backend fault type for
  the phase and both of its interfaces; interface submission and the event
  stream report that same fault.
  `TxCompleted::Single` contains exactly one MPDU; `Aggregate` contains
  its submitted subframes. The backend must return the original owners,
  including when different classes share an AC queue. The common suite in
  runtime `lower_mac_owner::conformance` checks this obligation against the
  model now and is required of the S31 adapter at its runtime rewrite.
  The backend moves that context unchanged;
  no `Clone` bound, frame inspection or parallel lookup identifies the
  original operation. Bodies remain owned until the same terminal, even
  when the backend copies their octets into DMA memory. An empty pool is the caller's
  own capacity; an occupied queue is the backend's, which keeps the attempt
  until the queue frees.
- **Preparation and submission.** `TxBufferPool::prepare` checks a source
  where its buffer class is concrete and returns a closed `Prepared` value.
  `TxSource::check` first consumes the raw source into `CheckedTxSource`;
  layout errors return the source before entering the backend. `HeaderLayout`
  accepts an `AckHeader`, `NoAckHeader` or group-addressed header; this type
  selects QoS Ack Policy, with no separate flag at encoding. It
  obtains exactly sized arrays with `split_first_chunk_mut`. Each
  `HeaderLength` variant carries an encoder built from an exact wire array;
  encoding assigns the whole `[u8; N]` to its destination. Storage
  shortage, CCMP shortage and body overlap are `LayoutError`; backend
  `PreparationError` contains retransmission identity/kind, aggregate capacity
  and Block Ack window refusals.
  It retains the phase, install, role and key borrow. `ClassSizedFrame`
  carries a portable machine's size proof until the service adds phase-bound
  rates, without rechecking the class. `TxProtection::{Open, Ccmp(key)}` in
  the fresh source is the sole source of the Protected bit. It derives
  `MpduProtection` for the layout; callers cannot change a header's protection
  independently through a public setter. Station TX keys
  are pairwise; AP TX may use pairwise or group keys. The portable
  `MpduLayout` constructor checks CCMP room and tail-body overlap once,
  before any header/body write, admission or PN allocation.
  Preparation borrows `TxBody::bytes` once and keeps that view for both the
  length check and the complete tail copy. Re-reading an external body can
  select another slice; even a shorter second view must not leave stale
  payload bytes in an accepted MPDU. The body owner remains with its source.
  A checked header exposes `header_len`, and its MPDU layout exposes
  `mpdu_len`; neither has an empty state. Block Ack request lengths come from
  the MAC wire encoder's `BLOCK_ACK_REQUEST_LEN`.
  A refused preparation returns the original typed source and its owners;
  the caller still holds context and ticket. `submit` takes the prepared
  proof, context, ticket and attempt parameters and can refuse only poisoning.
  The queued operation owns backend data, never a borrow of a local machine.
  A retransmission takes the returned `Sent` MPDU and a phase-bound
  `RetransmissionPlan`: header, payload, body and installed-key identity
  remain inside that MPDU. SN and PN stay; only Retry changes in its bytes.
  Rate, retry plan, RTS/CTS, power and coexistence belong to the new attempt.
  Retiring the original installation makes preparation refuse
  `RetransmissionKey`, even if the slot and key material are reused.
  An aggregate builder owns its receiver, TID, role, rate and one protection.
  Its `PhaseAggregate::protection` derives the same typed layout value from
  its retained key, and each subframe produces an `AckHeader`.
  `CheckedAggregate::new` checks the class-specific `SubframeSource` with
  its builder and owns both until `TxBufferPool::push_aggregate` consumes it.
  The proof cannot be transferred to another builder. Fresh subframes have no protection argument, and
  their receiver/TID come from the builder. A Sent subframe retains its
  original installation and must match the builder. Refusal returns both
  the unchanged source and the builder with all earlier subframes.
  A builder using a compressed Block Ack window refuses `BlockAckWindow`
  before publication when a subframe is outside the 64 sequence numbers
  beginning at its first subframe, modulo 4096.
  A successful push returns `NonEmptyAggregate`; only this state can be
  submitted. `AggregateFull` remains a push refusal, and an empty aggregate
  has no submit operation ([owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6156247280)).
  `PhaseAggregate::into_parts` cancels either state without a ticket or TX
  terminal. Sent subframes retain their bodies; fresh subframes return
  indexed external bodies. `renew` returns memory and body explicitly.
- **Retry plans.** An individual frame carries its `RetryPlan`, a sequence
  of rates with their transmissions and protection, as mac80211's rate
  table. The first stage is mandatory; the tail is a bounded array with an
  initialized length, so it cannot contain holes. Filtering a plan keeps
  each accepted stage's full budget and order, within the original bound.
  The backend retries the head of its queue by the plan before the
  next attempt of that queue; the completion reports the transmissions made
  and the last rate, for the rate control above the port.
  `Ended.last` holds the exact nonzero MPDU count together with the last
  transmitted rate (`u16` covers all four 255-transmission stages). `None`
  means no MPDU reached air. A collision at a later rate preserves the
  preceding air transmission's count and rate. A hardware timeout with no
  completion ends as `Indeterminate`;
  the backend does not republish that MPDU, stop the queue, or return its
  tail. `TxStatus` describes only Success, AckTimeout, CtsTimeout, Collision
  and TimedOut observations of a publication. Decoder/accounting faults
  poison the backend and produce no `TxOutcome` or returned-resource event.
  Internal collisions remain driver retries; budget exhaustion is
  `Ended(ContentionLimit)`. Withdrawals name only GateClosed, KeyRemoved or
  Disabled. Rate selection ignores withdrawals, Indeterminate and
  ContentionLimit. A lifecycle barrier waits for one current publication,
  bounded by `LowerMacBackend::PUBLICATION_BOUND`, not the remaining plan.
  ACK still ends as `Ended(Delivered)`; a needed retry returns
  `Withdrawn(cause)` with Sent, retaining SN/PN. An unpublished attempt
  returns Fresh. Unknown timeout remains Indeterminate. Per-AC terminals
  precede the barrier confirmation in their original order.
- **Order.** Every operation reaches the hardware in the order the caller
  made it: an attempt that names a key is published after that key's
  install.
- **Keys and agreements.** A key holds a slot of its interface's own kind;
  its removal ends with `KeyRemoved`, once no prepared or published attempt names it. A
  receive Block Ack agreement is a token that holds its slot, replaced or
  removed by value. Its slot holds desired state: repeated add/replace/remove
  writes coalesce to the latest value before the driver's next command.
  They occupy no command slots; the command bound is tickets + 2 ×
  (key slots + 1), independent of agreement updates.
- **Settings.** An interface's receive rules, EDCA, the BSS color, a TBTT
  plan and monitor reception take effect before the next submission; the
  latest value of each wins. A receive configuration and a gate's closing
  end in events (`Configured`, `GateClosed`).
- **Gate evidence.** The install creates one `GatePlace` inside `GateOpen`
  and retains one `GateIssuer` in the backend. Closing moves that place to
  the driver; only after the publication barrier does the issuer transform
  it into `GateClosed` for the event. Opening consumes that evidence and
  returns the same place as `GateOpen`. There are no independent open and
  closed slots. The arbiter holds these exact branded states; the body can
  neither construct closed evidence, copy the place, nor keep both states.
- **Reception.** A received frame's buffer goes back through `recycle`; a
  lagging body loses unsolicited frames as `EventsLost`, never a terminal.

Station GTK replacement consumes the installed owner and its slot in one
FIFO command. That slot reserves one `GroupKeyReplaced` terminal under the
key-slot capacity bound, and the terminal returns the new key in the same
slot. Old-installation RX precedes it and new-installation RX follows it.
The station owns replay state and switches it at that boundary. A rejected
hardware write poisons the backend; an internal rollback is not a portable
terminal. `KeyTypes::GroupSlotIdentity` correlates the operation and terminal
without confusing slot reuse with `Identity`, which names key material's
installation for prepared TX.

The install mints one `Slot<ConnectionEpochKind>` for Station's epoch source.
This slot kind belongs to the lower-MAC contract; the backend neither depends
on the station's epoch implementation nor creates connection identities.

A frame's kind (`TxFrame`) fixes its response: an individual frame (an
`AckHeader` with an `IndividualAddress` receiver, any data or management
frame but Action No Ack), an individual frame without an ACK (a
`NoAckHeader` at a `SupportedNoAck` rate), a group frame or a BlockAckReq
(at `LegacyPhaseRate`s). The interfaces' types are their roles
(`StationPhase`, `AccessPointPhase`), each with its own receive rules
(`StationReceive`, `SupportedFilter` of the role).

The module `owner`'s table maps every refusal of the old lower-MAC port to
its type, and lists the world's outcomes that stay. Compile-fail tests in
`owner` and `phase` pin each rule a type carries.

## Optional features

Extension traits of the phase: `AmpduPhase` (one attempt sends one A-MPDU,
each subframe in a buffer of its own), `BeaconTimingPhase` (an interface's
TSF, its samples and its TBTT plan with the wake lead and the beacon window
and its `CoexPriority`) and `MonitorPhase`. Their values (`AmpduCapabilities`,
`TbttEvent`, `TsfSample`, `TsfRelation`, `VifTsf`, `MonitorCapabilities`)
are in `extensions`.

Parametric limits of what a backend has are its capabilities
(`LowerMacCapabilities`), and a value outside them has no checked type. Why
a backend lacks a feature or a value (hardware absent, glue not written,
vendor knowledge not recovered, policy decision pending) is recorded in its
qualification catalog, not in code.

The PHY and channel values come from
[`oer-ieee80211-mac`](../mac/src/phy.rs): `Channel` (2.4 GHz and 5 GHz with
20 MHz and 40 MHz geometry) and `PhyRate` (non-HT, HT and HE SU).
`RxEvidence` and `RxCryptoStatus` are the receive provenance values
`oer-ieee80211-softmac` re-exports as `MacRxEvidence` and
`MacRxCryptoStatus`, and softmac's `MacTxResult::HardwareFailure` carries a
`TxStatus`. `MacOperationOwnership::hardware_services` turns a softmac
service description into `HardwareServices`.

## Above the port

What a backend does not report in its `HardwareServices` is software above
the port, written once in portable packages for every backend:

| Work | Owner |
| --- | --- |
| The retry plan of an MPDU and the rate control that chooses it | `oer-ieee80211-upper-mac` |
| Which A-MPDU subframes the next aggregate resends after a `BlockAckReport` (`BlockAckReport::acknowledges`), the BlockAckReq, aging | `oer-ieee80211-upper-mac` `ampdu` |
| RTS/CTS or CTS-to-self and the control-frame rate | `oer-ieee80211-upper-mac` `protection` |
| Transmit packet numbers and receive replay | `oer-ieee80211-mac` `ccmp` |
| Receive Block Ack reordering | `oer-ieee80211-mac` `block_ack::reorder` |
| The station's lifecycle, from its scan to its disconnect | `oer-ieee80211-sta` `machine` |

Limits, rate ladders and duration estimates in which implementations differ
are parameters of those algorithms; the Espressif values recovered from the
vendor stack are the family package `oer-espressif-ieee80211-policy`.

## Implementers

The host model in `oer-ieee80211-runtime` implements the phase with
composition-selected context types. It returns queued operations on gate
close and normal disable, including queues held by the model air. On
fail-stop, no terminal is invented: backend-owned resources end with the
install. The ESP32-S31 runtime still serves the earlier client shape; its
port migration and image composition are the next stage of #563.


Station group-key slots are fungible, affine resources. The backend stores
and returns each raw slot with its installed key; it carries no station-cell
wrapper. Station returns removed slots to its declared free pool. Its full
pool proves that no group install or removal remains outstanding. Pending
installation tokens carry the protocol key ID and connection epoch.
