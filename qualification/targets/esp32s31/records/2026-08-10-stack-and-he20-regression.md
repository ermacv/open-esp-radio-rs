# ESP32-S31 stack and HE20 regression

Base revision: `1f86b6146ebc`; target: ESP32-S31 rev. 0.0; peer: FRITZ HE20.
The host path was Ethernet -> OpenWrt -> FRITZ. A direct laptop Wi-Fi route
was rejected after it reproduced upstream UDP reordering.

- Static stack audit passed: runtime maximum frame 17,296 bytes; bootstrap
  maximum 368 bytes; hard limit 32 KiB. Flash tuning uses a caller-owned
  124-KiB static SRAM scratch buffer instead of a 127,344-byte stack frame.
- Typed lifecycle stack evidence passed roundtrip and three reconnect cycles.
  Minimum free space was CPU0 104,936/229,760 and CPU1 8,668/16,384 bytes.
- ICMP passed 100/100 after a separate L2-readiness probe: p95 17.964 ms,
  maximum 23.687 ms.
- UDP RX passed exact delivery for 30 s at 90 Mbit/s offered load (host
  89.972, target median 87.692 Mbit/s) and for 10 s at 92.5 Mbit/s. Exact
  delivery failed at 93.75 and 95 Mbit/s; the stable HE20 ceiling is therefore
  above 90 and below 93.75 Mbit/s in this cell.
- Saturated UDP TX did not qualify. One run was interrupted by peer
  deauthentication reason 16; the station currently has no connected-state
  Group Key Handshake owner. A repeat without disconnect had every reported
  A-MPDU subframe BlockAck-confirmed, but 141/182,575 UDP datagrams were absent
  after the FRITZ/OpenWrt forwarding path. Bidirectional and TCP were not run
  after this first failing matrix edge.

This record qualifies the stack regression fix, lifecycle stack query, ICMP,
and 90-Mbit/s HE20 RX. It does not qualify long-lived WPA2 GTK rekey, UDP TX,
bidirectional UDP, or TCP.
