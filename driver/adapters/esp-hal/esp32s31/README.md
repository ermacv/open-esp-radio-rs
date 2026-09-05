# ESP32-S31 bindings to esp-hal

These modules connect the driver's typed contracts to the pinned upstream
`esp-hal` and `esp-pacs` implementations. They own platform integration;
portable radio policy and the generated radio PAC live elsewhere.

| Package directory | Responsibility |
| --- | --- |
| [soc](soc/README.md) | Upstream SoC register operations, cache/MMU, GDMA descriptors and transfer/completion ownership |
| [radio](radio/README.md) | Shared radio platform lease, clocks, interrupt publication and routing |
| `ieee80211` | Wi-Fi peripheral binding used by concrete composition |
| `ieee802154` | IEEE 802.15.4 platform interrupt/DMA binding |

The radio SVD, raw accessor backend and restricted semantic capability catalog
have their own [PAC provenance](../../../chips/esp32s31/pac/README.md). The
publisher belongs to host tooling. The retained `platform-pac` package name
for `soc` is a compatibility identity, not a claim that this adapter is generated.

Executor/time ABI bindings live under `adapters/embassy`; complete radio tasks
live under `runtime/embassy`; final static resources and board composition
belong to `integration`. A platform adapter does not become a second radio
owner when another protocol uses the shared hardware.
