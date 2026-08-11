# Persistent artifact schemas

Persistent project artifacts are reusable evidence files, not command-result
envelopes. Their identities and Serde producers live in `src/artifacts`; CLI
adapters only publish an already-built document. Consumers validate both the
version and command identity and fail closed on schema drift. There are no
old-schema compatibility readers.

Large generated JSON artifacts use compact canonical serialization to reduce
cold-build time, peak transient memory and repository size. Whitespace is not
part of their contract; consumers must parse the typed structure.

| Artifact | Version | Command identity | Owner |
| --- | ---: | --- | --- |
| Symbol inventory | 4 | `symbols inventory` | `artifacts/symbol_inventory.rs`, `artifacts/symbol_inventory/read.rs` |
| MMIO discovery facts | 5 | `mmio discover` | `artifacts/mmio_facts.rs`, `artifacts/mmio_facts_read.rs` |
| Interface discovery facts | 5 | `interfaces discover` | `artifacts/interface_facts.rs`, `artifacts/interface_facts_read.rs` |
| Linked IR | 42 | `ir export` | `artifacts/linked_ir_document.rs`, `artifacts/linked_ir_read.rs` |
| Review scopes | 7 | `project analyze` | `review_scopes.rs`, `review_scopes/model.rs` |
| Verification report | 7 | `verify source` / `verify inventory` | `verification/report.rs` |
| Project verification report | 6 | `project verify` | `verification/project_report.rs` |

Verification report artifact paths are canonical absolute paths. Verification
currency therefore does not depend on the process working directory used by a
later `project status` or `project check`; generated reports remain local
project state and are regenerated when the checkout moves.

MMIO schema 5, interface schema 5 and linked-IR schema 43 carry
reviewed-code-boundary provenance. MMIO and interface artifacts record the accepted boundary count per
input. Linked IR retains the complete reviewed physical ranges so downstream
reviewers can distinguish ordinary ELF symbol roots from promoted gap roots.
Linked-IR schema 43 also carries static data objects, uninterpreted initializer
bytes, symbolic data relocations and function xrefs. Relocatable archive
members retain section-relative offsets; these are evidence identities, never
invented runtime addresses or nominal source types.
MMIO schema 5 adds exact instruction PCs for every direct read/write finding;
the locations remain best-effort evidence and do not strengthen the artifact's
explicit `completeness_claim = false` contract.
Interface schema 5 replaces symbol-wide decode failures with per-instruction
`decode_blockers` carrying PC, raw word, width, extension class and whether
linear continuation is architecturally safe. Linked-IR blockers are restricted
to instructions reachable from the function entry, rather than every
unsupported byte in the symbol's declared extent. Malformed artifact failures are
kept separately as `analysis_failures`. The classification vocabulary keeps an
all-zero illegal encoding distinct as `zero-fill-or-illegal-trap`: it may be a
deliberate trap or unreachable fill, but is not evidence that the decoder lost
an otherwise valid instruction.

Review-scope schema 7 records the exact explicit `source:symbol` replacement
roots. Feature-pack schema 2 uses that list as a fail-closed effect-coverage
denominator; it never derives the release boundary from presentation text or
from the transitive helper closure.

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

Invocation reports such as `project analyze`, `project check`, `project
publish`, `ir build` and `project status` are deliberately separate typed
models. Their `schema`
field versions a command result, not a stored evidence artifact. Publication
metadata belongs to those command reports and is not embedded into persistent
symbol/MMIO/interface/linked-IR data.

`project status` currently emits command-result schema 3. Each non-ready
component may carry a typed `next_action`; human output groups identical
actions while JSON keeps the action attached to every responsible component.
The `inventory` component state denotes valid artifact-wide review debt that
does not gate project readiness. Configured publication scopes remain the review
gate.
`project analyze` emits command-result schema 2, which distinguishes a
content-verified write-mode `up-to-date` stage from a stage executed as
`written` or `verified`.
`project check` emits command-result schema 2 and combines the non-mutating
analysis, verification and publication verdicts without embedding or
duplicating their persistent evidence documents. Every failed aggregate stage
contains typed component issues and one concrete next action; verification
summary counts distinguish mismatches, incomplete comparisons and implemented
but unqualified replacements.
