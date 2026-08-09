# Persistent artifact schemas

Persistent project artifacts are reusable evidence files, not command-result
envelopes. Their identities and Serde producers live in `src/artifacts`; CLI
adapters only publish an already-built document. Consumers validate both the
version and command identity and fail closed on schema drift. There are no
old-schema compatibility readers.

| Artifact | Version | Command identity | Owner |
| --- | ---: | --- | --- |
| Symbol inventory | 3 | `symbols inventory` | `artifacts/symbol_inventory.rs`, `artifacts/symbol_inventory/read.rs` |
| MMIO discovery facts | 4 | `mmio discover` | `artifacts/mmio_facts.rs`, `artifacts/mmio_facts_read.rs` |
| Interface discovery facts | 4 | `interfaces discover` | `artifacts/interface_facts.rs`, `artifacts/interface_facts_read.rs` |
| Linked IR | 36 | `ir export` | `artifacts/linked_ir_document.rs`, `artifacts/linked_ir_read.rs` |

MMIO/interface schema 4 and linked-IR schema 36 carry reviewed-code-boundary
provenance. MMIO and interface artifacts record the accepted boundary count per
input. Linked IR retains the complete reviewed physical ranges so downstream
reviewers can distinguish ordinary ELF symbol roots from promoted gap roots.

`artifacts/mod.rs` is the only owner of these version/command constants.
Domain workspaces and navigation use the corresponding typed Serde consumer
projections. They neither repeat numeric schema literals nor walk arbitrary
`serde_json::Value` trees. Producer and consumer DTOs are separate only where
the complete report directly serializes live analysis-domain structures;
identity and supported claims are still validated once in the artifact layer.
Persistent consumer DTOs describe the complete stored document and apply
`deny_unknown_fields` recursively. Consequently both removed required fields
and unversioned additions fail closed instead of being silently ignored.
Contract tests build canonical fixtures with the producer wherever practical,
then exercise the same strict reader used by downstream workspaces.
The navigation join consumes these projections directly; it has no shortened
copies of the symbol, interface or linked-IR envelopes.

Invocation reports such as `project analyze`, `project publish`, `ir build`
and `project status` are deliberately separate typed models. Their `schema`
field versions a command result, not a stored evidence artifact. Publication
metadata belongs to those command reports and is not embedded into persistent
symbol/MMIO/interface/linked-IR data.
