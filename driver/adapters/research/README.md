# Hardware-first research datapath

This crate is the network-side oracle for measuring the ESP32-S31 radio
datapath without inheriting Xarxa or Embassy driver semantics. It is not a
third compatibility mode.

The current engine is allocation-free and synchronous. It owns bounded
general-memory UDP/control work, parses Ethernet/ARP/IPv4/ICMP/UDP, reports
durable `EgressDemand`, and writes only radio-selected work into a caller-owned
`ReservedTxBatch`. No complete Ethernet-frame staging tier is required for
normal UDP TX: the canonical payload is copied once while the final frame is
constructed in its physical destination.

Current source scope:

- resolved IPv4 UDP TX with software IPv4/UDP checksums;
- synchronous UDP RX delivery;
- ARP request generation and ARP reply generation;
- ICMP echo reply generation;
- typed bulk/link-control admission;
- fixed per-radio-flow queues with exact-owner failure semantics;
- no heap, executor, Xarxa, Embassy, PAC or hardware dependency.

Not yet implemented:

- ARP cache and unresolved datagram retention;
- fragments, IPv6, DHCP or TCP;
- physical ESP32-S31 SRAM batch composition;
- split-core batch SPSC transport;
- airtime scheduling or hardware completion feedback.

The next boundary binds `ReservedTxBatch` to production pinned SRAM leases,
then composes the same engine in fused and split-core runners. Hardware claims
start only after that composition is measured by HIL.
