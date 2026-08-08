# Persistent artifact schemas

Persistent project artifacts are reusable evidence files, not command-result
envelopes. Their identities and Serde producers live in `src/artifacts`; CLI
adapters only publish an already-built document. Consumers validate both the
version and command identity and fail closed on schema drift. There are no
old-schema compatibility readers.

| Artifact | Version | Command identity | Owner |
| --- | ---: | --- | --- |
| Symbol inventory | 2 | `symbols inventory` | `artifacts/symbol_inventory.rs` |
| MMIO discovery facts | 2 | `mmio discover` | `artifacts/mmio_facts.rs` |
| Interface discovery facts | 3 | `interfaces discover` | `artifacts/interface_facts.rs` |
| Linked IR | 35 | `ir export` | `artifacts/linked_ir_document.rs` |

`artifacts/mod.rs` is the only owner of these version/command constants.
Domain workspaces and navigation use the corresponding strict projections;
they do not repeat numeric schema literals or walk arbitrary
`serde_json::Value` trees at the frontend boundary.

Invocation reports such as `project analyze`, `project publish`, `ir build`
and `project status` are deliberately separate typed models. Their `schema`
field versions a command result, not a stored evidence artifact. Publication
metadata belongs to those command reports and is not embedded into persistent
symbol/MMIO/interface/linked-IR data.
