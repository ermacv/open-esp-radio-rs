# Documentation

Documents describe the current implementation and its boundaries. Component
details live beside their owner; this directory contains shared contracts.

## Choose a route

| Goal | Start | Continue |
| --- | --- | --- |
| Understand the architecture | [From binary evidence to a Wi-Fi station](binary-to-station.md) | [Worked channel change](channel-walkthrough.md), [responsibility and dependency reference](architecture.md) |
| Make a first contribution | [Host tutorial](first-contribution.md) | [Contribution workflow](../CONTRIBUTING.md) |
| Investigate hardware | [Blobray task map](../tools/blobray/README.md#choose-a-task) | [Reviewed models and publication](../registers/README.md), [PHY research](phy/README.md) |
| Build and validate a station | [ESP32-S31 route](station-hardware.md) | [Application example](../examples/esp32s31-station/README.md), [HIL](../hil/README.md), [qualification](../qualification/README.md) |

These routes assume Rust and embedded basics. The host route needs no board or
private binary; the hardware route names additional prerequisites at each step.
This directory is also an [mdBook](../tools/docs/README.md) book: the
[Pages address](https://ermacv.github.io/open-esp-radio-rs/) serves it with
searchable navigation and the API documentation after a manual publication, and
`mdbook serve docs` previews it locally.

For learning, start with the explanation and the host exercise. For a concrete
job, choose the hardware or contribution route. Component references define
the exact inputs, outputs and limitations once you know which owner you need.
You do not need to read every component reference to make a first change.

## Architecture and reference

- [Repository ownership](architecture.md): production, tooling, register,
  HIL, verification and qualification responsibilities.
- [Driver architecture](../crates/README.md): layers, resources and application
  integration.
- [Channel walkthrough](channel-walkthrough.md): one real hardware fact through
  review, publication, PAC, HAL, PHY and the station scan caller.
- [Protocol terminology](protocol-naming.md): IEEE 802.11, Wi-Fi, IEEE 802.15.4,
  Bluetooth and module naming.
- [Network implementation choices](network-implementations.md): original and
  patched Xarxa, released Embassy/smoltcp, rationale and current availability.
- [Wi-Fi network integration](wifi-egress.md): packet ownership, SRAM admission,
  completion, compatibility and research boundaries.
- [Verification and qualification](verification-and-qualification.md): evidence
  strength, freshness and the sole readiness authority.
- [Durable HIL archives](hil-archives.md): offline packaging and private storage.
- [HIL provenance and replay](hil-reproducibility.md): artifact identity,
  source reconstruction, rebuild comparison and lab observations.
- [PHY comparison](phy/README.md): compiled comparison and incomplete outcomes.
- [Public source policy](source-policy.md): permitted inputs and source checks.

## Component guides

- [Applications and examples](../README.md#start-here).
- [HIL](../hil/README.md) and [ESP32-S31 target](../hil/targets/esp32s31/README.md).
- [Qualification programs](../qualification/README.md).
- [Register publication](../registers/esp32s31/publication/README.md).
- [Vendor verification](../verification/README.md).
- [Blobray](../tools/blobray/README.md), [memory analysis](../tools/memory-report/README.md)
  and [repository commands](../tools/repo/README.md).

The [ESP32-S31 radio capability map](../crates/hardware/esp32s31/driver/FEATURES.md) indexes
shared lifecycle, PHY, coexistence and the Wi-Fi, Bluetooth and IEEE 802.15.4
inventories. These source references do not replace machine qualification or
hardware evidence.

For API semantics, consult module/item rustdoc: the published
[host](https://ermacv.github.io/open-esp-radio-rs/api/host/oer/index.html),
[ESP32-S31](https://ermacv.github.io/open-esp-radio-rs/api/esp32s31/oer_esp32s31_phy/index.html)
and [ESP32-S31 Wi-Fi composition](https://ermacv.github.io/open-esp-radio-rs/api/esp32s31-wifi/oer_esp32s31_embassy_wifi/index.html)
documentation, or `cargo xtask doc` from the repository root. Each package is
documented once, for the target and features in its `[package.metadata.docs.rs]`.
CLI references use each binary's `--help`; they are separate from library rustdoc.

Follow the [documentation policy](documentation.md) when adding or changing a
document.
