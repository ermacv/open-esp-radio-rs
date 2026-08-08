# HIL control protocol

This is the current protocol guide for the `open-esp-radio-hil-protocol`
crate, not a dated qualification result.

The HIL USB Serial/JTAG link is a bidirectional control plane. Network sockets
carry only the traffic being measured; boot identity, configuration, readiness
and evidence use this link.

## Wire contract

The target-neutral `open-esp-radio-hil-protocol` crate is shared by firmware
and the host runner. A frame is:

```text
00 00 | COBS(postcard(Envelope) || CRC32C) | 00
```

Two leading delimiters recover from arbitrary ROM or text output, including a
zero that opened a false frame. CRC is checked before postcard deserialization.
Every envelope carries `protocol_version`, `boot_id`, `message_sequence`,
`session_id` and `request_id`. Text output remains diagnostic and is never a
machine-readable lifecycle dependency once its corresponding event has been
migrated.

Protocol version 3 retains separate target RX and target TX traffic shapes and
adds one opaque startup artifact. The protocol defines only ordered chunks,
the total length and a transfer CRC32C. Its meaning and exact length belong to
the selected target adapter, not to the shared wire crate.

Protocol version 5 adds a correlated station-epoch completion. Acceptance of
`CycleStationEpoch` means only that the bounded command was admitted. A later
`StationEpochCompleted` with the same `request_id` is emitted reliably after
the target adapter has observed runner/teardown return, scan-owner return, a
fresh join, and startup of the replacement connected runner. Diagnostic text
may be dropped or truncated and is not accepted as lifecycle evidence.

Protocol version 6 adds reliable unsolicited `StationLifecycle` events.
`Connected` identifies the outer station generation. `Disconnected` preserves
whether the edge was a proved `BeaconLoss`, another link-policy decision, or a
healthy controller reconnect. This lets AP-loss qualification reject a
synthetic cycle even when both paths execute the same teardown and rescan.

Protocol version 7 keeps those link edges and adds typed `AttemptFailed` and
`RetryExhausted` events. They carry generation, one-based attempt, stable
station stage and a target-independent reason. A prolonged-absence cell can
therefore prove complete `NoCandidate` scans and the exact bounded exhaustion
edge without accepting lossy text as evidence. A station lifecycle publisher
also waits for the exact lifecycle sequence to be written by the USB protocol
owner. Merely entering the bounded event queue is insufficient, especially at
a terminal station exit.

Protocol version 8 adds a one-shot, correlated `InjectStationFault` command.
The terminal ESP32-S31 cell arms `ConnectedTxAfterPublication`: a real network
lease and MAC descriptor cross into the production LMAC transaction before
the HIL decorator replaces its next service wake with a contradictory TX
event image. `StationFault` is emitted with the same `request_id` only after
the runner returned, executor-task borrows were acknowledged, RX DMA stopped,
and the TX owner was observed in its reset-required quarantine. This is a
terminal owner-frontier test; the host performs a cold target reset and proves
a new network-ready epoch instead of reusing the quarantined descriptor.

Protocol version 9 makes station fault evidence a discriminated contract and
adds `ConnectedRxBeforeStagingOverCapacity`. The ESP32-S31 RX cell takes a real
completed DMA unit, then a statically dispatched admission policy narrows only
that unit's staging limit. The production `TooLong` path recycles its complete
descriptor chain and waits for reload; evidence is emitted only after the same
live service stages a following unit and returns successfully. The host then
requires a complete station-epoch cycle and exact UDP RX delivery. This
recoverable variant has no task-stop, DMA-stop or reset-required fields, so it
cannot be confused with the terminal TX quarantine variant.

Protocol version 10 moves the IPv4 policy into `ProvisionNetwork`. The host
selects DHCP or a static address/prefix/gateway for each boot, so an isolated
qualification topology no longer changes firmware identity.

Protocol version 11 adds directional `SessionReady` evidence. `Start`
acceptance proves that the console admitted a session; traffic begins only
after the selected RX/TX workers have consumed it. Full duplex requires both
directional readiness events.

The protocol contains no expected firmware hashes, ELF paths, vendor ABI
versions or target-specific register layouts. The firmware publishes actual
capabilities; the calling qualification manifest decides whether that image is
acceptable.

## Boot and network provisioning

The startup lifecycle is:

```text
reset
  -> Hello(capabilities)
  -> WaitingForNetwork
  <- GetCapabilities
  -> Hello(capabilities)
  <- UploadStartupArtifact(chunk 0..N)  [optional]
  -> Accepted                           [per chunk]
  <- ProvisionNetwork(credentials, IPv4 policy)
  -> Accepted
  -> Idle
  -> PHY/MAC/STA workflow
  -> NetworkReady(address)
```

The firmware image is built without an SSID or passphrase. Traffic commands
read them at runtime from:

```text
OPEN_RADIO_HIL_STA_SSID
OPEN_RADIO_HIL_STA_PASSWORD
OPEN_RADIO_HIL_STA_IPV4_CIDR             [optional, for example 10.42.0.138/24]
OPEN_RADIO_HIL_STA_GATEWAY_IPV4          [optional, requires a static CIDR]
OPEN_RADIO_HIL_STARTUP_ARTIFACT          [optional input/output path]
```

The older `OPEN_RADIO_STA_SSID` and `OPEN_RADIO_STA_PASSWORD` names are accepted
by the host as temporary compatibility aliases, but no longer affect firmware
compilation. Without a static CIDR the target uses DHCP. Both modes use the
same firmware image; IPv4 topology is therefore a host-owned qualification
input rather than part of the embedded build identity. Credentials are
bounded to the WPA2 limits, never echoed, never
written to UART capture, redacted from `Debug`, and cleared from transient
protocol buffers. They remain in target RAM only until PMK derivation; the
passphrase is then cleared. The SSID may still appear in ordinary scan and
association diagnostics; it is not treated as secret material.

If the startup-artifact path exists, the host uploads it before provisioning.
If it does not exist, initialization proceeds without retained state. After
initialization the target returns the validated or newly produced artifact and
the host replaces that file atomically. A typed status reports whether the
artifact was created, restored or rejected and replaced, together with the
initialization time. The ESP32-S31 adapter uses this for its 524-byte PHY
calibration record; the shared protocol contains neither that size nor
eFuse/ABI fields. Target firmware never writes NVS or flash for this flow. The
artifact CRC is transport integrity, not a firmware/oracle identity hash.

## Ownership

- One firmware task owns both async USB halves and arbitrates binary events
  ahead of droppable text logs.
- A bounded command queue separates decoding from control decisions.
- Radio and network tasks never wait for USB TX. They publish bounded events or
  retain evidence for later delivery.
- One host worker owns reset, reads, writes and frame decoding. Requests are
  correlated by boot and request identity.

The station epoch lifecycle is intentionally separate from transport-session
state:

```text
Idle
  <- CycleStationEpoch(request_id)
  -> Accepted(request_id)
  -> stop connected runner and return teardown owners
  -> scan and return scan owners
  -> Authentication / Association / WPA2
  -> start replacement connected runner
  -> StationEpochCompleted(request_id, complete ownership evidence)
```

Real peer-loss qualification uses unsolicited events rather than a command:

```text
-> StationLifecycle(Connected { generation: 0 })
   [host removes the controlled AP]
-> StationLifecycle(Disconnected { generation: 0, BeaconLoss })
   [host restores the controlled AP]
-> StationLifecycle(Connected { generation: 1 })
```

Terminal TX fault injection is a separate correlated flow:

```text
Idle
  <- InjectStationFault(ConnectedTxAfterPublication, request_id)
  -> Accepted(request_id)
  <- Configure / Arm / Start(UDP TX)
  -> real TX descriptor publication
  -> production RadioResetRequired
  -> stop executor tasks and RX DMA
  -> StationFault(request_id, exact owner frontier)
  [host cold-resets target]
  -> fresh NetworkReady / ServiceReady
```

Recoverable RX injection deliberately does not stop the runner:

```text
Idle
  <- InjectStationFault(ConnectedRxBeforeStagingOverCapacity, request_id)
  -> Accepted(request_id)
  -> take real completed RX DMA unit
  -> production TooLong discard and descriptor reload
  -> following unit staged by the same live ring
  -> StationFault(request_id, recoverable discard evidence)
  <- CycleStationEpoch(request_id')
  -> StationEpochCompleted(request_id', complete ownership evidence)
  <- Configure / Arm / Start(UDP RX)
  -> exact post-reconnect Transport evidence
```

## UDP RX session

The ordinary RX image advertises runtime configuration and structured
evidence. After `NetworkReady` and `ServiceReady`, the host resolves and warms
the ingress path with one negative-sequence UDP control datagram. The target
does not open a sample before `Start`; after `Start`, it discards that terminal
datagram before taking the first payload. A one-second settle interval makes
the former implicit readiness/BlockAck delay explicit and keeps cold-start
loss outside steady-state qualification.

The measured lifecycle is:

```text
Idle
  <- Configure(UDP, RX, payload, duration, offered rate)
  -> Accepted -> Configured
  <- Arm
  -> Accepted -> Armed
  <- Start
  -> Accepted
  -> SessionReady(RX)
  -> Running
  -> Draining
  -> Evidence(Transport)
  -> Finished(summary, evidence CRC32C)
  -> Finished state
  <- AcknowledgeResult
  -> Accepted -> Idle
```

The target first snapshots the complete result in RAM. USB serialization and
the retained detailed text report happen outside the measured interval. The
host verifies the evidence-set CRC and requires the typed byte, datagram and
throughput values to equal the independently parsed text oracle. It also
requires the target count and UDP sequence evidence to account for every host
datagram exactly; the throughput floor cannot hide loss or reordering.

## UDP TX session

The TX-only image uses the same lifecycle. Its `Configure` command carries the
host IPv4 endpoint, payload length, interval and optional offered-rate bound,
so changing the HIL host or traffic shape does not rebuild the firmware. The
target publishes `SessionReady(TX)` after consuming the accepted session,
snapshots socket and A-MPDU evidence after the interval, and allows a bounded
post-measurement drain before publishing `Finished`.

The host binds its sink before reset and compares target-enqueued bytes and
datagrams with the packets actually received. This detects both internal send
errors and network loss at the tail of a stream, where a receiver-only sequence
gap check has no later sequence number from which to infer the missing tail.

## UDP bidirectional session

The bidirectional image also uses the same lifecycle. One `Configure` command
carries both flow descriptions and the host endpoint for target TX. After
`Start`, a target coordinator routes RX-only and TX-only sessions to one
worker, and fans a bidirectional session out to both. Each selected worker
publishes directional readiness. The coordinator remains the sole completion
owner, waits for exactly the selected workers, and publishes one combined
`TransportEvidence` followed by one `Finished` event.

The host binds the target-TX sink before reset, then receives target traffic in
parallel with its paced target-RX producer. Qualification requires the typed
RX counters to equal the target text report and the typed TX counters to equal
the independently observed host sink counters. Both directions must report no
transport errors, loss or reordering before the result is acknowledged.
Duplicate target-TX datagrams are rejected independently rather than being
folded into either loss or reordering.

## Migration boundary

UDP and TCP RX, TX and bidirectional qualification share runtime network
provisioning, configuration, directional readiness, lifecycle and structured
evidence. TCP begins listening after `SessionReady`; the host half-closes after
its bounded interval, and target EOF closes the evidence interval. One TCP
`rx_unit` is one EOF-completed stream; exact byte equality replaces UDP
datagram/sequence accounting.
