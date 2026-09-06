# Network boundaries

Start with the [network implementation guide](../../docs/network-implementations.md)
for stack choices, concrete crates, patches and build commands. This directory
owns stack-facing contracts; chip radio execution belongs to `driver/runtime`
and complete ESP32-S31 composition belongs to `driver/integration`.

| Path | Responsibility |
| --- | --- |
| `interface/` | Stack-neutral interface, link and error values |
| `adapters/xarxa/upstream/` | Original Xarxa driver API and packet-owner queues, shared by upstream and patched Xarxa |
| `adapters/embassy/compat/` | Released Embassy token API and frame staging for smoltcp |
| `adapters/embassy/owned/` | Maintained Embassy/Xarxa packet-owner contract with explicit pools |
| [dependencies/](dependencies/README.md) | Cargo source overrides for stack implementations |
| `research/` | Experimental synchronous engine and materializer; no product composition |

Packet ownership and execution are specified in
[Wi-Fi network integration](../../docs/wifi-egress.md). Selecting a patched
stack does not create another physical radio owner or another packet adapter.
