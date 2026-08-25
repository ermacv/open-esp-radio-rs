# Registers, SVD and PAC

MMIO discovery, the reviewed register model, SVD publication and PAC
generation are one pipeline with explicit trust boundaries.

## Discovery is evidence, not a specification

Binary analysis can discover addresses, access widths, masks, calling
functions and instruction provenance. It cannot reliably invent peripheral,
register or field names. Generated MMIO facts therefore remain review input.

The reviewed register workspace owns accepted assertions: names, blocks,
access rules, reset facts, fields, enums, and links to supporting evidence. It
does not own or rewrite the underlying observations. Validation rejects
overlapping definitions, unreviewed access, missing evidence and unsafe API
exposure.

New human conclusions belong in sparse `[reviewed-knowledge]` packs. A stable
physical subject such as `mmio:cpu:0x20103064/32` can independently acquire a
`register-name` or `hardware-write-semantics` assertion. If that physical
register is absent from the reusable model, exactly one explicit
`register-declaration` assertion may authorize creating it. Its string value
names an existing, concrete, non-array peripheral/region; a separate
`register-name` for the same subject and effective applicability supplies the
SVD identity:

```toml
[[assertions]]
id = "radio.event-status.declaration"
subject = "mmio:cpu:0x20103064/32"
kind = "register-declaration"
value = "RADIO"
[[assertions.evidence]]
source = "REVIEWED_REGISTER_HEADER"
locator = "RADIO event-status offset"

[[assertions]]
id = "radio.event-status.name"
subject = "mmio:cpu:0x20103064/32"
kind = "register-name"
value = "EVENT_STATUS"
[[assertions.evidence]]
source = "REVIEWED_REGISTER_HEADER"
locator = "RADIO event-status name"
```

The declaration fails closed when its address space differs, the width or
alignment is invalid, the extent falls outside a published register address
block, the region is absent/array/derived, or the new extent aliases existing
geometry. Its effective assertion (pack, classification, applicability and
evidence) remains attached to the in-memory effective model. If a region has
no address blocks, the explicit reviewed region assertion is the ownership
boundary and the offset must still fit the SVD representation.

Observed software reads/writes remain generated evidence; they neither create
model geometry nor infer read/write access. W1C, self-clear and trigger
behavior remain absent or explicitly unknown until a separate reviewed
hardware-semantic assertion proves them. Suspected incorrect vendor access is
a separate vendor-bug record and must not redefine the hardware semantic.

Sparse packs are validated, inventoried and applied over the reusable base
model before inspection, validation, SVD/PAC publication and cache-key
construction. `[registers].model` may override the base for an experiment, but
normally `chip.toml` owns `register-model` and the project only selects sparse
review packs. Conflicting assertions with overlapping applicability fail
closed during merge. Disjoint revision facts are filtered through the
ecosystem/chip/project applicability composition before application; missing
revision context and a context that selects multiple values for one
subject/kind also fail closed. Complete SVD-shaped geometry is
generated/reusable input, not the normal unit of a human change.

## Normal workflow

```console
cargo blobray project research next --project path/to/vendor-project.toml
cargo blobray inspect register 0x20000000 --project path/to/vendor-project.toml
cargo blobray registers review --project path/to/vendor-project.toml
cargo blobray registers validate --project path/to/vendor-project.toml
cargo blobray project publish --project path/to/vendor-project.toml
```

`research next` ranks blockers by transitive benefit and co-blocking structure.
`inspect register` prints the stable sparse-fact subject, configured review
pack and supported assertion kinds. A reviewer then adds only the proven fact
and its durable evidence to a project pack. The generated review report and
MMIO inventory are regenerated; they are not committed as hand-authored
knowledge. Promote a conclusion from a project pack into the chip model only
when its applicability and evidence support reuse by other blob revisions or
projects.

Publication produces the configured clean SVD, raw PAC/register
representation and restricted capability API. Outputs are reproducible from
reviewed inputs and may be checked with `project check`.

## PAC boundary

PAC generation remains a core Blobray responsibility:

- the raw PAC represents reviewed hardware structure;
- the bindings index maps reviewed registers to generated Rust items;
- the safe API pack exposes only operations intentionally approved for the
  production driver;
- generated code is never hand-edited.

Rust type safety proves memory/API properties, not reverse-engineered hardware
meaning. A typed field or enum is trustworthy only to the extent of its
reviewed register assertion. Raw addresses belong in reviewed low-level
boundaries, validation probes or generated PAC code, not scattered across
driver logic.

The production dependency direction is strict:

```text
reviewed model -> generated PAC -> radio HAL -> Wi-Fi/BLE/SoftMAC driver
```

The restricted PAC owns register-local access, masks, field/enum types and
non-forgeable authority. The HAL owns hardware sequences, waits, retry limits,
lifecycle transitions, recovery, and shared-access serialization. A driver
must not obtain a PAC owner, directly or through an HAL re-export/arena, and
must not orchestrate several PAC operations. Capability granularity follows
real sharing and exclusivity requirements; it need not mirror an SVD block and
does not require a mechanical `split()` API. Hand-written PAC modules are
removed one vertical slice at a time only after a generated equivalent exists;
analysis results never become production register access automatically.

The ESP32-S31 project configures these outputs under `[registers.svd]`,
`[registers.pac-raw]`, `[registers.bindings]` and `[registers.api]` in its
project manifest.

## Existing SVDs

An existing SVD may bootstrap the model, but it does not bypass review.
Imported descriptions must still be reconciled with binary/HIL evidence and
the public API policy. Conflicts stay visible until resolved.
