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
  chosen by the composition. The backend moves that context unchanged;
  no `Clone` bound, frame inspection or parallel lookup identifies the
  original operation. Bodies remain owned until the same terminal, even
  when the backend copies their octets into DMA memory. An empty pool is the caller's
  own capacity; an occupied queue is the backend's, which keeps the attempt
  until the queue frees.
- **Submission.** `submit` takes the ticket, context and `TxSource` together.
  A fresh source owns its class buffer, `ClassFrame` and body. Preparation
  checks the actual memory, Protected/key agreement, CCMP reserve directly
  after the MAC header, and placement of the external body at the MPDU's
  tail. `SubmitError::Refused` returns every resource and parameter before
  admission or PN allocation; it creates no terminal. Its buffer uses the
  backend's exhaustive return type, while its `ClassFrame` retains the
  original class and phase. Poisoning remains the separate fail-stop path.
  The model retains its checked layout beside that exact allocation through
  numbering and publication; no unchecked body/CCMP offset crosses a queue.
  There is no prepared buffer detached from its phase.
  A retransmission takes the returned numbered MPDU and a phase-bound
  `RetransmissionPlan`: its header, payload, body and key-install identity
  remain inside the MPDU. There are no independent header/body arguments to
  substitute. SN and PN stay; only Retry changes in the MPDU. Rates, retry
  plan, RTS/CTS, power and coexistence are parameters of the new attempt.
  A different key installation is refused even when it reuses the same slot.
  An aggregate builder retains its phase, install and interface role until
  `submit_ampdu`; push failures return their inputs, and submission refusal
  returns the entire aggregate, context and ticket. Numbered subframes own
  their bodies on return; fresh subframes return indexed external bodies.
  `PhaseAggregate::into_parts` releases the builder's resources directly,
  so refusing an aggregate never forces resubmission to recover its memory.
  `renew` returns both memory and the retained body explicitly.
- **Retry plans.** An individual frame carries its `RetryPlan`, a sequence
  of rates with their transmissions and protection, as mac80211's rate
  table. The backend retries the head of its queue by the plan before the
  next attempt of that queue; the completion reports the transmissions made
  and the last rate, for the rate control above the port.
- **Order.** Every operation reaches the hardware in the order the caller
  made it: an attempt that names a key is published after that key's
  install.
- **Keys and agreements.** A key holds a slot of its interface's own kind;
  its removal ends with `KeyRemoved`, once no attempt in flight names it. A
  receive Block Ack agreement is a token that holds its slot, replaced or
  removed by value.
- **Settings.** An interface's receive rules, EDCA, the BSS color, a TBTT
  plan and monitor reception take effect before the next submission; the
  latest value of each wins. A receive configuration and a gate's closing
  end in events (`Configured`, `GateClosed`).
- **Reception.** A received frame's buffer goes back through `recycle`; a
  lagging body loses unsolicited frames as `EventsLost`, never a terminal.

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
