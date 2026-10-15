# IEEE 802.11 lower-MAC port

`oer-ieee80211-lower-mac` declares the radio port between portable Wi-Fi MAC
logic and the backend that puts frames on the air, on the owner shape of
[the architecture](../../../../docs/architecture.md#radio-ports), with the
values its backends' cores take. It is sans-IO: it declares the port and its
values and never waits on it.

One submission is one hardware transmission attempt. The backend counts down
the caller's backoff, sends the PPDU once at the frame's rate, waits for the
solicited response and reports one completion. Retries, rate fallback, the
backoff draw, sequence and packet numbers and reordering stay above the port
unless the backend reports them in its `HardwareServices`.

## On the owner shape

The module `owner` states the lower-MAC port on the owner shape of
`oer-ieee80211-port`, the Wi-Fi block's port lifecycle: a backend implements `LowerMacBackend`, a `PortBackend`
whose disabled-only settings are `LowerMacSetup` (the channel, as a
`SupportedChannel`; a backend whose addresses change also implements
`AddressSetup`), whose phase control is
`LowerMacControl` (the transmit gate, closed by an awaited `close_gate`
whose `TxGated` guard reopens it, EDCA as a `SupportedEdca`, the beacon
receive priority) and whose clients are `LowerMacClient<'id>`s, one per
configured interface, branded by the install. A client awaits the grant
of the queue an access category occupies (`LowerMacClient::Queue`, a
`QueueGrant`) and a transmit buffer (`LowerMacClient::TxBuffer`), waited
for, never refused, each with a seat in its pool's line that the caller
lends to the take and keeps across takes: the body receives each
interface's `TxSeats` once, a queue seat and a buffer seat per access
category (`TX_SEATS` per line, the station's then the access point's), and
a queue seat's index names the access category whose queue it waits for
(`seat_access_category`), so `queue(seat)` takes no access category of its
own. Both are the backend's own types: this package states
only the contract, so no caller forges one, and the backend proves it with
compile-fail tests of its types. The buffer request
(`LowerMacClient::tx_buffer`) describes the frame (`TxFrame`), and its kind
fixes its rate and response, so a frame and its response never disagree.
A data or management frame carries a `SizedFrame`: its header with the
MPDU's length (`MpduLength`, at most what the buffers hold), which
`SizedFrame::new` takes only when it holds the header
(`FrameHeader::header_len`), and the buffer is exactly that long. The kinds
are
an individual frame (an `AckHeader` with an `IndividualAddress`
receiver, any data or management frame but Action No Ack, an ACK at a
phase rate), an individual frame without an ACK (a `NoAckHeader`, a QoS data or
Action No Ack frame that says so, at a `SupportedNoAck` rate), a group frame (a
`GroupAddress` receiver, no response) or a BlockAckReq (a BlockAck, at a
`LegacyPhaseRate`). The header is a `HeaderSpec` whose fields agree with
its `FrameControl` (a data or management frame); the backend writes it, and
its QoS Ack Policy from the frame's response, so that field has one source.
The caller writes the body (`TxFrameBody`). A client's attempt
(`PhaseAttempt`) carries the queue grant, that buffer and the values the
capabilities marker `K: Ieee80211Capabilities` checks (`SupportedPower`,
`SupportedBackoff`, `SupportedCoex`). A submission moves them into the backend and returns its `Work`; the
backend gives them back when it takes the attempt's terminal, before it
hands the event out, so no caller holds them and a dropped handle or event
loses none. The body receives each role's pairwise and group key slots
and the shared Block Ack slots once; a key holds a slot of its client's own
kind and gives it back when removed, once no attempt in flight uses it: a station's key is
not an access point's, nor a group key a pairwise one. A client keeps one
Block Ack agreement per peer and TID; a new one for the pair replaces the
old and gives its slot back. The in-memory model keeps separate
agreement tables for station and access point while their slots come from
one shared bank pool: the same peer and TID in one role does not replace
the other role's agreement. A client's type is its role
(`StationClient`, `AccessPointClient`): only the station has an HE BSS
color, and each role states its receive rules with its own type
(`StationReceive`, `SupportedFilter` of the role), applied once its frames
in flight end. The install gives the port its channel and addresses;
`LowerMacSetup` changes the channel while disabled, and a backend whose
addresses can change implements `AddressSetup` (the ESP32-S31 does not).

The module's table maps every refusal of the old lower-MAC port to its
type, and lists the world's outcomes that stay. The keeper of the transmit permits and
their guards (`TxPermits`, `QueuePermit`, `BufferPermit`) and an in-memory
backend on the owner shape (`OwnedModel`, driven through `ModelHandle`)
wait, so they live in `oer-ieee80211-runtime`'s `lower_mac_owner`, not in
this sans-IO package.

## Optional features

The owner shape has no optional operation yet: A-MPDU transmission, beacon
timing (each interface's TSF and TBTT schedule), monitor reception, air
reservation and live retune. Their values (`AmpduBuffer`, `AmpduPayload`,
`AmpduCapabilities`, `TbttSchedule`, `TbttEvent`, `TsfSample`,
`TsfRelation`, `VifTsf`, `MonitorCapabilities` in `extensions`) stay, as the
ESP32-S31 core takes them; the old port's extension traits went with it in
#709 step 1, and they come back as owner-shape extensions with the product
datapath's move onto the port.

Parametric limits of what a backend has are its capabilities
(`LowerMacCapabilities`), and a value outside them has no checked type. Why a backend lacks a feature or a
value (hardware absent, glue not written, vendor knowledge not recovered,
policy decision pending) is recorded in its qualification catalog, not in
code.

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
| Retry counters, limits and the Retry bit of an MPDU; the rate of each attempt through a `RateLadder` | `oer-ieee80211-upper-mac` `retry` |
| Which A-MPDU subframes the next attempt resends after a `BlockAckReport` (`BlockAckReport::acknowledges`), republication after a failed protection exchange, the BlockAckReq, individual retries, aging | `oer-ieee80211-upper-mac` `ampdu` |
| RTS/CTS or CTS-to-self and the control-frame rate of each PPDU | `oer-ieee80211-upper-mac` `protection` |
| One exchange as a sequence of `TxAttempt`s, and its statistics | `oer-ieee80211-upper-mac` `tx::TxPlanner` |
| The backoff draw | `oer-ieee80211-softmac` `EdcaContention` |
| Transmit packet numbers and receive replay | `oer-ieee80211-mac` `ccmp` |
| Receive Block Ack reordering | `oer-ieee80211-mac` `block_ack::reorder` |

Limits, rate ladders and duration estimates in which implementations differ
are parameters of those algorithms; the Espressif values recovered from the
vendor stack are the family package `oer-espressif-ieee80211-policy`.

## Users of the port

None in the product yet: the ESP32-S31 station and access point drive their
datapath directly until #709 step 2 moves it onto the port. The portable
services that drove the old port (the upper-mac service, the access point,
STA+AP and the station's port module) were deleted in #709 step 1; git
history keeps them for the rebuild on the portable stack.

## Implementers

| Backend | Where | What it serves |
| --- | --- | --- |
| Host model (`OwnedModel`, driven through `ModelHandle`) | `oer-ieee80211-runtime`, `lower_mac_owner::model` (`test-support`) | The base contract, with the world ending published attempts, refusing an enable or poisoning the backend |
| ESP32-S31 (`Esp32s31Backend` over the static runtime `Esp32s31LowerMac`) | `oer-esp32s31-ieee80211-runtime`, `lower_mac` | The base contract over the S31 core's checked operations: four queues, the station and access-point clients, keys per role (an access point's pairwise slot is its association identifier), the receive Block Ack banks as one pool both roles share, and the retune of an `Enable` after a channel change |
