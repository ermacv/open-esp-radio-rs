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

### Fresh-read field replacement

`[[field-replace-modifies]]` declares one ordinary read-write register update
that replaces named SVD fields and preserves every other bit from the same
fresh read. Each field selects bits from either one zero-based bounded domain
or one fixed logical value. Blobray derives destination masks, shifts and Rust
field widths from the reviewed SVD and rejects duplicate/read-only fields,
out-of-range source projections and values outside the projected source
image. Generated code owns the required unsafe field-writer calls; handwritten
PAC/HAL code receives only the typed or argument-free accessor. Separate
declarations remain separate volatile reads, so an evidenced sequence of RMW
operations is not silently collapsed into a software register image.

### Bitwise-composed register inputs

`[[bitwise-composed-domains]]` declares a register-specific logical input when
the reviewed vendor encoder shifts and ORs typed arguments before applying one
final mask. This is intentionally distinct from independent field projection:
overlapping contributions are preserved, including high argument bits that
reach an adjacent physical field. Blobray validates the argument types, names,
shift bounds, register binding and output mask, then emits a crate-private
composer. Handwritten PAC code supplies only the logical arguments and cannot
construct or inspect the packed register image. When the domain feeds a
`[[masked-register-modifies]]` transaction, publication also rejects any
composed bit that the transaction would silently truncate.

### Sample-and-zero bit publication

`[[sampled-bit-zero-writes]]` declares the non-RMW transaction used when the
hardware sequence samples one ordinary readable/writable bit and then writes
that sample into an otherwise all-zero register image. Blobray rejects arrays,
multi-bit or read/write-restricted fields, modified-write semantics and read
side effects. The generated leaf owns both volatile accesses, so handwritten
PAC and HAL code cannot accidentally turn the transaction into an RMW or reach
for a raw writer.

### Field observations

`[[field-reads]]` declares one side-effect-free SVD field observation. Blobray
derives the smallest ordinary Rust return type from the reviewed field width,
supports explicitly indexed register arrays, and rejects write-only fields or
register/field read actions. The generated restricted-PAC leaf owns the
register lookup and read, while handwritten code receives only the field
value needed by its state machine. `signed = true` is accepted only for a
complete 8-, 16-, or 32-bit field and moves the reviewed two's-complement
reinterpretation into the generated accessor.

When several fields must come from the same volatile observation,
`[[field-snapshot-reads]]` lists them in return order. Blobray validates every
field with the same read rules and emits one register read followed by a tuple
of field values. This prevents handwritten sequencing from accidentally
turning one evidenced sample into several temporally different MMIO reads.

### Complete-word observations and raw-only leaves

Schema-5 API packs may declare a reviewed 32-bit observation with
`[[full-register-reads]]`. Publication accepts only one non-array readable
32-bit register with one full-width readable field, rejects read-side effects,
and requires the declared domain to represent every `u32`. The generated raw
leaf returns the complete word without an address, mask or handwritten
accessor.

Every operation kind that can emit a direct restricted-PAC bridge declares an
explicit `exposure`. `exposure = "raw-only"` is used when a hand-written affine
owner must mediate the generated leaf; `exposure = "facade"` requests the
direct bridge. Raw-only value domains are still generated for the restricted
PAC, but no redundant full-block facade function is emitted. There is no
implicit/default exposure and no compatibility escape hatch.

### Affine snapshots for same-register W1C fields

The closed-PAC API pack uses schema 5. Schema 4 is rejected; there is no
compatibility or migration path. A same-register status/acknowledgement path is
declared as one transaction rather than as a general writable register or an
untyped integer write:

```toml
schema = 5

[[w1c-register-snapshots]]
name = "event_status"
peripheral = "RADIO"
register = "EVENT_STATUS"
field = "EVENTS"
sources = ["PUBLIC_EVENT_STATUS_W1C"]
```

A W1C snapshot is accepted only for one non-array 32-bit read-write register
whose reviewed SVD semantic is `oneToClear`. Its selected field is masked from
one register read. The generated token is `must_use`, has no public constructor,
and is neither `Copy` nor `Clone`. Acknowledge consumes the token and writes
exactly its masked image back to the same register:

```rust,ignore
let snapshot = sample_event_status(registers);
inspect(snapshot.bits());
acknowledge_event_status(registers, snapshot);
```

The generator never accepts a caller-supplied acknowledgement image, and the
affine token cannot be replayed. Ordering, interrupt masking, dispatch and
recovery remain responsibilities of the closed PAC wrapper and HAL; this
declaration alone is not a production-readiness claim.

## Existing SVDs

An existing SVD may bootstrap the model, but it does not bypass review.
Imported descriptions must still be reconciled with binary/HIL evidence and
the public API policy. Conflicts stay visible until resolved.
