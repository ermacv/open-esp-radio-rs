# ESP32-S31 radio PAC boundaries

This package owns the restricted radio-register interface. `RadioPartitions`
acquires the generated singleton once as opaque register partitions; register
sets such as `WifiRadioRegisters` and `BluetoothTaskRegisters` are assembled
from those partitions and carry the reviewed domain transactions. The PAC
holds no protocol route: the HAL owns the neutral radio root, decides which
partitions each exclusive route consumes, retains the others and supplies
multi-register sequencing, polling, delay, recovery and lifecycle policy.

Every handwritten PAC operation is a single transaction: it keeps no state
between calls, has no phases, never polls and never routes. A straight-line
multi-register sequence and the affine receipt it returns (for example a
stopped-scheduler or rollback token) are allowed; pending/step machines,
in-flight flags and retry loops belong to the HAL. The HAL is the only
production crate that depends on this package.

## Sources and generated outputs

The [source-only publication configuration](../../../../registers/esp32s31/publication/registers.toml)
declares the outputs below. Generation belongs to the host-side
[`cargo registers` publisher](../../../../tools/registers/README.md), with shared schemas
and invariants in `tools/registers/model`. Reviewed model, ownership policy and provenance live together under
[`registers/esp32s31`](../../../../registers/esp32s31/README.md). There is no runtime
`pac-gen` dependency or additional generator crate in the production dependency graph.

| Artifact | Source and responsibility |
| --- | --- |
| [Radio SVD](../../../../registers/esp32s31/published/radio.svd) | Published from the reviewed chip register model and project policy |
| [raw/src/lib.rs](raw/src/lib.rs) | Generated svd2rust register accessors from that SVD |
| [src/generated.rs](src/generated.rs) | Generated semantic capability catalog selected by [PAC API policy](../../../../registers/esp32s31/policy/api.toml) |
| [Radio bindings](../../../../registers/esp32s31/published/radio.bindings.toml) | Published register-to-raw-PAC binding metadata |
| [src/ownership.rs](src/ownership.rs) and domain modules | Handwritten authority, register-local operations and restricted access |

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
the handwritten `oer-esp32s31-soc-esp-hal` package. It neither generates
nor replaces this radio PAC.

The two chains use their reviewed dependency revisions. A capability exposed
by either PAC is not itself hardware qualification of a radio operation.

## Validation

`cargo registers generate --check`, `cargo xtask check architecture` and
`cargo xtask check images` validate register
sources, dependency direction, handwritten unsafe boundaries and the final
linked image. Publication checks also cover source-only publication reproducibility unconditionally. Artifact-scoped
publication is additionally checked when its review report is present.
Generated addresses, masks and field positions are not regression test oracles;
ownership and memory protocols are tested through behavior and typed contracts.
