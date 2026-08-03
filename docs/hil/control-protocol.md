# HIL control protocol

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

The protocol contains no expected firmware hashes, ELF paths, vendor ABI
versions or target-specific register layouts. The firmware publishes actual
capabilities; the calling qualification manifest decides whether that image is
acceptable.

## Boot and network provisioning

The current compatibility lifecycle is:

```text
reset
  -> Hello(capabilities)
  -> WaitingForNetwork
  <- GetCapabilities
  -> Hello(capabilities)
  <- ProvisionNetwork(credentials)
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
```

The older `OPEN_RADIO_STA_SSID` and `OPEN_RADIO_STA_PASSWORD` names are accepted
by the host as temporary compatibility aliases, but no longer affect firmware
compilation. Credentials are bounded to the WPA2 limits, never echoed, never
written to UART capture, redacted from `Debug`, and cleared from transient
protocol buffers. They remain in target RAM only until PMK derivation; the
passphrase is then cleared. The SSID may still appear in ordinary scan and
association diagnostics; it is not treated as secret material.

## Ownership

- One firmware task owns both async USB halves and arbitrates binary events
  ahead of droppable text logs.
- A bounded command queue separates decoding from control decisions.
- Radio and network tasks never wait for USB TX. They publish bounded events or
  retain evidence for later delivery.
- One host worker owns reset, reads, writes and frame decoding. Requests are
  correlated by boot and request identity.

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
  -> Accepted -> Running
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
throughput values to equal the independently parsed text oracle.

## UDP TX session

The TX-only image uses the same lifecycle. Its `Configure` command carries the
host IPv4 endpoint, payload length, interval and optional offered-rate bound,
so changing the HIL host or traffic shape does not rebuild the firmware. The
target begins only after `Start`, snapshots socket and A-MPDU evidence after
the interval, and allows a bounded post-measurement drain before publishing
`Finished`.

The host binds its sink before reset and compares target-enqueued bytes and
datagrams with the packets actually received. This detects both internal send
errors and network loss at the tail of a stream, where a receiver-only sequence
gap check has no later sequence number from which to infer the missing tail.

## Migration boundary

The bidirectional image still advertises compatibility mode: it uses typed
network/service readiness but retains its existing compile-time TX peer,
readiness probe and text completion. It is the next session migration target.
TCP RX/TX/bidirectional follows after the UDP lifecycle is uniform.
