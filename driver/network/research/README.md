# Experimental network datapath

This crate contains a synchronous network engine and a physical batch
materializer for radio research. Its only external repository consumer is a
Wi-Fi adapter host test using `physical::PinnedBatchResources`. The engine has
its own unit tests; a product integration or HIL runner does not compose it.

The current engine is allocation-free and synchronous. It owns bounded
general-memory UDP/control work, parses Ethernet/ARP/IPv4/ICMP/UDP, reports
durable `EgressDemand`, and writes only radio-selected work into a caller-owned
`ReservedTxBatch`. No complete Ethernet-frame staging tier is required for
normal UDP TX. The current enqueue API copies caller bytes into canonical work
storage; final construction copies that payload into SRAM. This is two payload
copies from the caller, not an application-to-radio one-copy API.

Current source scope:

- resolved IPv4 UDP TX with software IPv4/UDP checksums;
- synchronous UDP RX delivery;
- ARP request generation and ARP reply generation;
- ICMP echo reply generation;
- typed bulk/link-control admission;
- fixed per-radio-flow queues with exact-owner failure semantics;
- transactional pinned-SRAM reservation and direct final-frame construction;
- `PhysicalTxSource` transfer into the shared STA encoder/BA/retry owner,
  covered by a host regression with partial BlockAck and terminal credit return;
- no heap, executor, Xarxa, Embassy, PAC or hardware dependency.

Outside the implemented scope:

- ARP cache and unresolved datagram retention;
- fragments, IPv6, DHCP or TCP;
- a fused ESP32-S31 radio runner/HIL target;
- split-core batch SPSC transport;
- airtime scheduling or hardware completion feedback.

This crate exposes no executor wait or radio lifecycle. Its host tests establish
software ownership behavior; they do not qualify an integrated hardware path.
