# HIL RX delivery observations

With driver observation enabled, single-flow RX sessions retain the first eight
legacy/unknown PHY observations as `ORX_ANOMALY`, with UDP sequence (including
negative terminal markers), IP length, QoS identity when present, copied PHY
signal words and acquisition time. `ORX_ANOMALIES` reports the total even when
storage is full. Records are printed after collection, never from the RX hook.

Delivery telemetry retains the first 16 forward UDP gaps with adjacent valid
QoS identities in the same TID. `ORX_GAP` reports the UDP and MAC sequence pair;
`ORX_GAPS` reports the correlated count so truncation remains visible. Missing
or incompatible metadata is not reconstructed. These bounded records survive
session completion, reset at the next session, and are printed after collection
outside the RX observer and its critical section. They distinguish UDP gaps
with continuous MAC numbering from losses of already numbered MPDUs; late
recovery and the delivery ledger remain separate evidence.

The correctness image also retains up to 32 ARP observations per UDP RX
session. `ORX_ARP` records decoded Ethernet/IPv4 ARP identity and radio,
network-admission or explicit rejection edges. With original/patched Xarxa,
the observer additionally records stack consumption and the result of the
stack's TX call (`TxAccepted`/`TxRejected`). Acceptance is queue admission, not
radio completion. `ORX_ARP_SUMMARY` includes the total so a truncated sample set
is visible. Records are frozen at session end and published through the console
capacity event after measurements; no USB wait enters packet processing.
These observations do not generate replies, reserve packet storage or change
stack backpressure. Pair them with `host-wire.pcapng` to distinguish neighbor
resolution stalls from radio delivery pauses.
