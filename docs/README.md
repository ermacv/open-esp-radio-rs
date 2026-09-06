# Documentation

Documents describe the current implementation and its boundaries. Component
details live beside their owner; this directory contains shared contracts.

## Architecture and reference

- [Repository ownership](architecture.md): production, tooling, register,
  HIL, verification and qualification responsibilities.
- [Driver architecture](../driver/README.md): layers, resources and application
  integration.
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

Implementation support is documented by the owning chip/role, including the
[IEEE 802.11](../driver/chips/esp32s31/ieee80211/FEATURES.md) and
[Bluetooth](../driver/chips/esp32s31/bluetooth/FEATURES.md) matrices. These
references do not replace machine qualification or hardware evidence.

For API semantics, consult module/item rustdoc. From the repository root:

```console
cargo doc --workspace --lib --no-deps --locked --offline
cargo test --workspace --doc --locked --offline
```

These commands cover the root workspace libraries in their default feature
configuration. Independent integration workspaces and target-only APIs require
their own manifest, target and supported feature profile. CLI references use
each binary's `--help`; they are separate from library rustdoc.

Follow the [documentation policy](documentation.md) when adding or changing a
document.
