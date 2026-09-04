# ESP32-S31 rev0 executable runtime adapters

This crate composes the RV32 C and ESP-IDF executable adapters with separately
owned chip declarations from `../knowledge`. Its `PROVIDER` identifies the
selected implementation as `runtime-semantics`, including revision and
applicability metadata. That registration is not a proof of model equivalence.

Runtime addon implementations retain their existing public-symbol/body-policy
checks. The fixed crystal value comes from declarative chip knowledge. This
provider does not install the investigation's private body reconstructions.

Change `PROVIDER.revision` and the corresponding suffix of
`RISCV_HARNESS.semantic_cache_domain` together when executable semantics change.
Registry validation rejects an inconsistent pair. Project stage identity and
persistent function facts then invalidate together.
