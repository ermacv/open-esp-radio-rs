# Sparse reviewed knowledge

`open-radio-vendor-review` owns the generic, human-sized facts that survive
regeneration of Blobray observations and migration between vendor artifacts.

The crate deliberately does not know about ESP chips, register names, ABI
catalogs, SVD, ELF, qualification policy or output paths. Consumers interpret
the opaque `subject`, `kind` and `value` fields. The generic layer validates:

- stable pack, assertion and vendor-bug identifiers;
- evidence, provenance and applicability;
- artifact SHA-256 guards;
- fail-closed conflicts between assertions that apply to the same subject;
- deterministic merge and semantic fingerprints.

Generated observations are not accepted facts and do not belong in these
packs. A project should add a record only after a reviewer accepts a name,
hardware semantic, ABI meaning or vendor defect.

Register-model migration consumes the effective classification, applicability,
evidence and semantic fingerprint from this crate without rewriting a pack.
The read-only workflow is documented in
[`../../docs/register-migration.md`](../../docs/register-migration.md); a
duplicated base value is only an extraction candidate and never weakens the
review requirement.

Register-model consumers reserve `register-declaration` as the explicit
authorization to add geometry that is absent from the reusable base model.
Its MMIO subject owns address space, address and width; its string value names
an existing peripheral/region. A separate `register-name` assertion supplies
the required SVD identity. Both records carry evidence and must have identical
effective applicability. Merely observing software reads or writes never
constitutes a declaration and never proves hardware access or W1C semantics.
