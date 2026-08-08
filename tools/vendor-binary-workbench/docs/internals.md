# Workbench internals

The binary entry point only translates the library result into an exit code.
The implementation is split by responsibility:

- `crates/contracts` is a zero-dependency, architecture-neutral contract model;
- `crates/analysis-model` owns architecture-neutral symbolic/reference IR, indexed-MMIO
  proofs and the SVD-derived register catalog;
- `tools/register-model` owns the editable schema-2 hardware model, clean SVD
  encoding, generic model invariants and reusable PAC/evidence pack schemas;
- `crates/semantics` owns architecture-neutral effect-policy, verification
  request/result and evidence-source interfaces;
- `crates/backend-riscv` owns ELF decoding, RISC-V relocations, reference CFG
  analysis, code generation, concrete RV32 execution and image auditing;
- `crates/harness-esp32s31` owns the external ABI versions and lifecycle
  fixture data and depends only on contracts;
- `crates/harness-esp32s31-semantic` owns reviewed summaries, typed
  verification and the only workbench-side dependency on the production PHY;
- `analysis` contains the thin architecture-facing artifact service;
- `orchestration` owns cross-layer workflows such as compiling and
  independently re-extracting a generated reference;
- `platform_pack` validates the project-level composition of target ABI,
  optional harness and reusable semantic catalogs without teaching those
  semantics to the backend;
- `project_analysis` owns project-level generic analysis artifacts such as the
  complete symbol inventory and their output-collision invariants;
- `cli/commands/project_navigation` builds the optional navigation-only join
  over symbol, linked-IR and interface reports without feeding facts or
  semantics back into those analyzers;
- `verification` owns profiles, dispositions, evidence and comparisons;
- `harnesses::esp32s31` is a thin registry facade over the two ESP32-S31
  harness crates;
- `verification/vendor/targets/esp32s31` owns the checked target/profile/disposition data;
- `cli` parses a typed top-level command and dispatches it to those services.

The backend depends only on the neutral contracts/analysis-model crates. Chip-specific
secondary-return recognition and reviewed summaries are supplied through the
typed `RiscvHarnessSpec`; the backend contains no platform registry. The
facade does not depend directly on the production PHY: that dependency ends at
the ESP32-S31 semantic harness boundary.

Register discovery facts and coverage remain in the workbench facade. The
shared register-model crate knows neither artifacts nor targets. A project may
attach reviewed safe PAC transactions through `[registers.api]` and evidence
catalogs through `[registers.evidence]`; their contents remain target-owned.
The project-owned register commands are the primitive SVD/PAC publication
operations; `project publish` is their strict, preflighted project-level entry
point.

The hierarchical workflows are `inspect`, `mmio`, `ir`, `reference`, `execute`,
`verify`, and `image`. Removed flat command spellings are rejected. The
remaining orchestration and additional-backend work is
tracked in
[`docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md`](../../../docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md).

## CLI boundary

The CLI has one declarative grammar and no raw-argument or compatibility
dispatch path:

| Module | Responsibility |
| --- | --- |
| `cli/args.rs` | `clap` workflow/subcommand hierarchy, global options, command capability policy and `ParsedInvocation` |
| `cli/arguments.rs` | Typed leaf-command arguments, declarative conflicts and value grammar |
| `cli/resolver.rs` | Project discovery, project/target/run-spec composition, precedence-aware defaults and owned `ResolvedInvocation` |
| `cli/resolver/defaults.rs` | Typed CLI > run-spec > project/target argument-default merge |
| `cli/resolver/register_catalog.rs` | SVD plus reviewed register-model composition |
| `cli/resolver/tests.rs` | Resolution precedence, discovery and path-origin contract tests |
| `cli/dispatch.rs` | Exhaustive routing of fully resolved invocations into domain workflows |
| `cli/output.rs` | Single stdout boundary and `human`, `json`, `jsonl`, and `tsv` result rendering |
| `cli/ui.rs` | miette diagnostics and tracing configuration on stderr |
| `cli/mod.rs` | Thin parse → UI initialization → resolve → dispatch composition root |
| `cli/commands/*` | Domain validation and execution; never reparses an argv vector |

Explicit CLI values take precedence over run-spec values. A run spec fills
only missing inputs and never synthesizes command-line tokens. Help, usage,
unknown-option rejection and option conflicts are all derived from the same
`clap` declarations. Runtime and project errors therefore do not print CLI
usage, while parser errors retain `clap`'s command-specific diagnostics.
Compound `SOURCE=PATH`, `SOURCE=VALUE` and `NAME=START..END` values also cross
the clap boundary as typed values. Run-spec input names are parsed once into a
closed `InputRole` enum; the resolver never recovers roles from prefixes or
recreates compound arguments as strings.

Configuration resolution is a distinct phase:

```text
clap argv -> ParsedInvocation
               + discovered/explicit project
               + target and platform pack
               + explicit/project run spec
               + CLI/project/target SVD and memory defaults
             -> ResolvedInvocation -> dispatch -> domain workflow
```

The resolved form owns every loaded configuration object and effective path.
Dispatch does not perform discovery, load configuration files or merge
defaults. Project initialization and configuration are explicit resolved
variants, so they never carry a fake or partially initialized target context.
`--format` changes only stdout. Diagnostics, warnings and verbosity-controlled
tracing stay on stderr, so JSON and JSONL output remains pipe-safe. Errors are
typed at every workbench crate boundary; there is no boxed external-error
escape hatch in the facade. The facade error itself implements `miette::Diagnostic`;
project, run-spec and target parse failures retain their named source and
labelled span through the common renderer.

Without `RUST_LOG`, the default tracing filter shows workbench warnings while
keeping dependency diagnostics below `error`; `-v`, `-vv` and `-vvv` raise
only workbench targets to info, debug and trace. An explicit `RUST_LOG` owns
the complete non-quiet filter. `--quiet` always disables tracing. A command
that fails before producing a result leaves stdout empty, including in JSON
mode; its diagnostic is rendered only on stderr.

Machine output is a schema-1 stream of `{ kind, data }` records. Commands with
stable domain reports emit those reports directly: project status, symbol
inventory, MMIO and interface discovery, linked IR, artifact analysis, and
batch reference generation, direct trace extraction/comparison and concrete
single-symbol execution/comparison and profile verification do not serialize
their human presentation. The verification evidence document is also a Serde
model shared by stdout and file output; the former handwritten JSON encoder
has been removed. The
remaining command renderers still enter the same boundary as explicit
`line`/`text` records; no analysis or verification module writes directly to
stdout. This makes the residual DTO migration visible without allowing raw
text to corrupt JSON or JSONL output.

## Shared trace and effect-contract layout

`crates/analysis-model/src/ir/trace.rs` is the single public `ir::*` façade.
Its child modules separate stable data from queries:

| Module | Responsibility |
| --- | --- |
| `events.rs` | Observable effects, draft reference events, and event formatting |
| `flow.rs` | Draft CFG types and ABI-input collection |
| `validation.rs` | Call-result availability and fail-closed flow validation |
| `function.rs` | `FunctionAnalysis`, eligibility, and inventory queries |
| `tests.rs` | Function-level trace classification tests |

`crates/semantics/src/effect_contract.rs` similarly keeps the existing public
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

## RISC-V executable-image layout

`backend-riscv/src/execution/image.rs` is the stable model and method facade
for a linked diagnostic image. The algorithms operating on that model are
split by the evidence they produce:

| Module | Responsibility |
| --- | --- |
| `image/loader.rs` | ELF segments, symbols, companion images, and fail-closed relocation collection |
| `image/access.rs` | Symbol extents, relocation queries, loaded bytes, memory ranges, and instruction decoding |
| `image/closure_identity.rs` | Address-independent local code-closure identity and symbolic call edges |
| `image/coverage.rs` | Conservative direct-control-flow and conditional-branch inventory |

Loading does not perform control-flow analysis, and byte/instruction access
does not choose either identity or coverage policy. Both analyses consume the
same relocation-aware access facade, so an unresolved reachable relocation is
rejected consistently. The public `ExecutableImage` API remains unchanged.

## RISC-V concrete-execution layout

`backend-riscv/src/execution/machine.rs` owns `Machine` state, scenario
initialization, and the top-level `execute` completion checks. Mutations of
that state are grouped by execution concern:

| Module | Responsibility |
| --- | --- |
| `machine/memory.rs` | MMIO and normal-memory access, ownership checks, observed changes, and persistent-memory projection |
| `machine/events.rs` | Ordered branch, call, modeled-return, and observable-event accounting |
| `machine/step.rs` | Step budget, call interception, RV32 instruction dispatch, and PC/register progression |

The state owner constructs the machine and consumes its final result, while
the dispatcher reaches memory and timeline mutation only through their
methods. Memory policy is therefore reusable from focused execution tests and
does not depend on individual instruction encodings. The public `execute`
facade and scenario/result types are unchanged.

## RISC-V event-codegen layout

`backend-riscv/src/codegen/events.rs` retains the exhaustive event-family
dispatch and renders the ordered event stream. Family-specific lowering lives
under it:

| Module | Responsibility |
| --- | --- |
| `events/mmio.rs` | Static/indexed MMIO reads and writes plus simple MMIO polls |
| `events/polls.rs` | Bounded polls, composed poll flows, and reviewed calibration search |
| `events/memory.rs` | ELF/RAM accesses and proven byte/word transfer loops |
| `events/calls.rs` | External ABI, diagnostics, composed calls, scratch calls, and reviewed wide division |

The facade match remains exhaustive, so a new `ResolvedReferenceEvent` cannot
silently bypass code generation. All family renderers mutate the same
`RenderState` in stream order; token namespaces and external-table validation
therefore still have one ordered owner.

## RISC-V concrete-execution test layout

Shared synthetic image/SVD construction lives in
`backend-riscv/src/execution/tests/mod.rs`; regression cases are grouped by
the policy boundary they exercise:

| Module | Responsibility |
| --- | --- |
| `tests/image_and_control.rs` | Code-closure identity, relocation resolution, branch pruning, calls, and ordered control flow |
| `tests/session.rs` | Warm/cold reset, persistent state, external mutation, ownership, and ordered RAM timelines |
| `tests/calls.rs` | Unresolved calls and scenario-provided modeled return sequences |
| `tests/memory.rs` | Poison/BSS/read-only memory, MMIO responses, atomics, stack fill, fences, and observed-memory projection |

Fixtures with cross-family meaning remain in `tests/mod.rs`; individual test
modules do not construct competing definitions of the executable-image
invariants.

## RISC-V codegen test layout

`backend-riscv/src/codegen/tests/mod.rs` owns the shared trace-to-program
generation helper. Test modules follow the renderer policies they protect:

| Module | Responsibility |
| --- | --- |
| `tests/value.rs` | Symbolic expressions, branch conditions, and static/indexed read-token validation |
| `tests/generation.rs` | Self-contained output scaffolding, incomplete-flow rejection, and ordered RAM operations |
| `tests/memory.rs` | Proven byte/word transfer compaction and value-escape constraints |
| `tests/calls.rs` | Composed-call scoping, result escape, and reviewed wide division |
| `tests/polls.rs` | Bounded-poll rendering and exhaustion diagnostics |

Memory-transfer fixtures remain owned by the memory test module; the one
composed-call escape regression that consumes such a fixture imports it
explicitly instead of duplicating the flow.

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

## Function-workspace source layout

`function_workspace/mod.rs` is the façade for the human-reviewed function and
context layer over linked-IR JSON. Its modules keep generated parsing, editable
claims, validation, and presentation separate:

| Module | Responsibility |
| --- | --- |
| `facts.rs` | Strict minimal projection of schema-v32 linked IR, including site-bearing calls and guard expressions |
| `interface_links.rs` | Exact caller/site join from validated interface bindings to optional linked-IR CFG evidence |
| `pack.rs` | Editable pack and resolved workspace models |
| `pack_parse.rs` | TOML syntax parsing without evidence interpretation |
| `pack_validate.rs` | Provenance, stale-identity, completeness, and coverage rules |
| `template.rs` | One-shot unreviewed pack initialization |
| `review.rs` | Generated human reading view over validated facts and claims |
| `tests.rs` | Pack lifecycle, stale provenance, coverage, and report tests |

The renderer receives an already validated workspace and cannot create review
claims. Pack validation consumes only the stable facts projection rather
than the linked analyzer's internal Rust types, so schema changes cross one
explicit fail-closed boundary.

## IR export source layout

`cli/commands/export_ir.rs` consumes typed options, invokes linked analysis,
and selects outputs. Rendering and domain-input validation are separate:

| Module | Responsibility |
| --- | --- |
| `input.rs` | Artifact syntax, source names, and namespace validation |
| `human.rs` | Tabular terminal report |
| `pseudo.rs` | Pseudo-Rust file output |
| `render_common.rs` | Shared guard/MMIO formatting and traversal |
| `json_report.rs` | Serde document envelope and report summary |
| `tests.rs` | Artifact-domain input validation tests |

Schema v32 serializes the typed `LinkedIrReport` model directly; the removed
schema-v31 handwritten renderer has no compatibility path. Renderers are
consumers of `LinkedIrReport`; they must not independently
recover calls, guards, MMIO fields, or semantic actions. This keeps JSON,
pseudo-Rust, and terminal views consistent.

## Remaining large-file review

Line count is only a signal, but the next useful responsibility reviews are:

- `backend-riscv/src/codegen/mod.rs`: separate the generated Rust runtime
  scaffold from render state/value-address helpers while retaining the public
  `codegen::generate` facade.

These should be split only at ownership and invariant boundaries. Moving a
contiguous block into another file without reducing shared mutable state or
clarifying the dependency direction is not an architectural improvement.
