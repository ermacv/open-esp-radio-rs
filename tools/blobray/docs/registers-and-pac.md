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
physical subject such as `mmio:cpu:0x20103128/32` can independently acquire a
`register-name` or `hardware-write-semantics` assertion. Observed software
reads/writes remain generated evidence; W1C, self-clear and trigger behavior
remain absent or explicitly unknown until reviewed hardware evidence proves
them. Suspected incorrect vendor access is a separate vendor-bug record and
must not redefine the hardware semantic.

Sparse packs are already validated and inventoried as reviewed project inputs.
The layered register-model cutover composes reusable chip geometry, generated
review candidates and those sparse assertions before publication. Conflicting
assertions with overlapping applicability already fail closed; until the
cutover is complete, existing schema-2 register fragments remain the
publication input. Complete SVD-shaped generated geometry is the target
output, not the normal unit of a human change.

## Normal workflow

```console
cargo blobray registers review --project path/to/vendor-project.toml
cargo blobray registers validate --project path/to/vendor-project.toml
cargo blobray project publish --project path/to/vendor-project.toml
```

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
