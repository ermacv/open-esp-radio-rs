# Network boundaries

Start with the [network implementation guide](../../docs/network-implementations.md)
for stack choices, concrete crates, patches and build commands. This directory
owns stack-facing contracts; chip radio execution belongs to `crates/runtime`
and complete ESP32-S31 composition belongs to `crates/composition`.

| Path | Responsibility |
| --- | --- |
| `interface/` | Stack-neutral interface, link and error values |
| `../adapters/embassy-net/owned/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |
| `../../experiments/network-engine/` | Experimental synchronous engine and materializer; no product composition |

Packet ownership and execution are specified in
[Wi-Fi network integration](../../docs/wifi-egress.md). Selecting a patched
stack does not create another physical radio owner or another packet adapter.
