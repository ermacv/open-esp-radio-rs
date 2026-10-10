# Reviewed register publication

`oer-register-tool` validates source-owned hardware descriptions and generates
SVD, raw PAC, restricted PAC API and the binding index. It does not load a Blobray
project, vendor binary, device model or global provider registry.

`cargo registers init-model --request geometry.toml --directory new-model`
creates an editable native model from an explicit schema-1 TOML request:

```toml
schema = 1
chip = "example-chip"
address-space = "cpu"
[device]
name = "EXAMPLE"
version = "1"
description = "Unreviewed source model"
address-unit-bits = 8
width = 32
[[peripherals]]
name = "CONTROL"
base = 0x20000
length = 256
```

The new model has empty peripherals and address blocks, no inferred register
widths or fields. Device metadata/defaults use `ModelDevice`; the device bus width
does not declare the size of any register. Add explicit reviewed source assertions
with physical chip/address-space/address/width identities and evidence before
publication. Binary access observations support investigation; they do not supply
hardware access or write semantics.

`cargo registers import-svd --source device.svd --directory new-model --chip
example-chip --address-space cpu` imports standard CMSIS-SVD declarations into the
same schema-3 manifest/schema-2 fragment format. It preserves original XML as
`source.svd`, including extensions outside the native model. Imported declarations
have no review annotations or accepted assertions. The initializer similarly
retains `initialization.toml`. `device.toml` is the model entry point.

Both commands require a new directory with an existing parent. They validate and
render before exclusive directory creation, sync the source/fragment and write
the manifest last. Errors clean up that invocation's directory; an abrupt process
death can leave an incomplete draft. Existing destinations are never overwritten.
These operations create source models only; applicability, review/evidence packs
and publication policy are supplied explicitly through the following manifest.

```console
cargo registers validate --manifest registers/esp32s31/publication/registers.toml
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Omit `--check` to publish an intentional reviewed source change. `--check` is
read-only and fails if any selected output is absent or differs. All four outputs
are prepared before replacement; each file is replaced atomically, but a failure
between replacements reports an incomplete batch. Rerun generation to finish;
there is no claim of atomicity across four directories.

A device manifest may also place layouts of a register library through
`[[shared-fragments]]` entries (library manifest, layout fragment, the chip's
review file and a base address per peripheral). A library layout has no
`baseAddress` and no reviews; the loader rejects either, and a placement must
name exactly the layout's peripherals. A publication manifest that names a
`library` instead of a `model` publishes that library: svd2rust renders its
layouts at placeholder bases, the tool keeps only the register modules, makes
each peripheral alias the bare `RegisterBlock` and appends the library API's
transactions. It writes the raw crate and API bridges, but no SVD or binding
index, since a library has no addresses. A chip that places a library layout
re-exports the library's module from its raw PAC and may not define
transactions on that peripheral. See [`registers/ieee80211`](../../registers/ieee80211/README.md).

The schema-1 manifest selects model, sparse reviewed assertions, applicability,
memory map, ownership policy, API/lint packs, evidence catalogs and output paths.
Paths are relative to that manifest. No configuration is inferred from a vendor
project. Output parents must exist; aliased destinations and input replacement
are rejected. The source-only context never invents authenticated artifact hashes:
artifact-scoped facts require a different, explicitly authenticated consumer.

[Contracts](contracts/README.md) owns semantic identities, evidence classification
and applicability without execution dependencies. [Review](review/README.md) owns
reviewed-pack validation and conflict selection. [Model](model/README.md) owns
geometry, hardware semantics and PAC policies. The tool loads one model and policy
set, validates their evidence/ownership and renders all outputs from those values.
Reading several source documents is not an atomic filesystem snapshot; the loaded
values, rather than reopened paths, supply generation.

The host owns temporary files and the `rustfmt` process group. Formatting has a
60-second timeout and cancellation reaps the child group. Generated Rust is limited
to 128 MiB before reading it back. These trusted source-generation inputs and
third-party SVD rendering do not claim allocation-controlled binary analysis.
Blobray remains the owner of binary investigations.

Publication checks reference validity, selected applicability, reviewed semantics,
memory ranges, API safety and generated source consistency. They do not qualify
hardware or prove equivalence to vendor behavior. Review policy, source provenance
and required recovered tables remain with `registers/`.

## Checks the gate calls

`oer_register_tool::checks` holds the register rules other tools run, so no
caller keeps its own copy:

- `pac_transactions::check`: a handwritten PAC module
  (`crates/hardware/*/pac/src`) never loops, except a `while` walking an
  index over a fixed-length table; polling and retries belong to the HAL.
  `cargo xtask check architecture` runs it.
- `shared_words::check`: every MMIO word a write transaction of the radio
  PAC (`policy/api.toml`, through the published SVD) and pinned esp-hal both
  store to has a reviewed entry in `registers/<chip>/shared-words.toml`, and
  no reviewed word is no longer shared. `cargo xtask check architecture`
  resolves the esp-hal and platform PAC sources and runs it.
- `inventory`: the publication's owned MMIO regions and declared words
  (`model`), Blobray's `register-accesses` records (`observations`), their
  classification and the report; `inventory::run` (behind `cargo registers
  inventory`) runs Blobray over the pinned binaries under the RISC-V integer
  calling convention, ranks the provenance registry's cited functions first
  and writes the report. An indexed progression (an array element chosen at
  run time) touches each word of a small index bound (a mask), otherwise only
  declared words inside its bound: the elements of the declared register array
  at its stride that has its base as an element, and the run of declared words
  along it from its base. The report counts these touches as `indexed`: the
  vendor reaches the array, not provably that element.

`ChipSources` resolves the files a chip's publication manifest names
without validating them. The binding index format is
`oer_register_bindings::BindingIndex` ([`bindings/`](bindings/README.md)), a
foundation data format the generator writes and every reader (the vendor
harness's address naming, the stand's reset-cause read) parses.

A chip publication's `[outputs]` name the SVD and the binding index; its
optional `[outputs.pac]` (`raw`, `api`, `crate-name`, `target`, `edition`)
adds the raw PAC and its API facade, and a publication has a PAC API policy
(`api`) exactly when it has that table. A publication without one, a chip's
`platform.toml`, describes registers the host reads through JTAG and that
no firmware reaches: its binding index names no crate.
