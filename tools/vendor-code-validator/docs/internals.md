# Validator internals

The binary entry point only translates the library result into an exit code.
The implementation is split by responsibility:

- `crates/core` is a zero-dependency, architecture-neutral contract model;
- `crates/model` owns architecture-neutral symbolic/reference IR, indexed-MMIO
  proofs and the SVD-derived register catalog;
- `crates/semantic` owns architecture-neutral effect-policy, qualification
  request/result and evidence-source interfaces;
- `crates/backend-riscv` owns ELF decoding, RISC-V relocations, reference CFG
  analysis, code generation, concrete RV32 execution and image auditing;
- `crates/harness-esp32s31` owns the external ABI versions and lifecycle
  fixture data and depends only on core;
- `crates/harness-esp32s31-semantic` owns reviewed summaries, typed
  qualification and the only validator-side dependency on the production PHY;
- `analysis` contains the thin architecture-facing artifact service;
- `orchestration` owns cross-layer workflows such as compiling and
  independently re-extracting a generated reference;
- `verification` owns profiles, dispositions, evidence and comparisons;
- `harnesses::esp32s31` is a thin registry facade over the two ESP32-S31
  harness crates;
- `validation/esp32s31` owns the checked target/profile/disposition data;
- `cli` parses a typed top-level command and dispatches it to those services.

The backend depends only on the neutral core/model crates. Chip-specific
secondary-return recognition and reviewed summaries are supplied through the
typed `RiscvHarnessSpec`; the backend contains no platform registry. The
facade does not depend directly on the production PHY: that dependency ends at
the ESP32-S31 semantic harness boundary.

The hierarchical workflows are `inspect`, `mmio`, `ir`, `reference`, `execute`,
`verify`, and `image`. Legacy flat command spellings remain accepted during
the migration. The remaining orchestration and additional-backend work is
tracked in
[`docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md`](../../../docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md).

## Shared trace and effect-contract layout

`crates/model/src/ir/trace.rs` is a compatibility façade that preserves the
public `ir::*` API. Its child modules separate stable data from queries:

| Module | Responsibility |
| --- | --- |
| `events.rs` | Observable effects, draft reference events, and event formatting |
| `flow.rs` | Draft CFG types and ABI-input collection |
| `validation.rs` | Call-result availability and fail-closed flow validation |
| `function.rs` | `FunctionAnalysis`, eligibility, and inventory queries |
| `tests.rs` | Function-level trace classification tests |

`crates/semantic/src/effect_contract.rs` similarly keeps the existing public
exports while delegating to:

| Module | Responsibility |
| --- | --- |
| `model.rs` | Closed effect vocabulary, selectors, dispositions, and policy |
| `parser.rs` | Fail-closed textual policy parser |
| `compare.rs` | Observable extraction and vendor/Rust effect comparison |
| `tests.rs` | Parser, replacement, omission, and comparison tests |

The comparator consumes `EffectPolicy` through its public query methods.
Parser and comparison logic do not access the policy's internal rule map.

## ESP32-S31 reviewed-summary layout

`harness-esp32s31-semantic/src/reviewed_summaries.rs` is the single registry
facade used by the backend hooks. It applies reviewed recognizers in an
explicit order and delegates exact identity checks and trace construction to
subsystem modules:

| Module | Responsibility |
| --- | --- |
| `body_identity.rs` | Shared exact name, address, and body-size identity predicate |
| `direct_semantic.rs` | Direct semantic overlay for the reviewed `pp_post` body and relocation schema |
| `intrinsics.rs` | Bounded `memcpy`/`memset` effects and the reviewed wide signed divide intrinsic |
| `rf.rs` | RFPLL calibration, frequency-offset scratch, and IQ-estimator traces |
| `i2c.rs` | Analog-I2C register access and host-table summaries |
| `tests.rs` | Fail-closed identity and generated-trace regression tests |

The subsystem modules do not form independent registries. The facade remains
the auditable selection point, while each module owns the exact identities,
constants, and semantic trace builders for one domain. Artifact
authentication remains a caller-owned precondition; these summaries only
match the reviewed symbol metadata and body/schema constraints they declare.

## RISC-V structural-analysis layout

`backend-riscv/src/static_analysis/mod.rs` owns the fail-closed instruction
walk and dispatch order. Its supporting modules own state and semantic units
that do not need to control that walk:

| Module | Responsibility |
| --- | --- |
| `alu.rs` | Register-only RV32 integer semantics and ALU-related relocations |
| `calls.rs` | Relocated, direct, function-table, and external-ABI call semantics |
| `context.rs` | Relocated calls, pointer cells, tables, and harness summary context |
| `memory.rs` | Effective addresses, data relocations, indexed-memory proofs, and bounded memory intrinsics |
| `memory_access.rs` | Load/store effects over MMIO, ELF memory, caller RAM, and private stack |
| `poll.rs` | Structural polling-loop recognition and checkpoint validation |
| `stack.rs` | Symbolic private stack and RV32 call-argument recovery |
| `state.rs` | Register file, effect streams, blockers, tokens, checkpoints, and trace finalization |

ALU evaluation cannot emit effects or change instruction traversal. Memory
dispatch mutates one `StructuralTraceState` and cannot select the next PC.
Poll recovery consumes immutable trace prefixes plus an explicit checkpoint,
which the state owner restores atomically. The orchestrator retains only call
control results, branch/local-jump decisions, and instruction traversal. Call
semantics return `NotCall`, `Advance(n)`, or `Stop`; they cannot mutate the
orchestrator's instruction index.

## RISC-V reference-analysis layout

`backend-riscv/src/reference_analysis/mod.rs` is the entry facade for
reference-trace resolution. It selects a reviewed summary, a structural trace,
or bounded symbolic-CFG recovery and then delegates composition:

| Module | Responsibility |
| --- | --- |
| `inline.rs` | Argument substitution and token remapping for a flat callee summary |
| `flatten.rs` | Stateful straight-line call composition, caller private stack, and call-result rewriting |
| `flow.rs` | Bounded CFG exploration, scoped calls, and recursive flow composition |
| `resolver.rs` | Artifact symbol catalog, preferred identities, relocations, and entry-point resolution |

The facade owns path selection and fail-closed blocker attribution. `inline`
does not resolve symbols, `flatten` does not explore branches, and `resolver`
does not implement event semantics. Recursive callee analysis returns through
the facade so the same selection and blocker rules apply at every call depth.

## Linked-IR source layout

`analysis/linked_ir.rs` is the façade for building, merging, and
project-linking reports. Its child modules own one analysis phase each:

| Module | Responsibility |
| --- | --- |
| `model.rs` | Stable linked-IR report types |
| `identity.rs` | Function identity catalog and diagnostic compaction |
| `calls.rs` | Call normalization, typed arguments, and semantic annotations |
| `direct_trace.rs` | Direct call-graph exploration and guarded MMIO provenance |
| `provenance.rs` | Return-bit provenance and wrapper traversal |
| `effects.rs` | Direct MMIO, delay, and context-access extraction |
| `summary.rs` | Reachable effect/context/event-dispatch projection |
| `register_index.rs` | Register inventory and candidate-field aggregation |
| `pseudo.rs` | Best-effort pseudo-Rust rendering |
| `tests/...` | Tests grouped by calls, guards, MMIO flow, summaries, and recursion |

The façade is the only module that schedules symbols and assembles a complete
`LinkedIrReport`. Analysis modules may consume the stable model and shared
helpers, but report rendering does not feed facts back into analysis.

## IR export source layout

`cli/commands/export_ir.rs` parses options, invokes linked analysis, and
selects outputs. Rendering and input validation are separate:

| Module | Responsibility |
| --- | --- |
| `input.rs` | Artifact syntax, source names, and namespace validation |
| `human.rs` | Tabular terminal report |
| `pseudo.rs` | Pseudo-Rust file output |
| `render_common.rs` | Shared guard/MMIO formatting and traversal |
| `json_report.rs` | JSON document orchestration |
| `json_report/values.rs` | JSON encoders for individual IR values |
| `tests.rs` | CLI input compatibility and validation tests |

Renderers are consumers of `LinkedIrReport`; they must not independently
recover calls, guards, MMIO fields, or semantic actions. This keeps JSON,
pseudo-Rust, and terminal views consistent.

## Remaining large-file review

Line count is only a signal, but the next useful responsibility reviews are:

- `backend-riscv/src/execution/image.rs`: separate ELF/relocation loading,
  code-closure identity, coverage traversal, and byte/instruction access while
  retaining `ExecutableImage` as the stable facade.

These should be split only at ownership and invariant boundaries. Moving a
contiguous block into another file without reducing shared mutable state or
clarifying the dependency direction is not an architectural improvement.
