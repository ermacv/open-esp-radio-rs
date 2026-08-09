# HIL control protocol

`open-esp-radio-hil-protocol` is the shared host/target wire model. Source
types and `PROTOCOL_VERSION` are authoritative; this file records invariants.

```text
00 00 | COBS(postcard(Envelope) || CRC32C) | 00
```

Every envelope carries protocol version, boot ID, message sequence, session
ID and request ID. CRC is checked before decoding. Text output is diagnostic
only and is never accepted as lifecycle or performance evidence.

Boot:

```text
reset -> Hello/Capabilities -> WaitingForNetwork
      <- optional startup-artifact chunks
      <- ProvisionNetwork(credentials, IPv4 policy)
      -> Idle -> NetworkReady(address) -> ServiceReady
```

Credentials and IPv4 policy are runtime inputs. Credentials are bounded,
redacted, never echoed and cleared after PMK derivation. Startup artifacts are
opaque chunked byte strings with transport CRC; the ESP32-S31 adapter uses one
for retained PHY calibration. The target never persists it to NVS/flash.

Traffic sessions use one correlated state machine:

```text
Configure -> Configured -> Arm -> Armed -> Start
          -> SessionReady(direction) -> Running -> Draining
          -> typed Evidence -> Finished(CRC) -> AcknowledgeResult -> Idle
```

UDP evidence accounts for bytes, datagrams, loss, duplication and ordering in
each direction. TCP evidence accounts for bytes and EOF-completed streams.
ICMP records response latency. Full duplex requires both directional readiness
events before host traffic starts.

Station lifecycle commands acknowledge admission first and publish a separate
correlated completion only after the production owner returns:

- `CycleStationEpoch`: stopped connected epoch, finite rescan/rejoin and new
  connected generation;
- `StopStation`: child tasks, IRQ and DMA quiesced and role-neutral owner
  reconstructed;
- fault injection: typed recoverable discard or terminal quarantined owner;
- unsolicited lifecycle: connected, peer loss, attempt failure and retry
  exhaustion.

One target task owns USB RX/TX. Radio/network paths never wait for UART; typed
events use bounded queues or retained snapshots. One host worker owns reset,
framing and request correlation. Firmware publishes capabilities, never
expected hashes, ELF paths, vendor ABI versions or register layouts.
