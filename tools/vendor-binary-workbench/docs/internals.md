# Workbench internals

The binary entry point only translates the library result into an exit code.
The implementation is split by responsibility:

- `crates/contracts` is a zero-dependency, architecture-neutral contract model;
- `crates/analysis-model` owns architecture-neutral symbolic/reference IR,
  indexed-MMIO proofs and the physical register-catalog model populated by SVD
  imports or direct reviewed-model identities;
- `tools/register-model` owns the editable schema-2 hardware model, clean SVD
  encoding, generic model invariants and reusable PAC/evidence pack schemas;
- `crates/semantics` owns architecture-neutral effect-policy, verification
  request/result and evidence-source interfaces;
- `crates/execution-model` owns architecture-neutral device-model contracts,
  standard peripheral behavior, concrete function-table instances and their
  lifecycle evidence;
- `crates/backend-riscv` owns ELF decoding, RISC-V relocations, reference CFG
  analysis, code generation, the RV32 machine and image auditing; it consumes
  execution environments from `execution-model`;
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
- `navigation` builds and strictly validates the optional navigation-only join
  over symbol, linked-IR and interface reports without feeding facts or
  semantics back into those analyzers;
- `verification` owns profiles, dispositions, evidence and comparisons;
- `harnesses::esp32s31` is a thin registry facade over the two ESP32-S31
  harness crates;
- `verification/vendor/targets/esp32s31` owns the checked target/profile/disposition data;
- `cli` parses a typed top-level command and dispatches it to those services.

The backend depends only on neutral contracts, analysis and execution-model
crates. Chip-specific secondary-return recognition and reviewed summaries are
supplied through the typed `RiscvHarnessSpec`; the backend contains no platform
registry and does not own device or callback-table vocabulary. The
facade does not depend directly on the production PHY: that dependency ends at
the ESP32-S31 semantic harness boundary.

Register discovery facts and coverage remain in the workbench facade. The
shared register-model crate knows neither artifacts nor targets. A project may
attach reviewed safe PAC transactions through `[registers.api]` and evidence
catalogs through `[registers.evidence]`; their contents remain target-owned.
The project-owned register commands are the primitive SVD/PAC publication
operations; `project publish` is their strict, preflighted project-level entry
point.

The in-memory catalog adapter consumes the register model's expanded typed
identities directly. SVD is a publication/import boundary, not an internal
transport between two Rust models; array indices and reviewed names therefore
cannot be lost through an XML encode/parse round trip.

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
| `cli/args.rs` | `clap` workflow/subcommand hierarchy, global options and typed `Command`/`ParsedInvocation` values |
| `cli/arguments.rs` | Typed leaf-command arguments, declarative conflicts and value grammar |
| `application/resolve.rs` | Canonical project session: manifest, target/platform, run spec, memory map and register catalog |
| `application/pipeline.rs` | Frontend-neutral stage outcomes, dependency state and aggregate workflow counts |
| `application/project_analysis.rs` | Ordering, dependency policy and typed report for project analysis/review stages |
| `application/project_publication.rs` | Preflight, all-or-nothing preparation policy and typed report for reviewed register publications |
| `cli/resolver.rs` | Positive, exhaustive resource planning plus standalone-target resolution and typed `ResolvedInvocation` construction |
| `cli/resolver/defaults.rs` | Typed CLI > run-spec > project/target argument-default merge |
| `cli/resolver/register_catalog.rs` | SVD plus reviewed register-model composition |
| `cli/resolver/tests.rs` | Resolution precedence, discovery and path-origin contract tests |
| `cli/dispatch.rs` | Exhaustive routing of fully resolved invocations into domain workflows |
| `cli/output.rs` | Single stdout boundary and `human`, `json`, and `jsonl` result rendering |
| `cli/progress.rs` | TTY/machine-output progress policy and reusable operation/stage spans |
| `cli/commands/tooling.rs` | Shell completions and roff manual pages generated from the canonical `clap` grammar without loading a project |
| `cli/render.rs`, `cli/render/*` | Human presentation for typed application and verification reports |
| `cli/commands/{function_pack,interface_pack,registers}/report.rs` | Command-local reviewed-workspace presentation |
| `cli/commands/registers/publication/report.rs` | Typed SVD/PAC/binding leaf-publication results |
| `cli/ui.rs` | miette diagnostics plus tracing/progress layer composition on stderr |
| `cli/mod.rs` | Thin parse → UI initialization → resolve → dispatch composition root |
| `cli/commands/*` | Domain validation and execution; never reparses an argv vector |

Explicit CLI values take precedence over run-spec values. Run-spec selection
itself is explicit `--run-spec` > project manifest `run-spec` > an existing
sibling `local.run`. A run spec fills
only missing inputs and never synthesizes command-line tokens. Help, usage,
unknown-option rejection and option conflicts are all derived from the same
`clap` declarations. Runtime and project errors therefore do not print CLI
usage, while parser errors retain `clap`'s command-specific diagnostics.
The same command definition generates shell completions and the roff manual,
so these assets cannot acquire a second, hand-maintained command hierarchy.
Compound `SOURCE=PATH`, `SOURCE=VALUE` and `NAME=START..END` values also cross
the clap boundary as typed values. Run-spec input names are parsed once into a
closed `InputRole` enum; the resolver never recovers roles from prefixes or
recreates compound arguments as strings.

Configuration resolution is a distinct phase:

```text
clap argv -> ParsedInvocation
               + ProjectSession (project/target/platform/run/memory/catalog)
               + standalone target context when no project is selected
               + precedence-aware typed command defaults
             -> ResolvedInvocation -> dispatch -> domain workflow
```

The resolved form owns every loaded configuration object and effective path.
Dispatch does not perform discovery, load configuration files or merge
defaults. Every `Command` variant owns its exact leaf arguments, and every
`ResolvedInvocation` variant owns the exact project/target/run/catalog context
needed by its workflow. There is no parallel command discriminator/argument
pair and no dispatch-time downcast or impossible branch. Resource loading is
driven by one exhaustive positive `ResolutionNeeds` classification rather than
independent `requires_*`/`uses_*` deny-lists. Project initialization and
configuration are explicit resolved
variants, so they never carry a fake or partially initialized target context.
`--format` changes only stdout. Diagnostics, warnings and verbosity-controlled
tracing stay on stderr, so JSON and JSONL output remains pipe-safe. Errors are
typed at every workbench crate boundary; there is no boxed external-error
escape hatch in the facade. The facade error itself implements `miette::Diagnostic`;
project, run-spec and target parse failures retain their named source and
labelled span through the common renderer.
The project loader validates an immutable `toml_edit::Document`, because
conversion to `DocumentMut` removes parser spans. A shared `ProjectSource`
context reaches the analysis, register, interface and function-section
decoders, so nested type, value and cross-reference failures label the exact
physical manifest value instead of falling back to a location-less string.
Platform packs and memory maps follow the same immutable-document rule through
the reusable `ManifestContext`. Their type checks and semantic validation
(including catalog entries, address spaces, ranges, aliases and overlaps)
therefore retain the responsible TOML value instead of wrapping the failure in
a path-only manifest error. Optional fields reject the wrong type rather than
silently behaving as if the field were absent, and unknown keys are rejected at
the table where they occur.

Concrete MMIO execution keeps physical classification separate from register
catalog enrichment. `MmioRegion` carries the project-owned name, half-open
range and read/write permissions; an execution event carries that region and
an optional register name. Missing SVD metadata is therefore visible without
being confused with incomplete bus coverage. Structural reference generation
retains its stricter requirement for a fully named register bank because those
names participate in generated preconditions.

Reviewed interface metadata follows the same separation of evidence and
behavior. `InterfaceWorkspace` produces stable `ResolvedInterfaceContract`
and slot identities from facts, the reviewed pack and semantic catalogs. A
semantic annotation never becomes executable behavior implicitly. Optional
`execution-contract` and `execution-model` foreign keys must resolve against
the selected compiled platform harness and agree on layout size, slot offset,
ABI arity and semantic operation before the execution model is exposed to
linked-IR/function review. Runtime table contents are scenario-owned
`TableInstance` values and never mutate the reviewed layout pack.

`WorkbenchError` deliberately has no `From<String>` or `From<&str>` escape
hatch. A facade boundary must select `InvalidInput` explicitly, while errors
from domain crates retain their dedicated transparent variants.

Without `RUST_LOG`, the default tracing filter shows workbench warnings while
keeping dependency diagnostics below `error`; `-v`, `-vv` and `-vvv` raise
only workbench targets to info, debug and trace. An explicit `RUST_LOG` owns
the complete non-quiet filter. `--quiet` always disables tracing. A command
that fails before producing a result leaves stdout empty, including in JSON
mode; its diagnostic is rendered only on stderr.

Long-running commands expose progress through the same tracing spans. The
global `--progress auto|always|never` policy defaults to `auto`: progress is
shown only for human output when stderr is a terminal. Machine formats and
redirected stderr therefore stay quiet by default; `always` is an explicit
override, while `--quiet` suppresses progress regardless of this setting.
`tracing-indicatif` supplies the stderr writer used by the tracing formatter,
so `-v` diagnostics cannot overwrite an active progress bar. Project analysis
and publication use nested workflow/stage spans; direct long-running commands
use one root operation span.

Machine output is the command's typed report itself; there is no generic
record envelope. The output boundary owns a single report slot: a second
report is an invariant violation, and human `line`/`text` presentation cannot
be converted into machine output. Commands with
stable domain reports emit those reports directly: project status, symbol
inventory, MMIO and interface discovery, linked IR, artifact analysis, and
project diagnostics/analysis/publication, project IR builds, batch reference
generation, reviewed register/function/interface workspace lifecycles, direct
trace extraction/comparison, concrete single-symbol execution/comparison,
source/inventory verification and profile verification do not serialize their
human presentation. The verification evidence document is a Serde model shared
by the aggregate command report and file output. Source and inventory
verification return typed per-function verdicts from the engine; their removed
line protocol has no compatibility path. The former handwritten JSON encoder
has been removed. No analysis, verification or platform-harness module writes
directly to stdout. Semantic qualifications return a typed report containing
artifacts, scenario verdicts, coverage totals, state-footprint counts and the
first retained difference; only the CLI renders that report.

Line-oriented verification profiles, dispositions and evidence baselines
preserve their physical source line as a typed diagnostic span. TOML project,
memory, platform, function and interface manifests retain parser-provided byte
spans. Malformed JSON facts and verification reports retain the parser's
physical source line instead of degrading to a path-only message. Function and
interface semantic validation use source-neutral field locators to retain the
exact reviewed pack field after syntax parsing, including stale provenance,
ABI/layout and semantic-link failures. The generic register-model crate exposes
manifest diagnostic metadata as `kind/path/reason/span` without depending on a
terminal diagnostic library; the workbench facade promotes it to the same
source diagnostic at the CLI boundary.

Commands that publish JSON, pseudo-Rust or navigation files include the path
and `written`/`verified` state in their primary typed result. Publication is
not a second machine report; the removed `output::file` path has no
compatibility mode. Nested project navigation reports publication through its
own project-analysis stage and tracing span.

`project doctor` is split into generic capability, register, interface and
caller-input collectors. They populate one top-level report together with the
IR-profile and function-workspace reports. Human rendering is separate from
collection, while JSON and JSONL serialize the same `project-doctor`
model directly. Human status/capability, symbol inventory, profile verification
and register/function/interface review summaries use tables only as a
presentation layer; structured output serializes the underlying typed reports.

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

Verification aggregation is split by responsibility: `engine.rs` computes
per-function source verdicts, while `engine/model.rs` owns source inputs,
aggregate gates, protocol inventory and probe accounting. `evidence.rs` owns proof digests;
`evidence/baseline.rs` owns baseline comparison and candidate publication;
`evidence/report.rs` owns the persistent core; `execution.rs` owns comparison
and `execution/scenario.rs` owns scenario normalization and coverage inputs.
`execution_report.rs` owns concrete comparison DTOs, while `report.rs` owns
the single schema-v4 command/file report plus its human renderer.
`dispositions.rs` owns its strict parser and inventory validation;
`dispositions/model.rs` owns entry, binding and effect-policy invariants.

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
does not depend on individual instruction encodings. Device behavior and
runtime table-instance types live in `crates/execution-model`; the RV32 machine
only maps those architecture-neutral effects to concrete loads, stores and
indirect calls.

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

## Persistent artifact boundary

`artifacts/` owns the identity and read boundary of reusable generated project
artifacts. Producer commands and downstream consumers use the same
`ArtifactSchema` constants. Command-result envelopes remain separate because
they describe one invocation rather than durable project evidence.

| Module | Responsibility |
| --- | --- |
| `artifacts/mod.rs` | Canonical schema version/command identities and source-aware JSON loading |
| `artifacts/linked_ir.rs` | Typed linked-IR summary projection and claim validation |
| `artifacts/symbol_inventory.rs` | Typed symbol-inventory summary projection |

Full MMIO and interface-facts models still live with their domain validation
in `registers/facts.rs` and `interfaces/facts/`; their producers nevertheless
share the canonical identities from `artifacts/`. Linked-IR workspace readers
may use narrower typed projections, but they may not invent an independent
schema version.

## Function-workspace source layout

`function_workspace/mod.rs` is the façade for the human-reviewed function and
context layer over linked-IR JSON. Its modules keep generated parsing, editable
claims, validation, and presentation separate:

| Module | Responsibility |
| --- | --- |
| `facts.rs` | Stable generated-fact model, multi-report loading and queries |
| `facts/parse.rs` | Strict schema-v35 linked-IR projection, including site-bearing calls and guard expressions |
| `facts/json.rs` | Low-level JSON shape, integer, address and digest readers |
| `facts/validate.rs` | Cross-report identities, source ownership and field invariants |
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

## Project and interface-facts source layout

`project.rs` is the stable project model and discovery façade. TOML decoding,
workspace path resolution and semantic manifest checks live in
`project/load.rs`; focused manifest fixtures live in `project/tests.rs`.
Callers therefore depend on `ProjectSpec`, not on parser helpers or
`toml_edit` details.

Generated interface facts follow the same direction:

| Module | Responsibility |
| --- | --- |
| `interfaces/facts.rs` | Stable facts model, loading boundary and queries |
| `interfaces/facts/parse.rs` | Strict schema-3 JSON projection with affine indexed-slot evidence |
| `interfaces/facts/validate.rs` | Cross-record identities, slot/call consistency and digest rules |

Parsing constructs the model and then invokes validation once. Pack code can
query validated facts but cannot bypass that loading boundary.

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

Schema v35 serializes the typed `LinkedIrReport` model directly; the removed
schema-v31 handwritten renderer has no compatibility path. Renderers are
consumers of `LinkedIrReport`; they must not independently
recover calls, guards, MMIO fields, or semantic actions. This keeps JSON,
pseudo-Rust, and terminal views consistent.

## Source boundary guards

`tests/architecture_boundaries.rs` rejects CLI/output dependencies from
application and domain modules and rejects target-specific PHY defaults in the
generic grammar. Large files are split only when the move establishes an
ownership or invariant boundary; line count alone is not a module boundary.

## Application and frontend boundary

`WorkbenchApplication` is the stateful, CLI-independent project facade. It
uses the same canonical `ProjectSession` resolution as project-oriented CLI
commands for the manifest, target, platform composition, run spec, memory map and
register catalogs once, owns generation-scoped analysis caches, and exposes
typed workspace, analysis and execution-comparison reports. The CLI and the
read-only TUI are consumers of this application state; neither may parse the
other frontend's rendered output.

The application snapshot deliberately tolerates missing generated facts and
reports them as component diagnostics, so a partially built reverse-engineering
workspace remains browsable. Invalid source configuration fails resolution.
Reload is atomic: the old state remains usable if the new project cannot be
resolved.

See [Application API and alternate frontends](application-api.md) for the
public API and frontend contract. Every project-init/configuration operation
remains fully scriptable through typed arguments and checked-in manifests. A
future interactive editor must remain an explicit frontend over the same
validators; it must not silently rewrite reviewed project data.
