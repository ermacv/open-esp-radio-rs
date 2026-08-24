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
