# ESP32-S31 radio PAC boundaries

This package owns the restricted radio-register interface. Its handwritten
ownership and domain modules consume reviewed generated capabilities. HAL
borrows finite register authority and supplies multi-register sequencing,
polling, delay, recovery and lifecycle policy.

## Sources and generated outputs

The [project configuration](../../../../verification/vendor/targets/esp32s31/vendor-project.toml)
declares the outputs below. Generation belongs to the host-side
[Blobray publisher](../../../../tools/blobray/README.md), with shared schemas
and invariants in `tools/blobray/crates/register-model`. There is no runtime
`pac-gen` dependency or additional generator crate under `driver`.

| Artifact | Source and responsibility |
| --- | --- |
| [Radio SVD](../../../../svd/esp32s31-radio.svd) | Published from the reviewed chip register model and project policy |
| [raw/src/lib.rs](raw/src/lib.rs) | Generated svd2rust register accessors from that SVD |
| [src/generated.rs](src/generated.rs) | Generated semantic capability catalog selected by [registers/api.toml](../../../../verification/vendor/targets/esp32s31/registers/api.toml) |
| [Radio bindings](../../../../svd/esp32s31-radio.bindings.toml) | Published register-to-raw-PAC binding metadata |
| [src/ownership.rs](src/ownership.rs) and domain modules | Handwritten authority, register-local operations and restricted access |
| [Raw sidecars](raw/README.md) | Handwritten IEEE 802.15.4 ownership and validation operations, with their own unsafe policy |

Both Rust generated outputs are regenerated through the publisher. Do not
edit them by hand or extend their lint exceptions to handwritten files. The
semantic catalog may contain reviewed leaves that no current owner exposes;
that does not authorize a broad register escape hatch.

## Upstream PAC and esp-hal

The separate upstream chain is the pinned `esp-pacs` dependency used by
`esp-hal`. Its board and SoC bindings live under
[`adapters/esp-hal/esp32s31`](../../../adapters/esp-hal/esp32s31/README.md).
The [`soc` adapter](../../../adapters/esp-hal/esp32s31/soc/README.md) owns
typed upstream-register operations, cache/MMU and GDMA transactions. It is
handwritten integration code, even though its retained package name ends in
`platform-pac`. It neither generates nor replaces this radio PAC.

The two chains retain their reviewed dependency revisions. Normalizing source
paths does not update upstream hardware descriptions or qualify new behavior.

## Validation

`tools/audit-source-only.sh` validates register sources, dependency direction,
handwritten unsafe boundaries and the final linked image. It also checks
publication reproducibility when the reviewed local inputs are present.
Generated addresses, masks and field positions are not regression test oracles;
ownership and memory protocols are tested through behavior and typed contracts.
