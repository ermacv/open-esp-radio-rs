# ESP32-S31 bindings to esp-hal

These modules connect the driver's typed contracts to the pinned `esp-hal`
and `esp-pacs` forks. They own platform integration;
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

## Dependency boundary

The PAC fork publishes the missing Wi-Fi, Bluetooth and IEEE 802.15.4 interrupt
sources through SVD patches and generated vectors. Radio register fields and
their access policy belong to this repository's radio PAC. IEEE 802.15.4 uses
the ordinary typed HAL interrupt API; the adapter owns binding and teardown
on the same CPU.

The HAL fork extends upstream ESP32-S31 support with radio ownership tokens,
TRNG, TIMG clock selection, external-memory startup and interrupt handoff
mechanisms. The shared board profile in `platform/esp32s31`
chooses memory sizes and timings; the platform bootstrap owns relocation and the
transition to the separately linked runtime. HAL PSRAM adoption records an
existing mapping without resetting the device or remapping live memory.
S31 Ethernet is enabled explicitly through HAL's `__ethernet` feature, so a
radio-only or compatibility-network build does not acquire its network-driver
dependencies. The Ethernet implementation remains available in the fork.

Cargo manifests pin immutable dependency revisions, and each workspace lockfile
records the resolved graph. All workspace islands use the same S31 PAC
revision. A source check establishes API compatibility; PSRAM execution,
interrupt placement and multicore startup also require HIL checks.
