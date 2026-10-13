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
- `pmksa`: the SAE PMKSA cache a reconnect resumes, and its shared owner;
- `ftm`, `twt`: bounded requester state and deadlines; their presence does not
  establish a chip timestamp or wake-schedule implementation. The TWT wake
  plan is computed in `TsfInstant`s of one TSF generation the caller tracks;
  its intervals and guards are `TsfDuration`s. A guard before the epoch is
  `BeforeTsfEpoch`, and an endpoint beyond the epoch is `BeyondTsfRange`;
- `request`: caller-visible station configuration and selection values.

This is not a generic 802.11 frame crate and it is not an ESP32 backend.
Frame parsing/building belongs in `crates/protocols/ieee80211/mac`; ESP32-S31 ordering
and hardware ownership belong in `crates/roles/esp32s31/ieee80211/sta`; clocks, tasks,
DMA wakeups and network leases belong to the radio runtime; final resource
claims and board composition belong to integration.


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
return; no constructor or frame-byte inspection runs again. `StaData` keeps only counts of operations
at the port for admission, with no in-flight metadata FIFO or TID decoding
at completion. An unpublished return is inserted by its original order,
so a batch of returns preserves both per-TID and shared-AC order. An old
link's terminal returns its tokens without requeueing or affecting the new
link; the service checks the same epoch before updating rate selection.
`LinkView` still borrows sequence/replay/key state only for a synchronous
step. No queued context borrows the station or data machine.

Station alone issues `LinkEpoch`, before starting Connect. Its non-cloneable
`LinkEpochSource` consumes the install's unique station source slot; there
is no reset or slot extraction. The epoch and contexts carry the invariant
install brand. Recreating Station therefore cannot restart its counter
while earlier operations can return. Exhaustion emits
`StaNotice::ConnectRefused(EpochExhausted)` without starting a connection;
already accepted operations still return their resources. Ending a link
closes its epoch without allocating another. Data takes this same epoch
from `LinkView`; temporary sleep or a channel absence does not close it.

Preparation refusal is `StaInput::Unsent { reason, .. }`, distinct from
`TxDone`. Refusing M2/M4 ends the handshake through key removal and the
normal leave path; an unsubmitted M4 cannot establish a connection. A
required join frame refused at preparation ends that join. Connected PM
frames retain the existing unsuccessful-send policy; refusing a group-key
response ends the handshake rather than retiring its keys as if it sent.
Data's `Refused` input ends that MSDU and releases its admission count and
tokens once, without retrying unchanged invalid input. The service reports
the typed reason to its application and continues; preparation refusals
never enter radio/rate statistics.
