# Interfaces, identities and formats

Reference application owners, content identities, result assessment and native formats.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Owners and interfaces

| Crate | Responsibility | Internal dependencies |
| --- | --- | --- |
| [domain](../../../crates/domain/README.md) | Identities, records, limits and portable control/stream ports | None |
| [artifacts](../../../crates/artifacts/README.md) | ELF/AR inventory over borrowed captured bytes | Domain |
| [verification](../../../crates/verification/README.md) | Concrete observation comparison and verdicts | Domain |
| [analysis](../../../crates/analysis/README.md) | Local CFG, values and memory effects | Domain |
| [riscv](../../../crates/riscv/README.md) | RV32 decoding, lifting and relocation interpretation | Domain |
| [application](../../../crates/application/README.md) | Operations over given executables: inventory, library analysis, linking, data, audit and in-process comparison | Domain, artifacts, analysis, verification |
| [linker](../../../crates/linker/src/lib.rs) | Linux adapters of the external ELF linkers behind `LinkerHost` | Domain, application |
| [cli](../../src/main.rs) | CLI rendering and host composition | Domain, application, backend-riscv |

Every operation is a function call over `blobray_application::in_process::Executable`
values, a `WorkingMemory` and a `RunControl`; the
[contracts](../../../docs/design/contracts.md#operations-in-process) list them.
Libraries do not discover executables, install signal handlers or change
process-global state. The host supplies the decoder/executor and the linker
adapter.

## Identity and schema 1

[Domain records](../../../crates/domain/src/lib.rs) define identities:

- `ArtifactId` hashes exact bytes with SHA-256.
- `ObjectId` combines the container artifact with standalone or zero-based archive
  payload ordinal. Archive index/name-table metadata does not consume ordinals.
- `SymbolId` includes object, table kind, table section and entry index.

Inventory retains sections, static/dynamic symbols including null/local/weak/
common/undefined entries, raw metadata and REL/RELA references. Malformed names,
invalid references, unreadable members and unknown remaining membership are
explicit diagnostics. Packed RELR decoding and nested archive expansion are
unsupported. Inventory never chooses linker definitions or proves equivalence.
The initial interpretation target is `riscv32-ilp32`; structural ELF32/64 reading
is independent of target execution support.

## JSON and checks

Typed clients decode `--format json` output with the host library's
`blobray_cli::wire` module. The renderer emits the same types, and their
payloads are the application and domain records, so clients never copy a
schema.

### Current formats

| Document | Schema | Shape |
| --- | --- | --- |
| `audit-targets` | `TARGET_AUDIT_SCHEMA` 1 | `{schema, artifact, decoder, semantics, ranges, records, summary, verdict}` |
| `register-accesses` | `REGISTER_ACCESSES_SCHEMA` 6 | `{schema, inputs, abi, records, groups?, summary}`, records streamed; `groups` only with `--group-by`; a store observation carries `stored`, an unresolved one its `indexed` progression when it has one; `summary` (its own `schema` 3) carries `partial_causes` and `indexed_addresses` |
| `field-accesses` | `FIELD_ACCESSES_SCHEMA` 3 | `{schema, inputs, abi, offset, width, functions, blocked, partial, partial_causes, gaps, unknown_addresses}` |
| `call-arguments` | `CALL_ARGUMENTS_SCHEMA` 2 | `{schema, inputs, abi, symbols, callers, blocked, partial, partial_causes, gaps}` |
| `symbols` | `SYMBOLS_SCHEMA` 1 | `{schema, inputs, symbols}` |
| `strings` | `STRINGS_SCHEMA` 1 | `{schema, inputs, strings}` |
| Linked image manifest | 4 | `ImageManifest` |
| Execution request | `EXECUTION_SCHEMA` 25 | `ExecutionRequest` |
| Errors | 1 | `{schema:1, error:{code, message, memory}}` on stderr |

`partial_causes` is `{decoding, control_flow, references, opaque_calls,
value_limits, other}`: each the number of partial functions with that cause
(an undecodable instruction; an unexpanded indirect jump, conflicting
boundaries or an instruction with an unknown relocation; an unresolved
relocation; a call whose callee is not analyzed; a value whose alternatives
exceeded their bound; none of these). A function with several causes counts
under each, so the causes can sum to more than `partial`, and every partial
function counts under at least one. The human summaries list the nonzero
causes as `(by cause: …)`.

Partial research exits 0. A failed or inconclusive audit exits nonzero with its
document on stdout. Other failures print the error document and exit nonzero.
Error codes are machine interfaces; descriptive prose is not a parsing key.

```console
cargo test --manifest-path tools/blobray/Cargo.toml --workspace
cargo clippy --manifest-path tools/blobray/Cargo.toml --workspace --all-targets
cargo xtask check blobray-standalone
```

Tests use synthetic binaries. The standalone check extracts only the shipping
core and runs its tests. It excludes the independent register
source-publication tool.
