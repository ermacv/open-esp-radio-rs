# Registers, SVD and PAC

MMIO discovery, the reviewed register model, SVD publication and PAC
generation are one pipeline with explicit trust boundaries.

## Discovery is evidence, not a specification

Binary analysis can discover addresses, access widths, masks, calling
functions and instruction provenance. It cannot reliably invent peripheral,
register or field names. Generated MMIO facts therefore remain review input.

The top-level register manifest uses schema 3 and requires an explicit `chip`
alongside its `address-space`; top-level schema-2 manifests are rejected,
while reusable peripheral fragments remain schema 2. The reviewed
register workspace owns accepted assertions: names, blocks,
access rules, reset facts, fields, enums, and links to supporting evidence. It
does not own or rewrite the underlying observations. Validation rejects
overlapping definitions, unreviewed access, missing evidence and unsafe API
exposure.

New human conclusions belong in the project manifest's explicit
`[reviewed-knowledge].default-pack`; Blobray never guesses a destination from
pack order or protocol naming. A stable
physical subject such as `register:esp32s31/cpu/0x20103064/32` can independently acquire a
`register-identity` or `hardware-write-semantics` assertion. The identity is
one scalar `REGION.NAME`; it atomically names the existing concrete non-array
peripheral/region and the register. If the physical register is absent from
the reusable model, that same assertion authorizes materializing it:

```toml
[[assertions]]
id = "radio.event-status.identity"
subject = "register:esp32s31/cpu/0x20103064/32"
kind = "register-identity"
value = "RADIO.EVENT_STATUS"
[[assertions.evidence]]
source = "reviewed-register-header"
locator = "RADIO event-status identity and offset"
```

The identity fails closed when it is not one scalar `REGION.NAME`, its address
space differs, the width or
alignment is invalid, the extent falls outside a published register address
block, the region is absent/array/derived, or the new extent aliases existing
geometry. The retired `register-declaration` and `register-name` kinds are
rejected; there is no compatibility or pair-merging path. Every applied
register or field assertion remains attached to the in-memory effective model
with its pack, classification, applicability and evidence. If a region has
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
ecosystem/chip/project applicability composition before application. Exact
artifact identities in that context come only from hashing the active run-spec
inputs, never from a project-manifest digest; missing revision or artifact
context and a context that selects multiple values for one subject/kind fail
closed. Complete SVD-shaped geometry is generated/reusable input, not the
normal unit of a human change.

A reusable fragment `[[review]]` entry closes review debt only when it has at
least one source, a complete provenance/accuracy/completeness classification,
and non-`hint` provenance. Incomplete entries and hints remain navigation
metadata; they never make generated geometry publishable.

## Normal workflow

```console
cargo blobray project research next --project path/to/vendor-project.toml
cargo blobray inspect register 0x20000000 --project path/to/vendor-project.toml
cargo blobray registers review --project path/to/vendor-project.toml
cargo blobray registers validate --project path/to/vendor-project.toml
cargo blobray project publish --project path/to/vendor-project.toml
```

`research next` ranks blockers by transitive benefit and co-blocking structure.
`inspect register` schema 7 prints the stable sparse-fact subject, configured
review pack and supported assertion kinds. It also lists every selected
reviewed assertion for the exact physical subject, including its pack, ID,
kind, value and evidence; that list has `completion_claim = false`. For an owned, unreviewed discovery
fact it also emits a raw TOML draft whose state is `review-required` and whose
`completion_claim` is always false. External, already reviewed,
non-operational-only, and model/catalog-only addresses receive no draft. A
reviewer replaces every `REVIEW_REQUIRED` value and adds only manually proven
facts with durable evidence to the exact destination. The draft lists
typed executable actions to validate the register workspace, rerun project
analysis, and query the exact current finding ID. Each action preserves argv
boundaries, absolute working directory and required project context. A later `not-present` finding only
means the ID is absent from current analyzed inputs; it is not proof of
correctness or completion. Generated review reports and MMIO inventory remain
disposable rather than hand-authored knowledge. Promote a conclusion from a
project pack into the chip model only when its applicability and evidence
support reuse by other blob revisions or projects.

For an unreviewed physical register, `registers review` emits only one sparse
`register-identity` template with a deliberately unresolved
`REVIEW_REQUIRED_REGION.REVIEW_REQUIRED_REGISTER_NAME` value. It never emits a
complete peripheral fragment and never promotes observed reads, writes, masks
or field partitions into access, W1C, self-clear or field assertions.

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

### Selected writes to SVD read-only registers

The closed-PAC API pack uses schema 4. Schema 3 is rejected; there is no
compatibility or migration path. Schema 4 adds one intentionally exceptional
operation for a hardware write which has exact reviewed evidence while the SVD
must remain read-only:

```toml
schema = 4

[[selected-register-writes]]
name = "write_event_status_selected_image"
peripheral = "RADIO"
register = "EVENT_STATUS"
value = 0x00000040
sources = ["HIL_EVENT_STATUS_SELECTED_IMAGE"]
```

A selected register write is accepted only for one non-array, readable but
SVD-nonwritable 32-bit register. Its evidence sources are resolved through the
same reviewed evidence catalog as every other PAC operation. The generated raw
helper requires a mutable peripheral owner and performs exactly:

```rust,ignore
core::ptr::write_volatile(registers.event_status().as_ptr(), 0x00000040);
```

The generator does not change the register access class, create a `Writable`
implementation, or accept a caller-supplied image. The review qualifies only
the named target and literal image. Ordering, isolation, event sampling,
acknowledgement checks and recovery remain responsibilities of the closed PAC
wrapper and HAL; this declaration alone is not a production-readiness claim.

## Existing SVDs

An existing SVD may bootstrap the model, but it does not bypass review.
Imported descriptions must still be reconciled with binary/HIL evidence and
the public API policy. Conflicts stay visible until resolved.
