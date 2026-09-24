# Blobray completion pipeline

This is the sole user-authorized tracked work plan. Architecture, implemented
contracts and command syntax remain in [design](docs/design/architecture.md) and
the [operator reference](next/README.md). This plan completes the legacy feature
scope in Next; it does not preserve legacy command grammar or implementation.

## Executor contract

- Execute authorized stages autonomously in order. No confirmation is needed
  between stages. Do not introduce an agent runner or workflow framework.
- Finish the entire active stage: contracts, implementation, application API,
  frontend, regression tests, documentation and acceptance checks.
- A compiled API, passing isolated test or partial feature is not completion.
  Fix failed acceptance checks within the active stage before advancing.
- Technical interruption keeps the stage active. The next executor resumes it;
  do not restart completed discovery or call an interrupted stage complete.
- If scope or prerequisites prove infeasible, record `needs-replan`: evidence,
  invalid assumption, unmet criteria, revised dependencies/split. Preserve every
  obligation. Ask the user only when their decision or external input is needed;
  routine implementation and fixing tests do not require permission.
- Among work with satisfied prerequisites, finish existing scenarios before
  introducing additional abstractions. No new crate without an authority boundary.
- Review of scientific claims remains explicit. Automation of development does
  not authorize accepting unreviewed hardware assertions.
- Commit completed stages with their updated status. Private inputs, binary
  exports, measurements and logs stay in owner-specific ignored outputs.
- Breaking formats are permitted until stage 20; unsupported versions must be
  rejected without mutation. Do not add converters, compatibility readers or
  old-reader bundles. Stage 20 establishes the first stable persistence contract.

## Definition of done

Every product stage must have a complete application scenario and matching
frontend; explicit inputs/results, ownership/lifetime/errors and claim scope;
positive and meaningful negative regression tests; shared resource admission and
atomic publication; source-free reopening of retained results; current docs;
passing required checks; and a completed commit. Skipped checks are not passes.

Stages use `pending`, `ready`, `active`, `needs-replan`, `done`. A capability has
one owning stage and closes as implemented, replaced by a native interface, or
moved to a named owner. `target` never closes a capability. No silent exclusions.

## Stages and acceptance

| Stage | Status | Delivered scenario and required acceptance |
| --- | --- | --- |
| 00 | done | Establish this plan, its repository exception, all feature assignments and first-stage prerequisites. Every legacy leaf and internal mechanism has an owner; required tools and real inputs are identified. |
| 01 | done | Close existing capture/analysis/research/data/review/export/coverage/storage/execution/preservation promises. All detailed criteria below pass, including a real linked graph and source-free restore. |
| 02 | done | Exact code/data addressing: reviewed symbol-less executable ranges; physical static/dynamic symbol selection; zero-sized/alias occurrences; initialization bytes and range-local relocation effects. Accepted selectors survive analysis/export/reopen; only affecting relocations block integer interpretation; coverage does not infer code in unselected bytes. |
| 03 | done | Pointer tables and interfaces: exact relocated targets, bounded alternatives, roots/slots, layout/ABI/guards/index domains and semantic bindings. Discovery → proposal → review → query/export works on synthetic and real inputs; ambiguity/unsupported/null/external targets stay distinct; invalid guards and conflicting layouts fail. |
| 04 | active | Function/context contracts and research navigation: signatures, argument roles, fields, preconditions, reviewed paths/event routes; function/callers/callees, object readers/writers, field accesses, flow/effect slices. Answers retain evidence paths and distinguish structural from executable paths; cycles, ambiguous callbacks and partial results are tested; reads never schedule hidden analysis. |
| 05 | pending | Register lifecycle: MMIO/field discovery, physical catalog, evidence/conflicts/coverage, applicability/review. Independent register tool owns model initialization/SVD import and existing publication. Observation → reviewed source model → validate → four generated outputs works; observed access width is not physical width; generic Blobray gains no production/chip dependency. |
| 06 | pending | Saved linked semantic IR, configured builds/exports and static observable trace comparison. May-effects, exact static traces and concrete observations remain distinct; incomplete traces cannot MATCH; known differences DIFF; provenance and source-free reading survive composition. |
| 07 | pending | Integer execution sessions: stack ABI arguments, RV32 atomics, multiple entry points/setup phases, ownership/persistent regions, cold/warm resets and return/reach-symbol/observe-call goals. State transitions, unknown data, unsupported instructions, dependent-phase blocking and exact replay are tested. |
| 08 | pending | External-call returns/outputs/bounded allocation, delay events and standard constant/sequence/W1C/read-clear/self-clearing/FIFO/indexed-bank models. Every mechanism has positive/negative cases; no implicit response/fallback; model identity/applicability/participation is evidence; code and modeled boundaries stay distinct. |
| 09 | pending | Stateful FIFO services and interface tables: enqueue/dequeue/length, wake/output, table lifecycle, service-event completion. Full/empty/order/isolation and cross-phase state are tested; only selected reviewed bindings resolve calls; failed phases cannot imply workflow completion. |
| 10 | pending | Comparison relations: selected final RAM, ordered calls/reviewed argument pairs, RAM/branch timeline, ABI/layout projections and effect contracts. Explicit relation selects returns/memory/events; RAM-only and call-only differences are caught; invalid projections fail; all three verdicts and retained excluded observations are tested. |
| 11 | pending | Real PHY I2C workflow from authenticated capture and tables through linked execution/models and compiled-production comparison. Independent expectations, model boundaries, source removal and backup/restore are required; no model hides absent production/vendor code or grants hardware qualification. |
| 12 | pending | Real PHY calibration/RF workflow, required intrinsics and reviewed summaries. Each declared calibration case has explicit conditions and expected outcome; coefficients/models/production changes alter dependency identity; reconstruction never impersonates captured execution. |
| 13 | pending | Project verification: native roles/scopes/models/suites/policies, profile domains/coverage, production bindings/dispositions/audit, baselines/status/check/files/doctor and ranked next actions. One application lifecycle executes suites; auxiliary inputs do not inflate coverage; stale/unreviewed evidence cannot satisfy gates; no CLI-only orchestration. |
| 14 | pending | Remaining declared Wi-Fi/Bluetooth/coexistence/radio-leaf suites. Every original suite has a native scenario and checked expected MATCH/DIFF/INCOMPLETE; preserve exclusions/claim strength; compiled production paths are required. Split independent large groups into separately accepted sub-stages before activation, without dropping suite obligations. |
| 15 | pending | Vendor revision snapshots/prepare-update/diff, symbol correspondence/lineage and reviewed rebase of assertions/boundaries. Rename is evidence, not acceptance; changed bodies/layouts/applicability invalidate dependencies; ambiguous mappings require decisions; A→B→reviewed rebase retains A. |
| 16 | pending | Incremental reuse and bounded parallelism with complete dependency identities. Identical work reuses results; independent edits invalidate only dependents; missing-dependency regressions, serial/parallel equivalence, cancellation/capacity atomicity and measured work reuse are required. |
| 17 | pending | Permanent storage: transitive retention roots/pins, reachability/reclaimable report, GC preview/apply and compaction. Review evidence/active readers survive; roots are revalidated at apply; interruption preserves committed projects; reclaimed bytes are verified separately from temporary quotas. |
| 18 | pending | Pseudo-Rust and separate executable-reference consumer, single/batch generated/blocked manifest with provenance. Pseudocode is not executable evidence; generated supported code compiles and is tested; unsupported semantics block generation; no production substitution/qualification. |
| 19 | pending | TUI over shared application/read APIs; completions/manpage and consistent diagnostics/progress/details. No frontend analysis/workflow duplication; partial/stale/conflicted states, empty/large streams, cancellation and errors are tested; help/examples/generated docs agree. |
| 20 | pending | Full replacement and stable persistence: all assignments closed, no legacy runtime calls, old engine/launcher/dependencies removed after checking consumers, reviewed machine inputs preserved. Generic standalone/register publication and real workflows/recovery pass. Freeze a research fixture corpus and transitive persistence contract that subsequent versions must read without conversion. |

### Stage 01: existing checkpoint

1. Share physical occurrence acquisition/validation within application. Generic
   proposal, specialized proposal, review and export validate the same object and
   optional symbol. Integer tables use data-symbol validation, not function
   validation; range and evidence checks borrow the same prepared object.
   Regressions must cover invalid index/table/object via `KnowledgeAction::Propose`
   with correct image/range/source evidence and no analysis references. Valid data
   symbols accept/export. Failed validation creates no knowledge revision/head.
   Cover input/image, generic/specialized and review/export paths.
2. Roll back payload reservation when the final `RecordBuffer::push` step fails.
   Test failure after payload admission, unchanged records, released orphan
   payload, subsequent successful use and complete release on drop. Vector
   capacity retained for reuse is distinct from record-owned payload.
3. Extend the real PHY scenario with a prepared linked image and
   `phy_i2c_master_cmd_mem_init` → `phy_encode_i2c_master`/`phy_i2c_master_fill`/`phy_get_data_sat`.
   Check resolved physical targets, call arguments and composed-fact provenance,
   not exit code. Tail transfers retain their separately documented limits.
   Add a diamond graph whose child remains available for both parents and is
   released after the last consumer. Cover repeated edges, cycles and unknown
   targets without promoting incomplete analysis. Three repeats must preserve
   semantic results, record load/compose phase measurements and obey the budget;
   do not invent a speedup threshold.
4. Check data against independently extracted authenticated archive bytes:
   PHY archive SHA-256 `d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580`;
   `phy_i2c.o` SHA-256 `7e6ebb1353d1bd2c53b4b5b1176bbf795c57d899c3f07a26ce56e74b5803e9d9`;
   `.rodata.CSWTCH.51` 200 bytes, SHA-256
   `927b3305a35468bb52f3de4e3305f4b4d0674831014376a094ceb00022bab183`.
5. Run affected existing capture, research, data/review/export, coverage/storage,
   execution/replay and persistence regressions. Real linked research and
   review/export must survive source removal, move and backup/restore. Document
   that selected data exports are not standalone transitive research backups.

## Complete legacy assignment

Each row names actual CLI leaves; grouped rows enumerate every member explicitly.
The 67 leaves are sourced from `src/cli/args.rs::Command` at baseline `357b4d7e`.
Aliases/old grammar are not portability requirements.

| Legacy leaf or leaves | Completion owner |
| --- | --- |
| project init; project inputs init | 01: native init/import |
| project configure | 13: native project composition |
| project doctor; project files; project status | 13 |
| project cache stats; project cache gc; project cache compact | 17 (existing logical usage checkpoint: 01) |
| project revision snapshot; project revision prepare-update; project revision diff; project revision rebase | 15 |
| project research next; project audit bindings | 13 |
| project browse | 19 |
| project analyze | 13 (analysis components: 01–06; reuse: 16) |
| project verify; project check | 13 |
| project publish | 05: independent register publication |
| advanced functions init-pack; advanced functions validate; advanced functions review | 04 |
| advanced code init-pack; advanced code validate; advanced code review | 02 |
| advanced code rebase | 15 |
| advanced symbols inventory | 02 |
| advanced symbols correlate; advanced symbols lineage | 15 |
| advanced interfaces discover; advanced interfaces init-pack; advanced interfaces validate | 03 |
| registers list; registers coverage; registers evidence | 05 |
| registers init-model; registers import-svd; registers validate; registers review | 05 |
| registers export-svd; registers generate-pac-raw; registers generate-pac-api; registers generate-bindings | 05: existing separate owner |
| inspect function; inspect flow; inspect object; inspect register | 04 (register catalog: 05) |
| inspect scope | 13 |
| inspect analyze | 06 (local profile checkpoint: 01) |
| inspect trace; inspect compare | 06 |
| advanced mmio discover | 05 |
| advanced ir export; advanced ir build | 06 |
| advanced reference generate; advanced reference generate-batch | 18 |
| advanced execute run; advanced execute replay | 09 (sessions: 07; models: 08) |
| advanced execute compare | 10 |
| advanced verify profiles; advanced verify source; advanced verify inventory; advanced verify evidence | 13 |
| advanced image audit-targets | 01: native audit |
| tooling completions; tooling manpage | 19 |

| Mechanisms not captured by command names | Completion owner |
| --- | --- |
| Exact static/dynamic symbols, symbol-less code, aliases, explicit data extents | 02 |
| Pointer relocation/initializers, finite value alternatives, interface guards/slots | 03 |
| Contexts, preconditions, event routes, data-flow/ownership navigation | 04 |
| Hardware assertions/applicability, physical catalog and publication policy | 05 |
| Linked semantic IR and statically extracted effects | 06 |
| Stack arguments, LR/SC/AMO, multi-entry phases, RAM ownership/reset/goals | 07 |
| External calls/outputs/allocation/delays and standard peripheral mechanisms | 08 |
| FIFO services, interface instances/lifecycle and service goals | 09 |
| RAM/call/branch relations, reviewed projections and effect contracts | 10 |
| PHY I2C models/scenarios | 11 |
| Calibration/RF/intrinsic summaries and scenarios | 12 |
| Profile coverage, claim policy, production bindings and evidence baselines | 13 |
| Declared remaining radio suites/mechanisms | 14 |
| Cross-revision correspondence and assertion applicability | 15 |
| Dependency-aware incremental cache and bounded jobs | 16 |
| Retention, reclaimability, pins and compaction | 17 |
| Pseudo-Rust and executable reference generation | 18 |
| Human/JSON diagnostics, details/progress/color and TUI | 19 |

## Gates and checkpoints

- Each stage: focused regressions; `cargo fmt --all -- --check`; Clippy for changed
  packages/all targets with warnings denied; owned Markdown and public/private
  API docs checks as applicable. Broaden after new changes or unresolved failures.
- Boundary changes: Next architecture test and
  `cargo xtask check blobray-standalone`.
- Stage 01: domain record-memory tests; Next functions/images tests; affected
  knowledge/data/research regressions; real PHY script; relevant existing gates.
- Full checkpoints: stages 01, 06, 10, 12, 14, 17 and 20, including relevant
  integration suites and `cargo xtask check source-only`.
- Real workflows use built-in supervision with explicit limiter mode. Kernel
  cgroup integration requires delegated controllers; unavailable is not passed.
  Watchdog evidence never claims kernel enforcement. Hardware qualification is
  independent and is not a completion claim of this pipeline.
- Checkpoint meanings: 01 current promises; 06 static research; 10 execution and
  comparison; 12 practical PHY; 14 declared radio campaigns; 17 long-lived
  research; 20 complete replacement and stable persistence.

## Current execution position

Stages 00–03 are complete. Stages 04.1–04.2 are complete; stage 04.3.1 is complete and stage 04.3.2 is ready with the partitions below.
Stage 03 acceptance includes captured pointers, finite callback alternatives, native
interface review/discovery, ambiguous bindings and authenticated ROM query/export
reopening. All standalone Next packages/tests, Clippy, formatting and affected
public/private documentation gates pass. Real PHY capture, linked research,
table/constant/pointer/interface review and byte-identical restored exports pass
under explicit watchdog limits; this does not claim kernel memory enforcement.
Stage 01 acceptance is covered by `record_memory` regressions, application
`research::lifetime_tests`, Next `functions`/`images` tests, and the real
`phy_research.py` linked/review/export/reopen scenario. All nine Next packages,
Clippy, application/domain public/private docs, standalone extraction and
source-only checks passed. Kernel integration remains explicitly environment-gated.
The workspace provides LLVM LLD 22.1.8 and retained authenticated PHY/ROM captures
under ignored `target/blobray-research/real/`; source bytes are not committed.

### Stage 02 refinement

The original stage combines independent data-range work with a cross-cutting
change to function identity in API, saved plans, knowledge and persistence.
`FunctionRequest`/`FunctionRecipe` currently require `SymbolId`; inventing a fake
symbol for a range is forbidden. Split these scopes before implementation,
retaining the parent acceptance criteria and native source-free workflows:

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 02.1 | done | Range-local relocation effects: unaffected integer bytes decode; intersecting/unknown transformations remain explicitly unresolved; preserve all source relocations and initialization classification; boundary/width/unknown cases and review/export/reopen pass. |
| 02.2 | done | Exact dynamic-symbol occurrences: selected physical tables/indices are validated, data/functions use the correct table, duplicates and aliases do not merge, malformed/unsupported ELF is explicit; ordinary static-symbol scenarios remain correct. |
| 02.3 | done | Native symbol-less executable identities and reviewed boundaries across request/recipe/knowledge/planning/query/coverage. No synthetic SymbolId, no inference from neighboring symbols, no dual legacy resolver. Analyze/export/reopen retains exact ranges and explicit claim scope. |

### Stage 03 refinement

`needs-replan` resolved before activation: the original stage combines data-table
interpretation, a change to the abstract-value domain and reviewed interface
contracts. Next currently has exact byte/range exports and single abstract
values; legacy interface roots/slots/guards/index domains are separate obligations,
not a ready native interface to copy. Complete the following independently accepted
sub-stages; none drops the parent scenario or transfers work into a compatibility
reader. Runtime device/service execution remains owned by stages 08/09.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 03.1 | done | Captured pointer tables: explicit layout and RV32 pointer profile, physical relocation target/addend or image address, null/external/ambiguous/unsupported distinctions. Discovery/data query → proposal → acceptance → export/reopen on synthetic and authenticated real bytes. Preserve initialization and relocation evidence; invalid/overlapping relocation extents, overflow, budget failure and unsupported encodings cannot yield a complete resolved table. |
| 03.2 | done | Bounded alternatives for addresses/call targets: stable finite join, operation-scoped admitted storage, cycles/convergence and explicit overflow-to-unknown/incomplete evidence. Read-only analyses and exported facts preserve all retained alternatives and provenance; no arbitrary target chosen. Real and synthetic callbacks plus budget/cancellation checks pass. |
| 03.3 | done | Native interface contracts and discovery: physical table/argument/address roots, access paths and slots, layout/ABI, guards, finite index domains and semantic bindings. Observation → proposal → review → query/export/reopen is one application workflow. Missing/rejected bindings, invalid guards/index domains, overlapping/conflicting layouts and unsupported ABI remain explicit; no model execution or hardware qualification is implied. |

Stage 03.3 refinement: `needs-replan` resolved before implementation. The parent
acceptance combined a new reviewed declaration format with discovery over saved
expression/call records. Neither native declaration nor discovery interface exists;
shipping one does not close the other. Keep the existing knowledge lifecycle and
separate these independently testable scenarios, preserving every parent criterion:

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 03.3.1 | done | Native interface declaration → physical/pure validation → proposal/review → knowledge query/export/source-free reopening. Table/argument/address roots, explicit paths, layout/slot ABI, guards, finite index domains and semantic bindings have bounded typed contracts. Bad identities, invalid guards/domains, unsupported ABI and conflicting accepted layouts fail without publication. No declaration claims runtime guard satisfaction or executes a model. |
| 03.3.2 | done | Discover table/callback observations from captured data and saved function facts; match only explicitly selected accepted contracts. Export exact slot/path/evidence, bindings and guard applicability; missing/rejected/ambiguous/unsupported cases remain explicit. Synthetic and authenticated real observation→review→query/export/reopen scenarios pass without hidden analysis, legacy resolution or runtime-model execution. |

Stage 03.2 implementation contract: domain exposes bounded nonrecursive exact
alternatives; the existing analysis phase owns interned sets and its admitted
index. Joins and candidate arithmetic have a fixed cardinality bound of eight,
with explicit overflow gaps. Public query/export retains all alternatives and
source provenance; multiple call targets remain ambiguous rather than selecting
a callee. Existing application/research ownership is retained. Symbolic expressions
remain a flat DAG; this stage does not add path correlation or hidden analysis.

Stage 03.1 implementation contract: keep the existing captured-data/review/export
workflow. Domain owns a tagged integer/pointer table layout and pointer observation
values. Artifacts supplies borrowed bytes and structural relocation bounds;
analysis streams slot interpretation using an injected relocation-semantics port;
the RV32 backend owns the absolute-pointer relocation profile. Application owns
selection, budgets and delivery, knowledge validates layout/evidence and store
retains accepted assertions. Raw pointer queries are explicit observations;
acceptance does not apply relocations, link external symbols or assert hardware
behavior. A numeric address, a physical defined-symbol reference, an external
symbol, null, an ambiguous transformation and an unsupported encoding remain
different machine-readable results. No per-slot full-section scan or new crate.

### Stage 14 partitions fixed before activation

Each partition inherits the stage 14 criteria and completes independently. Suite
IDs below are the existing `verification-addon.toml` declarations, not inferred
new qualification scope. PHY suites belong to stages 11/12.

| Sub-stage | Status | Exact suites |
| --- | --- | --- |
| 14.1 | pending | `libpp-interrupt`, `libpp-power-interrupt`, `wifi-ap-tsf-stop`, `wifi-ap-tsf-start`, `rom-sta-tsf-snapshot`, `libpp-tx-dma`, `libpp-rx-dma`, `libpp-tx-retry`, `wifi-interface-context`, `wifi-sta-ap-receive`, `wifi-sta-beacon-filter`, `ordinary-tx-ownership`, `tx-protection-control` |
| 14.2 | pending | `ble-interrupt-prefix`, `ble-scheduler-table-prefix`, `btbb-v2-init-arg-one`, `ble-memory-list-selector-one`, `ble-phy-register-init`, `ble-memory-list-selector-two`, `ble-memory-list-selector-three` |
| 14.3 | pending | `ieee802154-btbb`, `ieee802154-zb`, `ieee802154-coex`, `coex-timer-control`, `coex-timer-set`, `coex-core` |

Stage 01 fixture correction: the originally planned `phy_bias_reg_set` callee
is a four-byte ROM tail trampoline. Current call composition does not expand
that transfer, so it cannot meet the composed-effect acceptance criterion. The
replacement root above has ordinary calls to two concrete, small ROM bodies;
no acceptance obligation was removed.


### Stage 04 refinement

`needs-replan` resolved before activation: the legacy function workspace combines
signature/context declarations with graph queries and three event-route mechanisms.
Native saved call/value/effect records exist, but a reviewed function/context schema
and shared cross-analysis navigation do not. Keeping this as one stage would couple
independent review and graph acceptance. Preserve every obligation in these complete
scenarios; do not import legacy packs or add a second resolver:

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 04.1 | done | Physical function signature/context declaration → validate/propose/review → query/export/source-free reopening. Share call ABI vocabulary with interfaces. Argument/return roles, context extents/fields/access roles and explicit preconditions remain conditional claims. Bad selectors, field bounds/types, contradictory preconditions and conflicting accepted contracts fail without publication; symbol/range, ordinary/thin/image and resource regressions pass. |
| 04.2 | done | Shared saved-research navigation for functions/callers/callees, data-object readers/writers and context-field accesses. Explicit selected analyses/publications/knowledge, bounded operation indexes, physical selectors and evidence references; ambiguous/unresolved targets remain visible, cycles terminate, no hidden analysis. CLI/API/export/reopen agree on synthetic and real queries. |
| 04.3 | active | Flow/effect slices and reviewed paths/event routes: selector delivery, static callback registration/delivery and broker subscription with exact participants, sites, fields/selectors and evidence. Structural paths, conditional reviewed routes and executable evidence remain distinct. Missing/ambiguous/mismatched steps, cycles and bounds cannot claim completion; complete query/review/export/reopen scenarios close the remaining stage 04 obligations. Runtime replay remains a later execution consumer, never synthesized by navigation. |


Stage 04.1 acceptance is covered by knowledge `functions::tests`, the Next
function-contract symbol/range and linked-image review/export regressions, existing
interface tests through the shared signature validator, and all standalone packages.
Affected package tests, Clippy, formatting and public/private docs pass. Review
preserves analysis identity; resource/conflict failures preserve the selected base.
Saved fact loading in research and interface discovery shares the same scoped
JSON decoding envelope and admitted record owner.


Stage 04.2 acceptance is covered by shared navigation CLI/API regressions for
linked calls, NOBITS ranges, known/unknown context signatures and incoming stack
words; application cycle/ambiguity/foreign-occurrence tests; domain ABI placement;
store dependency-read reuse/corruption/lifetime checks. Affected tests, Clippy,
formatting, public/private docs and standalone extraction pass. The authenticated
PHY scenario checks 45 encode, 44 fill and three saturation saved calls and
byte-identical restored navigation exports with explicit watchdog limits.


### Stage 04.3 executable partitions

Inspection of the legacy flow owner identifies three independently usable algorithms:
inter-function target/effect traversal, intra-function reaching RAM definitions at a
publication anchor, and reviewed asynchronous route validation. The previous single
row coupled these algorithms without naming the RAM last-write obligation. Refine it
before implementation; none is dropped or moved into a legacy adapter.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 04.3.1 | done | Exact saved root → selected function target or reachable effect inventory, with bounded call graph, evidence hops, ambiguous/missing/partial frontiers and explicit structural scope. Native ordered path proposal/review revalidates every exact saved step, rejects missing/mismatched/ambiguous steps and preserves roots. CLI/API/query/export/source-free reopening, cycles/diamond/depth/resource cases and real linked target/effects pass. |
| 04.3.2 | ready | RAM definitions reaching an exact saved call/store publication anchor: incoming, must/alternative/candidate last writes; partial overlap, unknown alias and call clobbers remain explicit. Iterative bounded CFG/dataflow, local witnesses and values; joins, loops, killed definitions, unknown calls, resource failures and source-free query/export pass. It does not invent interprocedural memory effects. |
| 04.3.3 | pending | Native reviewed selector delivery, static callback registration/delivery and broker subscription routes with exact participants, sites, object/queue/domain/selector fields, callback identity, case handler and optional terminal. Query authenticates physical evidence and reports condition/lifetime/order blockers separately; review never proves actual asynchronous delivery. Positive/negative/ambiguous/source-free review/query/export cases for all three close stage 04; runtime replay remains stages 07–10. |


Stage 04.3.1 acceptance covers deterministic bounded graph witnesses, native
path review, ambiguous selected interpretations, exact record/occurrence failures,
source-free exports and the authenticated linked PHY effect inventory. The real
workflow preserves all 44 composed fill records and the expected focused first
write after move/backup/restore. Affected package tests, the full Next suite,
Clippy, formatting, public/private docs and standalone tests pass. The acceptance
run also closes runtime cleanup/reconciliation serialization with a focused lock
and capacity-release regression; failed initialization releases the root lock
before cleanup. No runtime path feasibility or hardware qualification is claimed.
