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
| 04 | done | Function/context contracts and research navigation: signatures, argument roles, fields, preconditions, reviewed paths/event routes; function/callers/callees, object readers/writers, field accesses, flow/effect slices. Answers retain evidence paths and distinguish structural from executable paths; cycles, ambiguous callbacks and partial results are tested; reads never schedule hidden analysis. |
| 05 | done | Register lifecycle: MMIO/field discovery, physical catalog, evidence/conflicts/coverage, applicability/review. Independent register tool owns model initialization/SVD import and existing publication. Observation → reviewed source model → validate → four generated outputs works; observed access width is not physical width; generic Blobray gains no production/chip dependency. |
| 06 | done | Saved linked semantic IR, configured builds/exports and static observable trace comparison. May-effects, exact static traces and concrete observations remain distinct; incomplete traces cannot MATCH; known differences DIFF; provenance and source-free reading survive composition. |
| 07 | done | Integer execution sessions: stack ABI arguments, RV32 atomics, multiple entry points/setup phases, ownership/persistent regions, cold/warm resets and return/reach-symbol/observe-call goals. State transitions, unknown data, unsupported instructions, dependent-phase blocking and exact replay are tested. |
| 08 | done | External-call returns/outputs/bounded allocation, delay events and standard constant/sequence/W1C/read-clear/self-clearing/FIFO/indexed-bank models. Every mechanism has positive/negative cases; no implicit response/fallback; model identity/applicability/participation is evidence; code and modeled boundaries stay distinct. |
| 09 | done | Stateful FIFO services and interface tables: enqueue/dequeue/length, wake/output, table lifecycle, service-event completion. Full/empty/order/isolation and cross-phase state are tested; only selected reviewed bindings resolve calls; failed phases cannot imply workflow completion. |
| 10 | done | Comparison relations: selected final RAM, ordered calls/reviewed argument pairs, RAM/branch timeline, ABI/layout projections and effect contracts. Explicit relation selects returns/memory/events; RAM-only and call-only differences are caught; invalid projections fail; all three verdicts and retained excluded observations are tested. |
| 10.L linker capabilities | done | After completed 10.4, deliver ElfAnalysisLinkV1 through LLD and GNU ld adapters: capability probes, semantic requests, normalized placement/extraction/exit evidence with raw provenance, exact root validation, quota-admitted seekable GNU output, retained closure/query/export/restore, and distinct tool identities/archive semantics. Both real RV32 toolchains, parser/application/resource regressions, affected suites, strict Clippy, formatting, public/private docs, standalone and source-only checks are mandatory. Commit only after full acceptance; this checkpoint does not include stage 11. |
| 11 | done | Real PHY I2C workflow from authenticated capture and tables through linked execution/models and compiled-production comparison. Independent expectations, model boundaries, source removal and backup/restore are required; no model hides absent production/vendor code or grants hardware qualification. |
| 12 | active | Real PHY calibration/RF workflow, required intrinsics and reviewed summaries. Each declared calibration case has explicit conditions and expected outcome; coefficients/models/production changes alter dependency identity; reconstruction never impersonates captured execution. |
| 13 | pending | Project verification over typed Rust scenario suites. Suites are Rust packages on Blobray types (as in stage 12); no new declarative suite format. Blobray composes their native executions and reviewed claims into the evidence index that qualification consumes, with roles/scopes, production bindings/dispositions, baselines/status/check/doctor and ranked next actions. Qualification (`qualification/evaluator`, targets/catalogs), `hil` runner tests and `tools/repo` vendor checks move from `vendor-project.toml`, `verification-addon.toml` and the legacy `project verify` index to it. One application lifecycle executes suites; auxiliary inputs do not inflate coverage; stale/unreviewed evidence cannot satisfy gates. |
| 14 | pending | Remaining declared Wi-Fi/Bluetooth/coexistence/radio-leaf suites as typed scenarios on the stage-12 mechanisms. Every original TOML profile/disposition/baseline suite has a native scenario and checked expected MATCH/DIFF/INCOMPLETE; preserve exclusions/claim strength; compiled production paths are required. The `verification/vendor/*/blobray-provider` crates, their legacy tests and the TOML profiles/dispositions/baselines are removed once replaced. Split independent large groups into separately accepted sub-stages before activation, without dropping suite obligations. |
| 15 | pending | After stage 20. Vendor revision snapshots/prepare-update/diff, symbol correspondence/lineage and reviewed rebase of assertions/boundaries. Rename is evidence, not acceptance; changed bodies/layouts/applicability invalidate dependencies; ambiguous mappings require decisions; A→B→reviewed rebase retains A. |
| 16 | pending | After stage 20. Incremental reuse and bounded parallelism with complete dependency identities. Identical work reuses results; independent edits invalidate only dependents; missing-dependency regressions, serial/parallel equivalence, cancellation/capacity atomicity and measured work reuse are required. |
| 17 | pending | After stage 20. Permanent storage: transitive retention roots/pins, reachability/reclaimable report, GC preview/apply and compaction. Review evidence/active readers survive; roots are revalidated at apply; interruption preserves committed projects; reclaimed bytes are verified separately from temporary quotas. |
| 18 | pending | After stage 20. Pseudo-Rust and separate executable-reference consumer, single/batch generated/blocked manifest with provenance. Pseudocode is not executable evidence; generated supported code compiles and is tested; unsupported semantics block generation; no production substitution/qualification. |
| 19 | pending | After stage 20. TUI over shared application/read APIs; completions/manpage and consistent diagnostics/progress/details. No frontend analysis/workflow duplication; partial/stale/conflicted states, empty/large streams, cancellation and errors are tested; help/examples/generated docs agree. |
| 20 | pending | Legacy removal, executed right after stage 14 and before stages 15–19. Every consumer is migrated (stages 12–14), no legacy runtime call remains, and the old engine, launcher, `src/`, legacy crates (`contracts`, `analysis-model`, `semantics`, `execution-model`, `backend-riscv`) and their dependencies are removed; reviewed machine inputs are preserved. Legacy capabilities owned by stages 15–19 are removed without porting; those stages add them later as Next features. Generic standalone/register publication and real workflows/recovery pass. |
| 21 | pending | Stable persistence after stages 15–19: freeze a research fixture corpus and transitive persistence contract that subsequent versions must read without conversion. |

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
  integration suites and the repository CI checks.
- Real workflows use built-in supervision with explicit limiter mode. Kernel
  cgroup integration requires delegated controllers; unavailable is not passed.
  Watchdog evidence never claims kernel enforcement. Hardware qualification is
  independent and is not a completion claim of this pipeline.
- Checkpoint meanings: 01 current promises; 06 static research; 10 execution and
  comparison; 12 practical PHY; 14 declared radio campaigns; 17 long-lived
  research; 20 complete replacement and stable persistence.

## Current execution position

Stages 00–10 and both corrective checkpoints below are complete, including all
four stage-10 comparison profiles and their combined acceptance. The 10.L linker-capability checkpoint is complete after 10.4. Execution has
resumed on explicit user instruction. Stage 11 is complete, including both
command-memory and transport acceptance. Stage 12.1 is complete under the full
calibration/RF assignment below. Checkpoint 12.H is complete. Stage 12.2 is complete, including PBus/DCODE
execution, negative outcomes and preservation. Stage 12.3 is complete: RFPLL
search, maintenance, frequency memory and restored replay. Stage 12.4 is complete:
gain arithmetic, coefficient boundaries and full Wi-Fi/BT publication. Stage 12.5
is complete: gain state and the RF-test producer. Checkpoint 12.R is active:
typed Rust verification scenarios replace the Python runners before 12.6.
Checkpoint 12.R, including checkpoint 12.P (execution performance), is
complete; no Python scenario code remains. Unit 12.6 is complete: channel
restoration, all temperature-prefix sensor windows and stuck readiness.
Checkpoint 12.M (mechanisms instead of handwritten knowledge) is complete.
Units 12.7 (RX gain/calibration) and 12.8 (TX-DC/PWDET) are complete.
Checkpoint 12.N (native claims) is active: 12.N.1 is complete and 12.N.2 is
next; unit 12.9 follows the checkpoint. Stages then
run in the order 13, 14, 20 (legacy removal), 15–19, 21.
Format numbers and active positions in earlier acceptance notes are historical
checkpoints; this section and the stage tables define the current position.
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
| 04.3 | done | Flow/effect slices and reviewed paths/event routes: selector delivery, static callback registration/delivery and broker subscription with exact participants, sites, fields/selectors and evidence. Structural paths, conditional reviewed routes and executable evidence remain distinct. Missing/ambiguous/mismatched steps, cycles and bounds cannot claim completion; complete query/review/export/reopen scenarios close the remaining stage 04 obligations. Runtime replay remains a later execution consumer, never synthesized by navigation. |


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
| 04.3.2 | done | RAM definitions reaching an exact saved call/store publication anchor: incoming, must/alternative/candidate last writes; partial overlap, unknown alias and call clobbers remain explicit. Iterative bounded CFG/dataflow, local witnesses and values; joins, loops, killed definitions, unknown calls, resource failures and source-free query/export pass. It does not invent interprocedural memory effects. |
| 04.3.3 | done | Native reviewed selector delivery, static callback registration/delivery and broker subscription routes with exact participants, sites, object/queue/domain/selector fields, callback identity, case handler and optional terminal. Query authenticates physical evidence and reports condition/lifetime/order blockers separately; review never proves actual asynchronous delivery. Positive/negative/ambiguous/source-free review/query/export cases for all three close stage 04; runtime replay remains stages 07–10. |


Stage 04.3.1 acceptance covers deterministic bounded graph witnesses, native
path review, ambiguous selected interpretations, exact record/occurrence failures,
source-free exports and the authenticated linked PHY effect inventory. The real
workflow preserves all 44 composed fill records and the expected focused first
write after move/backup/restore. Affected package tests, the full Next suite,
Clippy, formatting, public/private docs and standalone tests pass. The acceptance
run also closes runtime cleanup/reconciliation serialization with a focused lock
and capacity-release regression; failed initialization releases the root lock
before cleanup. No runtime path feasibility or hardware qualification is claimed.


Stage 04.3.2 acceptance covers saved-instruction joins, killed writes/clobbers,
loop anchor iterations, stable versus repeated pointer loads, partial-width
writes, unknown aliases, exact record identities and resource/malformed-request
failures. CLI/API source-free exports agree. Authenticated linked PHY prologue
writes retain exact CFG suffixes, partial-tail status and intervening call barriers;
restored exports are byte-identical. Full affected package/Next tests, Clippy,
formatting, owned public/private documentation and standalone checks pass.


Stage 04.3.3 acceptance covers all three native event mechanisms through query,
proposal/acceptance, source-free export and backup/restore. Wrong selectors/fields,
overwritten or truncated callback stores, ambiguous selected interpretations,
cycles, wrong records and capacity/work failures cannot publish an accepted route.
Borrowed navigation facts share physical target keys; checks retain explicit
service-semantics, lifetime/order/context/guard obligations. Full affected/Next
tests, focused regressions, Clippy, formatting, public/private documentation and
standalone checks pass. The authenticated PHY workflow reopens identical retained
research/data/interface/flow/memory-slice exports under watchdog supervision.
Native storage/journal are 17/18, including schema-18 ephemeral query run records;
no compatibility reader or converter is introduced. Stage 04 is closed.


Stage 05 acceptance covers saved register discovery through generic proposal,
acceptance, conflict rejection, source-free export and backup/restore. Candidate
access widths, expression masks, unknown/alternative addresses and coverage remain
separate from physical declarations. Scoped interval/name indexes preserve exact
applicability and exclude retired assertions from active conflicts. Native source
initialization/SVD import creates unreviewed models, retains original inputs and
rejects invalid geometry without overwriting existing sources. Synthetic explicit
source review validates and generates all four publication outputs; the existing
ESP32-S31 publication passes its source check. Full affected/Next tests, Clippy,
formatting, public/private documentation and standalone checks pass. Authenticated
PHY/ROM research preserves four alternative ROM addresses and 44 composed PHY I2C
writes in byte-identical restored catalogue exports under watchdog supervision.
No new persistence schema, compatibility mechanism or hardware qualification claim
is introduced.


### Stage 06 execution boundaries

The saved semantic bundle and exact trace evaluator have separate acceptance
boundaries. Existing function facts are the native semantic IR; building a bundle
must reuse those records and their physical identities rather than introducing a
second decoder or copying the legacy symbolic engine. Project-wide configuration
and automatic campaign execution remain stage 13; this stage accepts explicit
captured research scopes and named IR build profiles.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 06.1 | done | Native configured semantic IR build from explicit saved publications/analyses: named profiles, all/prefix/exact roots and optional resolved call closure; original function facts, exact/ambiguous links, coverage and transitive saved provenance. One supervised build owns memory/work/disk, stages and atomically publishes one immutable bundle. API/CLI show/export, duplicate/missing profiles, ambiguity/cycles, capacity/cancellation, source removal and restore pass. No hidden binary analysis, second semantic engine or legacy adapter. |
| 06.2 | done | Static observable trace extraction/comparison over saved IR with explicit physical observation scope. Ordered exact traces, symbolic/may-effects and concrete execution remain distinct. Unknown branches/addresses/calls, loops and unsupported effects retain blockers and cannot MATCH; exact equal/different cases yield MATCH/DIFF. Trace evidence/conditions/provenance and exports survive source removal/restore; real saved scope plus full source-only checkpoint close stage 06. |

The IR build owns a result, not an alternate analysis cache. Root selection and
call closure use the existing application navigation resolver; the bundle retains
original local/composed records with their evidence identities. New durable result
storage follows existing staged-receipt/publication rules. Scientific claim strength
is unchanged by packaging; the static trace consumer alone establishes its explicit
exactness profile, and never labels a may-effect inventory an execution trace.


Stage 06.1 acceptance closes configured saved IR build/show/export through the
shared application lifecycle. Prefix/all/exact roots, root-only versus reachable
profiles, a linked diamond, cycles and ambiguous finite callback targets pass.
Original local/composed facts and transitive evidence-only functions retain their
identities and coverage. Memory/work/disk exhaustion, cancellation, changed admitted
requests, inconsistent summaries, SQL publication failure and corrupted CAS streams
fail without publishing a substitute result. Source-free export and backup/restore
are identical. Full affected/Next tests, the final linked regressions, Clippy,
formatting, public/private API docs and standalone checks pass. Native database 18
and journal 19 identify IR results; no compatibility reader or second analyzer is
introduced. Stage 06.2 remains active and owns static-trace exactness and the full
source-only checkpoint.


Stage 06.2 acceptance closes native saved-IR trace extraction/comparison with
ordered MMIO/fence observations, explicit input/ABI assumptions and exact physical
scope. Linked calls and tails, symbolic reads, known differences, unknown branches,
addresses and value equality, cycles, unsupported effects, malformed requests,
capacity/work/cancellation and source-free exports pass. May-effects are never
promoted to traces. The authenticated ROM fill case independently checks the store
at 0x2010fc00, and MATCH/DIFF/INCOMPLETE survive source removal and restore.
Full affected/Next tests, Clippy, formatting, public/private docs, standalone and
the complete source-only checkpoint pass. Native database/journal are 19/20;
function schema/policy are 7/8. Exactness is conditional on the selected ABI and
immutable-code assumptions; neither trace comparison nor source-only checks claim
hardware qualification. Stage 06 is closed.

### Stage 07 execution boundaries

The existing executor accepts eight concrete register words and one entry with
independent/stateful cases. Stack placement, atomic memory transactions and session
phase/goal transitions change distinct contracts. Split these before activation,
preserving all parent acceptance and sharing the existing execution lifecycle:

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 07.1 | done | Explicit known/unknown RV32 integer argument words, register/stack ABI placement and bounded stack capacity. Shared request/backend/memory contracts, API/CLI execution/comparison/replay, source-free restore, more than eight arguments, unknown consumption and malformed/resource failures pass. No implicit zero arguments or stack initialization. |
| 07.2 | done | RV32 LR/SC/AMO through an explicit atomic memory port with owned reservation state. Single-hart ordering, reservation invalidation, unknown/unaligned/unsupported locations and all supported operations are tested; no nonatomic fallback or invented peripheral behavior. |
| 07.3 | done | Multiple exact entry/setup phases, explicit region ownership/persistence and cold/warm reset transitions. One application budget and publication; dependent-phase blocking, isolated/stateful data and exact source-free replay pass. |
| 07.4 | done | Explicit return, reach-symbol and observe-call completion goals over physical captured identities. Goals, premature returns, unresolved targets and phase failure have distinct evidence; API/CLI/comparison/replay and negative cases close stage 07 without claiming unobserved completion. |


Stage 07.1 acceptance closes explicit optional integer words and bounded aligned
stack placement across API/CLI execution, comparison and replay. Tests independently
check a7/stack arithmetic, SP at 0/8/9/12/13/256 words, unknown register consumption,
unknown words overriding filled stack seeds, uninitialized padding, oversized and
misaligned/overflowing stack geometry, capacity/cancellation and source-free restored
replay identity. All affected packages/Next tests, Clippy, formatting, public/private
docs and standalone pass. Execution request/manifest schema 2, database 20 and
journal 21 identify the new contract. Clients supply physically lowered words;
no type or variadic layout is inferred. Stage 07.2 is active.


Stage 07.2 acceptance closes LR.W/SC.W and all nine AMO.W operations across all
four aq/rl combinations. Independent expected old/new values, overlapping/disjoint
writes, replacement reservations, failed SC permission checks, phase/implementation
isolation, unknown/misaligned/read-only/MMIO locations and callback admission failure
pass. Atomic instructions run through the shared API/CLI evidence lifecycle; exact
replay of a known difference survives source removal and backup/restore. All affected
packages/Next tests, Clippy, formatting, public/private docs and standalone pass.
Execution schema 3, database 21 and journal 22 identify the explicit single-hart
program-order profile; no multi-hart, device or weak-memory guarantee is inferred.

Stage 07.3 implements explicit cold/warm reset per phase and physical entry per
invocation, replacing the global case mode and fixed entry. RAM declarations carry
phase/session lifetime; captured writable ELF is session-owned, stack and MMIO are
phase-owned. A cold phase creates a new independent dependency chain, while a warm
phase after incomplete execution remains blocked. The first phase must be cold.
These are native contracts, with no compatibility interpretation of old requests.


Stage 07.3 acceptance closes per-invocation entries and per-phase cold/warm resets.
Setup/read chains retain only session RAM and writable ELF; phase RAM/stack release
immediately after evidence delivery, with a memory-release regression. Cold reset
releases both old implementations before allocating new sessions, verified by a
32 MiB test swapping a large RAM allocation between sides. Invalid first-warm/entry
requests, live lifetime changes, missing state and later-phase resource failure
cannot publish a successful prefix. Warm successors block after incomplete phases;
a cold successor runs while preserving earlier incompleteness. The multi-entry
chain replays identically after source removal and backup/restore. All affected
packages/Next tests, Clippy, formatting, public/private docs and standalone pass.
Execution schema 4, database 22 and journal 23 identify these native transitions.
Stage 07.4 owns explicit goal completion; phase execution currently requires return.


Stage 07.4 closes return/reach-symbol/observe-call completion. Physical static/dynamic
symbol identities are validated against captured executable load mappings; zero-sized
FUNC/NOTYPE boundaries and aliases need no inferred function extent. Goal preparation
groups all phase/side requests by object and releases prepared ELF before sessions;
a regression asserts one preparation across different goals and both implementations.
Direct/indirect/x5 calls, explicit tails, canonical returns, premature return, starting
at a boundary, companion mapping, invalid symbols/relations and resource/cancellation
cases pass. MATCH/DIFF/INCOMPLETE remain scoped to the observed prefix; early goals
never claim target-body execution. Source-free backup/restore/replay is identical.
Store rejects incompatible outcome kinds and invented dependency blocking. All
seven affected packages/Next tests, Clippy, formatting, public/private docs and
standalone pass. Execution schema 5, database 23 and journal 24 identify this boundary.
Stage 07 is complete.

### Stage 08 execution boundaries

Inspection confirms two distinct legacy ownership mechanisms: peripheral instances
with end-of-lifetime coverage (`execution-model/device`) and ABI call responses with
normal-memory outputs/allocation (`backend-riscv/execution/model`). They require
separate complete scenarios but share native model provenance/lifetime/coverage.
Keep every listed standard model; do not import either legacy runtime or add a
compatibility model alongside the current register bank.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 08.1 | done | Native register-bank, constant/sequence read, W1C, read-clear, self-clearing, FIFO and indexed-bank mechanisms with explicit identity/applicability and phase/session lifetime. One supervised execution owns state, resource admission and model participation/closure evidence. Missing/extra/mismatched accesses, exhausted/unconsumed sequences, invalid geometry/overlap and cold/warm closure are tested; unmet model obligations cannot MATCH even if the entry returned. All mechanisms work through API/CLI/query/replay/source-free restore, with code outcomes distinct from model coverage. |
| 08.2 | done | Explicit external-call return words, private-stack/normal-memory outputs, bounded allocation and delay events. Exact selected bindings, ABI clobbers/stack words, consumed responses, modeled/code boundaries and provenance are checked. No implicit return, MMIO output fallback or hidden allocator. Unknown pointers, wrong ownership, capacity/exhaustion and unconsumed obligations remain explicit. A composed call/device/phase scenario, all mechanisms' positive/negative regressions and source-free restore/replay close stage 08. |

Hardware-specific mechanisms and calibrated expectations remain their assigned
stages 11/12/14. A model declaration is an explicit execution assumption, never an
automatic scientific review or hardware qualification. Shared FIFO services and
reviewed interface dispatch remain stage 09 rather than a second device implementation.


Stage 08.1 closes all eight standard device mechanisms through shared execution,
comparison, query and replay. Independent expected values, exhausted/mismatched
accesses, unconsumed obligations, cold/warm closure, port gaps and live ownership
conflicts pass. A returned program with unmet model obligations remains incomplete;
store rejects missing/forged identity, counts, closure and MATCH evidence. Model
payload/state release, admission failure and cancellation before consumption pass.
API/CLI and source-free backup/restore/replay preserve identities and outcomes.
All seven affected packages/Next tests, Clippy, formatting, public/private docs and
standalone extraction pass. Execution schema 6, database 24 and journal 25 bind
model assumptions separately from code goals. Stage 08.2 is active; external calls
and delay events remain its obligations, with no implicit response or legacy path.


Stage 08.2 closes explicit captured-code/unmapped call boundaries, optional return
words, caller-saved clobbers, stack ABI words, checked normal/private-stack outputs,
bounded fresh allocations and modeled microsecond delays. Required responses and
phase/session closure remain distinct from actual code goals. Unknown values,
invalid ownership, exhausted/unused responses, tail policy, observe-call precedence,
admission/cancellation and prefix accessibility/release pass. Store rejects forged
binding/argument/effect/return/consumption/closure evidence. A composed allocation →
warm RAM read → call → device scenario has checked returns and identical API/CLI
source-free backup/restore/replay; changed delays or known returns yield DIFF even
with incomplete coverage. All affected packages/Next tests, final 43 execution tests,
Clippy, formatting, public/private docs and standalone checks pass. Execution schema
7, database 25 and journal 26 identify the new boundary. No compatibility layer or
hardware timing claim is introduced. Stage 08 is complete.

### Stage 09 runtime interfaces and services

The legacy owner combines table placement/lifecycle with FIFO operations and a
service-event goal. Native reviewed interface contracts already exist; the missing
runtime table owner must precede reviewed service binding. Split these complete
scenarios without dropping any table/service obligation or copying legacy's
per-call scan over every table byte:

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 09.1 | done | Explicit selected accepted interface contract → runtime table/pointer placement → captured or explicitly modeled callback execution → retained lifecycle and source-free replay. Resolve physical roots and bounded paths, validate layout/ABI/index/guard conditions and exact slots; null/missing/ambiguous targets remain distinct. Track initialization, pointer installation, writes and indirect-target associations with bounded operation-owned indexes; association never claims unsupported pointer provenance. Wrong review/source/layout/guards, overlapping ownership, unknown/partial writes, alias targets, cold/warm state and resources fail explicitly. All root/path forms admitted by the reviewed contract retain an implementation or a named unmet criterion; no silent supported-profile reduction. API/CLI/reopen agree. |
| 09.2 | done | Stateful FIFO enqueue/dequeue/length through explicitly selected reviewed slot bindings, with argument/private-stack input, output and wake behavior. Bounded isolated queues persist by declared lifetime; full/empty/order/wrong handle/width and cross-phase failures are checked. Observe-dequeue service goals stop only after the selected successful event; failed phases never imply completion. A reviewed table → service → event-goal scenario, API/CLI/source-free restore/replay, resource/cancellation/closure and all positive/negative mechanisms close stage 09. |

Standard FIFO device transcripts remain stage 08's explicit peripheral mechanism;
stateful queue services own separate data and semantics. Execution uses the existing
call port, session memory, supervision and publication, not a second scheduler,
legacy adapter or generic workflow framework. Review authorizes a selected contract,
not automatic acceptance of a hardware claim.


Stage 09.1 closes selected runtime interface placement, all physical root forms,
checked paths/guards, captured/model callbacks, partial slot writes and bounded
current-target association. Regression coverage includes ownership and phase
release, frozen accepted/rejected selections, unavailable/ambiguous targets, ABI
admission, cancellation/resource atomicity, forged lifecycle evidence and
source-free CLI/API restore/replay. Affected package/Next tests, Clippy, formatting,
public/private documentation and standalone checks pass. Service queues and
service-event goals remain wholly owned by active stage 09.2.


Stage 09.2 closes reviewed FIFO enqueue/dequeue/length, explicit handle/width and
private-stack effects, wake values, isolated phase/session rings and selected
successful dequeue goals. CLI/API source-free backup/restore/replay agree.
Invalid bindings/handles/inputs/outputs, failed warm dependencies, expired owners,
resource/cancellation failures and forged transcripts are covered. Ring buffers
release on closure and reuse bounded instance slots; retained validation admits
its own queue buffers. All affected tests, 66 Next execution tests, Clippy,
formatting, public/private docs and standalone checks pass.

After completing stage 10.4 and its acceptance checks, commit and push the
completed stage, then pause this plan before stage 11 as requested by the user.
Remaining stage obligations remain pending.

### Stage 10: comparison checkpoints

The original row combines four independently reviewable comparisons. Split it
before implementation without dropping any legacy relation or observation. Each
sub-stage owns its complete API/CLI, retained validation/reopening, contracts and
required gates; no generic policy framework or compatibility path is introduced.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.1 | done | Explicit per-case comparison selection and bounded final normal-memory observations, alongside selected return words and existing ordered MMIO/fence/delay events. Physical paired ranges have exact lengths and identities; all selected bytes, including unchanged bytes and unknowns, are represented. RAM-only and high/low-return differences DIFF; equal known completed selections MATCH; unknown/unavailable/unfinished results remain INCOMPLETE. Excluded observations remain retained. Invalid/overlapping/overflowing selections fail, phase lifetime/resource/cancellation atomicity and source-free replay pass. No inferred layout equivalence. |
| 10.2 | done | Ordered captured-code, modeled and service call observations with physical targets, ABI words and exact interleaving with selected observables. Explicit reviewed semantic call pairs support exact/selected/ignored argument policies; unlisted calls remain retained and their exclusion is visible. Missing/ambiguous/unreviewed pairs fail; reordered/missing calls and selected argument-only differences are caught; unknown words stay incomplete. Shared application API/CLI and source-free retained comparison pass. |
| 10.3 | done | Ordinary RAM access/atomic and branch timeline observations plus explicit reviewed ABI/layout projections for corresponding memory/arguments and control observations. Validate exact domains, widths, aliases/overlap, offsets and applicability; no dropped unknown/missing fields or automatic pointer normalization. Final-state-only equivalence stays distinct from ordered internal-state equivalence. Different layouts can match only under the selected valid projection; invalid/stale projection and branch/RAM-order changes are tested. |
| 10.4 | done | Reviewed effect contracts classify required, omitted, replaced and added MMIO/delay/fence effects, preserving reason and claim ceiling. Absent/unclassified/conflicting rules fail closed; known effect differences DIFF and incomplete execution cannot MATCH. Policy identity, raw excluded evidence, provenance and source-free replay remain retained. Compose all stage-10 relations, run the full execution/comparison/recovery suites and source-only checkpoint; close stage 10 only after all four sub-stages pass. |


Stage 10.1 closes explicit per-case event/return/final-memory selection and
complete byte snapshots with separate availability/knownness. Physical pairing,
unchanged-byte and high-return differences, excluded observations, phase release,
interrupted-state claim limits, source-free CLI/API restore/replay, capacity before
allocation and canonical retained validation pass. All affected tests, 73 Next
execution tests, Clippy, formatting, public/private docs and standalone pass.

Before activating 10.2, separate its two complete comparison profiles. Instrumented
physical calls and reviewed cross-implementation correspondence have different
acceptance owners; the second introduces a proposal/review applicability lifecycle.
Both remain required to close 10.2.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.2.1 | done | Explicit bounded physical call/tail-transfer capture, known/unknown register and selected stack ABI words, exact target comparison and interleaving with selected MMIO/fence/delay. Captured code, call models and FIFO service boundaries are represented; observe-call goals retain their pre-dispatch semantics. Site/transfer provenance and excluded observations remain available. Target/order/argument-only differences, unknowns, invalid capture, resource/cancellation atomicity, API/CLI/source-free replay and all gates pass. No semantic pair is inferred. |
| 10.2.2 | done | Native proposal/review of semantic call correspondence anchored to exact captured occurrences or explicit modeled binding identity. Explicit exact/selected/ignored word policies and selected pair scope; no name fallback or implicit ABI projection. Invalid, unaccepted, ambiguous, mismatched/stale pairs fail; all listed and unlisted calls remain provenance. Ordered reviewed comparison, generic/specialized API/CLI, source-free query/replay, resource/atomicity checks and all gates close 10.2. |


Stage 10.2.1 closes opt-in physical call groups, exact targets/selected RV32 words,
pre-dispatch goals and ordered interleaving with MMIO/fence/delay. Captured code,
modeled/FIFO boundaries, explicit tail candidates, unknown/unavailable stack words,
exclusions, bounded owner release and malformed retained groups are covered.
Store rejects invented MATCH with unknown selected call words. All affected tests,
80 Next execution tests, Clippy, formatting, public/private documentation and
standalone pass. Source-free API/CLI restore/replay and failed-run atomicity pass.
Execution schema 11, database 29 and journal 30 identify this physical profile;
semantic correspondence remains active stage 10.2.2.


Stage 10.2.2 closes native call-pair proposal/review, exact/selected/ignored word
policies, explicit unlisted-call scope and immutable accepted selection. Both
captured endpoints, model/FIFO definition/binding/phase applicability, ambiguity,
raw excluded/unknown words, source-free generic/specialized API/CLI restore/replay,
resource atomicity and retained policy integrity are covered. All affected tests,
88 Next execution tests, Clippy, formatting, public/private docs and standalone
pass. Execution schema 12, database 30 and journal 31 identify this relation.
Stage 10.2 is complete.

Stage 10.3 has two complete acceptance units: acquiring/comparing the physical
internal timeline and reviewing cross-layout/ABI correspondence. The physical
checkpoint never claims equivalence under an unimplemented projection. Both are
required before the effect-contract checkpoint; no original obligation is removed.

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.3.1 | done | Explicit bounded normal-memory read/write/atomic and conditional-branch observations, with exact ordered physical comparison interleaved with selected calls/MMIO/fence/delay. Include declared model/service normal-memory effects once; setup/inspection reads are provenance rather than invented guest transactions. Capture/compare selections are explicit; unknown/inaccessible/unfinished states cannot MATCH, excluded evidence stays available, and equal final state cannot hide a selected timeline difference. All widths, LR/SC/RMW, branch choices, model effects, phase ownership, resource/cancellation atomicity, retained validation and source-free API/CLI restore/replay pass with all gates. |
| 10.3.2 | done | Native reviewed ABI/layout projections for corresponding final-memory fields, call words and internal memory/control observations. Exact endpoint domains/widths/offsets/aliases/applicability and every selected field are validated; no implicit pointer normalization, dropped unknown/missing field, unreviewed mapping or name fallback. Different layouts/ABI word positions can match only under the selected valid projection; reordered RAM/branches and known projected differences remain DIFF. Generic/specialized API/CLI, immutable review/source/definition retention, invalid/stale/conflicting mappings, resource atomicity and source-free replay pass. Compose physical and projected relations, then close 10.3. |


Stage 10.3.1 closes opt-in physical reads/writes/atomics and conditional branches,
interleaved with selected calls/MMIO/fence/delay. Modeled output/input and dynamic
allocation initialization enter once; setup and inspection remain separate.
Unknown/unavailable reads, equal-final-RAM timeline differences, all widths/AMOs,
compressed branches, admission before atomic mutation, phase release, forged
retained evidence and source-free API/CLI restore/replay are covered. All affected
tests, 97 Next execution tests, Clippy, formatting, public/private documentation
and standalone pass. Execution schema 13, database 31 and journal 32 identify this
physical profile. Reviewed cross-layout/ABI projection remains active 10.3.2.


Stage 10.3.2 closes reviewed same-width byte fields/arrays, physical conditional
branch correspondence and exact 32-bit ABI word-position projection. All declared
field/array bytes participate; unknown padding remains explicit outside selection.
Unmapped selected effects cannot MATCH. Both physical endpoints/instructions,
geometry/aliases/overflow, accepted/frozen/superseded/conflicting review, changed
source/entry, unknown fields/words, order/atomic outcomes, cross-chunk arrays and
bulk initialization are covered. Generic/specialized API/CLI, admitted owner release,
failed-run atomicity and source-free restore/replay pass. All affected tests,
105 Next execution tests, Clippy, formatting, public/private docs and standalone
pass. Execution schema 14, database 32 and journal 33 identify this profile. No
pointer-value, type-width or path normalization is inferred. Stage 10.3 is complete;
the effect-contract and composed source-only checkpoint remains active 10.4.


### Corrective checkpoint before continuing 10.4

| Correction | Status | Required acceptance |
| --- | --- | --- |
| 06.2 transfer state | done | Keep pre-transfer CallInputs; apply saved typed link writes on callee entry under trace policy 2. User-authorized exact saved indirect links may close only structural indirect CFG gaps; all other gaps remain blocking. ELF-to-IR-to-trace and concrete execution agree for x1/x5, 2/4-byte calls, tails, nested/repeated calls and selected effects. Section-relative addresses never become physical offsets; malformed facts fail closed. Source-free restore and existing real PHY/ROM trace acceptance pass before reclosing 06.2. |
| Local expression index | done | Operation-owned full-key interning preserves IDs, records, provenance and admission rollback. Collision, cancellation, capacity/release and 512/1024/2048 work-growth regressions pass without raising limits. |
| Runtime interface preparation | done | Group snapshot/object requests with admitted sorting and linear group passes; retain individual physical validation, result order and release before sessions. Work growth, cold/warm ownership, invalid selections and publication atomicity pass. Mixed callback/captured indirect calls retain the explicit strict-profile INCOMPLETE outcome and replay. |
| Documentation and full checkpoint | done | One current-format reference and coherent execution position; focused/public/private docs, formatting, Clippy, all eight core package unit suites, functions/images/execution/architecture, standalone, source-only and authenticated PHY/ROM watchdog acceptance pass. Artifacts remain ignored; unavailable inputs/checks keep obligations open. |

### Review corrections for completed stages 06 and 09

These corrections preserve the active stage-10 obligations and do not include a
shared FIFO/table policy refactor. Acceptance validates the isolated fixes and, as selected by the user, a frozen
copy of the current stage-10 WIP. Unfinished parallel edits remain owned by their
executor.

| Correction | Status | Required acceptance |
| --- | --- | --- |
| Static trace prefixes | done | Known differences in observed prefixes yield DIFF despite later blockers. Length differences require a completed shorter side; symbolic uncertainty alone cannot prove DIFF and incomplete paths cannot MATCH. Summary validation and trace policy agree. ELF-to-IR API/CLI, empty/unequal prefixes, symbolic values, source removal and backup/restore pass. |
| Retained interface validation | done | Store groups selected tables by frozen snapshot with admitted O(n log n) sorting and linear group traversal, retaining every per-declaration check. Grouping, publication and reopening work-growth tests, one history load per selected snapshot, cancellation/capacity release, invalid late entries and source-free replay pass without raising budgets. |
| Review correction acceptance | done | Both corrections pass core/Next tests, strict Clippy, formatting, owned public/private docs, standalone, authenticated PHY/ROM watchdog trace/restore and the final source-only checkpoint. Integration with the selected parallel snapshot is checked before closure; missing inputs or gates remain open obligations. |

Both corrections pass in the combined stage-10 tree: core/Next suites, strict
Clippy, formatting, owned public/private docs, standalone, authenticated PHY/ROM
watchdog trace/restore and the full source-only checkpoint. Application execution
and projection regressions supply the complete comparison contract. The review
correction acceptance is closed without a shared FIFO/table policy refactor.



Stage 10.4 closes reviewed required/omitted/replaced/added/forbidden concrete
MMIO/delay/fence contracts at exact captured entries. Immutable accepted selection,
claim ceilings, complete raw accounting, unknown alignment, exercise/value/count
obligations, shared generic/specialized API/CLI, physical endpoint validation,
conflicting acceptance, admitted ownership and retained-result forgery checks pass.
The composed ELF scenario covers reviewed call-word/layout relations, internal
memory/branch order, returns, physical final RAM and source-free restore/replay.
All affected suites (including 115 execution tests), formatting, strict Clippy,
public/private documentation, standalone, authenticated PHY/ROM watchdog
trace/restore and the full source-only checkpoint pass; the shipping host builds
in the blobray profile. Stage 10 is complete. Pause at the user-requested boundary
after committing and pushing; do not activate stage 11.


### Checkpoint 10.L: external ELF linker capabilities

LLD and GNU ld implement ElfAnalysisLinkV1 with bounded deterministic RV32
capability probes, independent dialect parsers and occurrence-qualified retained
observations. Application validates roots/extraction and ELF; GNU seekable output
uses a pre-admitted, hard-limited lease with reaped ownership and inode validation.
Raw evidence, normalized records, CLI/API queries and source-free backup/restore
are closed. Archive-order differences remain explicit tool behavior.

Both 46-case image matrices, parser/application/storage regressions, affected
package and Next suites, strict Clippy, formatting, public/private docs, standalone,
source-only and the shipping blobray build pass with LLD 22.1.8 and RV32 GNU ld
2.47. Recipe schema 2/policy 5, image schema 3, observation schema 1, database 34
and journal 35 identify this contract; older storage is rejected without migration.
GNU sources/builds and acceptance outputs remain ignored. The checkpoint ends before stage 11.


### Stage 11: authenticated PHY I2C execution

Two acceptance units separate command-memory execution using existing RAM/register
models from transactional analog-I2C peripheral responses. Both are required.
Probes only construct capabilities and lower arguments into shipping production
implementations; they contain no recovered algorithm or MMIO encoding. Chip
addresses and authenticated source identities stay in the ESP32-S31 verification
owner. Generic Blobray gains no chip or legacy-engine dependency. Expected values
must be independent of the other compared output.

| Sub-stage | Status | Required acceptance |
| --- | --- | --- |
| 11.1 | done | Authenticate archive/ROM and exact PHY object/table; link the real command-memory root with captured ROM callees. Execute all 45 command-RAM stores and descriptor/no-op leaves against freshly compiled production on zero, mixed and boundary parameters, with independent expected RAM/MMIO values. Retain source/probe identities, explicit ABI/layout/observation scope, known differences and unknown/resource outcomes. Source removal, move, backup/restore and replay reproduce records. Native API/CLI, focused regressions, documentation and required gates pass. |
| 11.2 | done | Bounded explicit packed-command peripheral responses for both hosts over a shared seeded analog bank, retained/scripted reads, completion and pending/busy ownership. Model identity and unused/pending obligations survive warm/cold lifetimes. Unknown registers/commands, widths, overwrites, exhausted samples, cancellation and capacity fail closed. Captured host-selection/read/write/masked/reset paths execute against compiled production under explicit observation scopes, including immediate/delayed completion and timeout cases with independently checked MATCH/DIFF/INCOMPLETE expectations. No peripheral model substitutes for code or RF/calibration algorithms. Native API/CLI, retained evidence, source-free restore/replay, real acceptance and gates close stage 11. |

RFPLL/calibration algorithms and measurement policies remain in stage 12; stage
11.2 supplies reusable peripheral responses, not physical timing or qualification.


11.1 acceptance: the authenticated native scenario checks four 45-command profiles,
four descriptor profiles and three no-op leaves against freshly compiled shipping
HAL/PAC/PHY probes. Explicit guest setup/register inputs, independent table/object
hashes, every expected write/output byte, changed-input DIFF, unknown-input and
missing-ROM INCOMPLETE, and capacity failure without publication are verified.
Moved-project replay and backup/restore reproduce positive and all negative evidence
without source copies. All 116 Next execution regressions, strict Next all-target
and probe-library Clippy, both workspace formatting checks, static docs and probe
public/private API docs pass. The probe doctest target is inapplicable, not passed.
Real execution uses watchdog; no kernel containment or hardware qualification is
claimed. Generic contracts/storage formats remain unchanged at this checkpoint.


11.2 acceptance: native packed-command declarations, admitted shared-bank state
and retained pending/sample accounting cover both ports, delayed commit, reset,
warm/cold ownership, invalid accesses, cancellation and capacity failures. Store
regressions reject forged accounting. The authenticated runner checks forty
transport cases against independent writes/read returns and freshly compiled
production, plus exhausted replies, out-of-field DIFF and two explicit timeout
INCOMPLETE cases. The full command-memory/transport route passes source removal,
move, backup/restore and exact positive/negative replay. All nine Next package
suites, 121 execution regressions, strict affected Next all-target Clippy, target
PHY/probe library Clippy, both formatting checks, owned public/private docs and
standalone extraction/tests pass. Standalone uses the documented absolute GNU
linker selection. Target-only doctests are inapplicable; the kernel cgroup test
remains environment-gated. Real execution uses watchdog and claims no hardware
qualification. Execution schema 16, database schema 35 and journal schema 36
retain the new model evidence; unsupported formats are rejected without conversion.


### Stage 12: calibration/RF acceptance units

The original stage spans independent leaves and composed calibration parents.
These units preserve its entire scope and close in order. Legacy profiles/tests
identify questions, inputs and claim limits; their execution code is not a Next
implementation. Every unit executes captured code and freshly compiled production
where comparison is claimed, independently checks expected values/outcomes,
retains explicit relations and environmental assumptions, and covers negative
resource/unknown/failure paths without partial publication. Each uses native
application/CLI operations, source-free reopening and replay after backup/restore,
current owner docs and the ordinary stage gates. No partial unit is completion.

| Unit | Status | Scope and additional acceptance |
| --- | --- | --- |
| 12.1 | done | Four finite leaves from `profiles/phy-calibration-leaves.toml`: TX-gain restore, forced digital gain, temperature-to-power and post-init AGC. Execute every declared input case with the current authenticated archive, independently expected writes/returns and compiled HAL/PHY. Preserve the restore-enabled domain and archive-versus-ROM temperature policy. Changed input DIFF, missing/unknown input INCOMPLETE, capacity/no-publication and restored replay must pass. |
| 12.H | done | Declarative harness checkpoint below: all three probe images, generated exports/roots/catalogs, shared explicit owner and Python preparation, executable-entry validation, compiled call boundaries and preserved Next scenarios. Stage 12.2–12.10 obligations are unchanged. |
| 12.2 | done | PBus clear and DCODE: all profiles in `phy_rfpll/calibration.rs`, `phy_rfpll/dcode.rs` and the PBus rows of the calibration profile. Retained values, both settle branches, delayed readiness, crystal selectors and eight measured bytes; stuck PBus/channel/I2C cannot publish partial codes or restore unfinished state. |
| 12.3 | done | RFPLL search, maintenance and frequency memory: all cases in `phy_rfpll.rs` and `phy_rfpll/memory.rs`. Zero/nonzero signed corrections, search commands, requested settles, retained memory/control transactions and timeout without restored hardware control. Preserve the installed-layout query exclusion explicitly and retain raw observations. |
| 12.4 | done | Gain arithmetic and publication: `phy_rfpll/gain_calculation.rs`, `phy_rfpll/tx_gain.rs`, `phy_rfpll/bluetooth_gain.rs`. Actual current coefficient selection and ROM kernel inputs, signed narrowing, additive current versus subtractive ROM behavior, boundary curves, complete Wi-Fi/BT publishers and bank wrap. Independently authenticate coefficients; changing coefficients changes dependency identity. |
| 12.5 | done | Gain state and RF-test producer: `phy_rfpll/gain_state.rs`, `phy_rfpll/gain_producer.rs`. Execute real backup/destruction/recovery/init/consumption and the separately authenticated RF-test power producer, including rounding/saturation and gain/MAC publication. Preserve their characterization scope; vendor storage support is not inferred for production. Missing RF-test input remains an unmet obligation, never an omitted case. |
| 12.R | done | Typed Rust verification scenarios, detailed below. Every current Python scenario, oracle and host test moves to a Rust owner package with unchanged cases, verdicts, negatives and preservation; Python is removed. Stage 12.6–12.10 obligations are unchanged. |
| 12.6 | done | Channel restoration: `phy_rfpll/channel.rs`. Actual callback installation, all temperature-prefix sensor ranges, full-root gain publication and committed channel/bandwidth/temperature; stuck readiness cannot publish gain or semantic output. Prefix and full-root evidence remain distinct. |
| 12.M | done | Mechanisms instead of handwritten knowledge, detailed below. Stage 12.7–12.10 obligations are unchanged. |
| 12.7 | done | RX gain/calibration: `phy_rfpll/rx_gain.rs`. Both complete roots, DC/table guards, signed estimators, delayed I2C/settle, projected coefficients and bank limits; failed channel, minimum search and shared budget preserve prior coefficients. Readiness observations and genuine output publication remain visible. |
| 12.8 | done | TX-DC/PWDET: `phy_rfpll/tx_dc_pwdet.rs`. Actual search/PBus/SAR children, Wi-Fi/BT selection, DC rows, constant/alternating samples and tone/settle paths. Independent PBus/SAR faults cannot publish calibration; observation capacity differs from time/work limits. Preserve seeded gain adjustment and explicit unused-read exclusions. |
| 12.N | active | Native claims, detailed below. Stage 12.9–12.10 obligations are unchanged. |
| 12.9 | pending | Combined calibration and tracking parents: `phy_rfpll/combined.rs`, `phy_rfpll/parent.rs`, `phy_rfpll/graph.rs`. Execute real children, guards/grant order, channel 13/HT40, client/thermal domains, RFPLL disabled/enabled and signed corrections. Failed TX preserves pre-calibration state and earlier completed power/RFPLL state. Modeled child completions cannot satisfy complete-parent acceptance. |
| 12.10 | pending | Combined practical-PHY checkpoint: all units and stage-11 scenarios work together under native identities, shared budgets and preservation. Complete required intrinsic/reviewed-summary coverage with direct semantic/unknown/resource tests and explicit applicability; changing model, summary or production invalidates identity. No summary impersonates executed capture. All relevant integration suites, standalone, formatting, strict Clippy, owned public/private docs and the repository CI checks pass before closing stage 12. |

Required engine/intrinsic or peripheral mechanisms belong to the first unit that
needs them and must be fully contracted/tested there; 12.10 audits their combined
coverage, not deferred partial implementation. Retain every original case and
claim restriction when expressing it as a native request. Any infeasible unit
uses the executor's `needs-replan` procedure, preserving all unmet obligations.


12.1 acceptance: all eleven declared four-leaf cases execute through native
comparison with independently checked writes/returns and a freshly compiled
capability-owning production wrapper. AGC retains its actual ROM child. Its
co-located archive section uses explicit ROM definitions and an authenticated
static SDK bootloader clock-symbol companion, captured before source removal;
no SDK body or synthetic callback executes in the selected cases. Positive,
changed-temperature DIFF, unknown/missing-input INCOMPLETE and event-capacity
failure without publication pass. The combined I2C/calibration route preserves
all original evidence and exact replay after move and backup/restore. Probe
release build, strict target-library Clippy, root/probe formatting and owned
public/private docs pass; the target-only doctest is inapplicable. Core interfaces
and formats are unchanged. The full stage-12 checkpoint remains in 12.10.


### Checkpoint 12.H: declarative verification harness

User-authorized insertion before 12.2; all stage-12 obligations remain intact.
Status: done.

- One Rust declaration owns each probe export, ABI wrapper, linker root and ELF
  catalog across all three probe libraries. Preserve existing names, ABI,
  projections and production behavior; remove every manual retention list.
- Share parsing between the procedural macro and build-time collection. Generate
  explicit roots, retain the catalog and validate every declared executable entry
  after release/fat-LTO linking through `cargo xtask build vendor-probes`.
- Share explicit owner setup and routine scalar/buffer adaptation. Complex async,
  assembly and ownership adapters remain explicit; delay/tail-call barriers retain
  their behavior and receive execution regression coverage.
- Share Python request preparation, catalog resolution, explicit ABI/buffer/phase
  construction and runner operations across current Next scenarios. Preserve all
  independent expectations, models, claim limits and existing executor/replay
  formats. Legacy provider scenarios remain compatible consumers, with migration
  assigned to their original stages.
- Acceptance: generator and negative catalog regressions, all three complete ELF
  inventories, incremental declaration changes, ordinary/delay/tail execution,
  all current I2C/transport/calibration cases and verdicts, failed-publication
  resource paths, source-free move/backup/restore/replay, research scenario,
  formatting, strict affected Clippy, public/private docs, architecture,
  standalone and source-only checks. Commit only after complete acceptance.
- Missing authenticated inputs or infeasible prerequisites require needs-replan
  preserving every unmet criterion; partial migration does not close 12.H.


12.H acceptance: all 172 prior probe exports and their ABI remain available;
all three release/fat-LTO images validate, with one additional owned entry for
ordinary-call/ROM-tail/delay acceptance. Shared primitive-array buffer preparation
preserves explicit addresses, seeds and lifetimes. Current I2C, transport,
calibration and research scenarios pass independent expectations, negative
outcomes and source-free preservation; the call-boundary evidence also restores
and replays. Generator, malformed-ELF, adapter, Python and actual RV32
addition/relocation regressions pass. Next architecture/execution, strict affected
Clippy, formatting, public/private docs, standalone and the complete source-only
checkpoint pass with both final images. The GNU acceptance linker was explicitly
selected. Private inputs and generated evidence remain in ignored outputs.
Executor/comparison/replay formats and all later stage obligations are unchanged.

12.2 acceptance: all 24 PBus and 16 DCODE cases execute captured children and
compiled production with independent command, frequency, NRX, sample and output
expectations. PBus compares every effect. DCODE's native selected relation covers
all writes/fences/delays and eight final bytes; the runner additionally checks all
other reads except the two command ports, while retaining the raw polling DIFF.
Three PBus timeouts and three DCODE channel/I2C/shared-budget failures preserve
unfinished state or unchanged output; changed samples, unknown parameters and
capacity failure have checked DIFF/INCOMPLETE/no-publication outcomes.
Finite read runs replace expanded sequence declarations with bounded encoded
storage and logical-count conservation, including cancellation and warm/cold
ownership tests. Independent PBus expectations exposed rv-asm's unsigned C.ANDI
immediate; the shared decoder now sign-extends it, with exhaustive immediate/register
lifting and concrete high-bit regressions and new producer identities. Execution
schema 17, database 36 and journal 37 reject prior formats without conversion.
All nine Next/core package suites (526 tests), strict affected Clippy, root/probe
formatting, probe release build/Clippy, owned public/private docs and standalone
checks pass. The kernel cgroup test remains environment-gated; real acceptance
uses explicitly selected watchdog mode. The combined stage-11/leaves/prefix route
passes source removal, move, backup/restore and exact replay for all retained
results. Full source-only remains the stage-12.10 checkpoint.


12.3 acceptance: all 36 captured RFPLL search cases and 48 maintenance cases
match compiled production under explicit relations. Independent expectations
check signed results, search commands/settles and every transaction across 85
frequency-memory words, including underflow/overflow and all five channel forms.
The single installed-layout query exclusion is checked explicitly; raw polling
DIFF remains retained. Production timeout leaves the command pending and software
frequency control unrestored. Changed capacitor DIFF, unknown callbacks and
unmapped diagnostics INCOMPLETE, and event-capacity/no-publication checks pass.
All 89 retained RFPLL results reopen and replay with identical identities after
source removal, move and backup/restore, together with the preceding I2C,
calibration-leaf and PBus/DCODE matrix.

Exact external definitions now acquire captured symbol/section/executable-mapping
identity without acquiring a runnable carrier. TLS and overlapping data segments
cannot grant execution authority; forged physical identities, competing executable
mappings, resource exhaustion and cancellation are tested. The RFPLL SDK diagnostic
definition is address-only; enabling that unprovided body remains INCOMPLETE.
The explicit `byte-addressed-memory-1` environment supports captured ROM memcpy's
unaligned ordinary data accesses within one mapping. Unknown bytes, permissions,
region boundaries and aligned-only MMIO/atomics remain enforced; timeline and
reservation regressions cover the same contract. The captured production delay
adapter saves/restores registers around its modeled time-observation edge; general
unknown-register handling remains strict.

All 532 Next/core tests pass (the delegated-cgroup test remains environment-gated),
along with affected strict Clippy, standalone, root/probe formatting, probe build
and Clippy, and owned public/private API and Markdown checks. This closes the
software RFPLL profile, without hardware, grant or RF qualification. No storage
conversion or compatibility reader is introduced.


12.4 acceptance: native requests execute all 150 current callback/ROM-kernel
input combinations, 360 compiled Wi-Fi arithmetic combinations, 24 complete Wi-Fi
publishers and 120 Bluetooth calculation/publication combinations. Another 72
cases select all 18 coefficient thresholds of each profile with both stack fills;
the original production matrix alone did not select every interval. Archive,
gain-object and coefficient-section identities are independently authenticated.
Actual kernel arguments, three coefficient copies, all 160/80 output bytes and
all 161/81 publication events satisfy independent expectations. Bluetooth executes
the captured callback installer and indirect callee; bank wrap and preserved
control bits remain observable. Eighteen vendor-only observations preserve current
additive adjustment versus the older ROM's subtraction and different tables.

Unknown curve and missing callback installation yield INCOMPLETE. Changing a
caller-supplied coefficient at the direct ROM boundary yields a completed DIFF and
a different execution identity. Event capacity failure publishes no execution and
preserves earlier evidence. All 196 retained executions across the original and
additional boundary matrices reopen and replay exactly after source removal,
project move and backup/restore. All three probe catalogs/builds, affected host
regressions, strict probe Clippy, root/probe formatting and owned public/private
docs pass; target-only doctests remain inapplicable. The generic engine, schemas
and architecture boundaries are unchanged. These are software gain-child claims,
not RF, protocol or whole-TXCAL qualification. No legacy runtime or compatibility
mechanism participates.


12.5 acceptance: storage runs without RF-test input: startup adjustment, six
adjustments and both fills through real backup, destruction, recovery,
parameter registration, isolation and both consumptions, with independent
backup-image, registration and 160-byte oracle expectations and no hardware
events. With the authenticated `librftest.a`, the real callback installer and
fifteen policy rows per fill check the adjustment byte, conditional gain
publication with all MMIO events and both MAC index writes. Negatives cover
two-sided corruption DIFF, unknown-cache INCOMPLETE, missing-callback and
unknown-policy-input INCOMPLETE without gain/MAC publication, and event
exhaustion without publication. Without `--rftest` the runner records the
unmet producer obligation and exits with status 2. With it, all 216 retained
executions reopen and replay exactly after move and backup/restore; without it,
all 204 do. Host oracle tests and the docs check pass. These are vendor
characterizations: no production storage or RF power API is inferred.


12.6 acceptance: the typed `channel` scenario links `phy_chip_set_chan` with
the real `phy_get_romfunc_addr` installer and authenticated ROM companions,
and executes parameter setup, installation and the transition in one session
against `open_phy_channel_trace_state`. Sixteen full-root cases (channels 1, 6,
11 and 13, both bandwidths and fills) match on native ordered writes and on
runner-side ordered reads/writes/delays, publish TX gain, and commit a channel,
bandwidth and temperature equal to an independent ROM sensor oracle and to
the production output. Forty temperature-prefix cases cover all five sensor
windows at codes 0, 64, 100 and 255. They are retained separately and
compare ordered effects only up to the first gain-bank read, with exactly one
sensor sample. Delay budgets are exact: out-of-range samples add the DAC
reselection's read-modify-write. Stuck readiness (both fills) returns
failure without gain data or semantic output. A changed production channel
is DIFF, omitted installation is INCOMPLETE, and event exhaustion publishes
nothing. All 60 retained executions reopen and replay exactly after move and
backup/restore. Host oracle tests, Clippy and the docs check pass. This is
channel state, not RXCAL or RF qualification.


### Checkpoint 12.R: typed Rust verification scenarios

User-authorized insertion after 12.5 and before 12.6. The Python runners in
`verification/vendor/projects/esp32s31/` build untyped request dictionaries,
duplicate native schemas, run outside `cargo`/`xtask` checks and add a second
language. All stage-12 obligations remain intact.

- One Rust package owned by the ESP32-S31 verification owner replaces every
  Python runner, harness, oracle and host test: research, I2C command memory and
  transport, calibration leaves/prefix, RFPLL, gain arithmetic/publication and
  gain state/RF-test.
- Requests and evidence use serde types from `blobray-domain`; no hand-built
  JSON schema copy. Blobray is invoked as the `blobray` process through its CLI,
  preserving the supervisor and explicit `--limit-mode`.
- A binary accepts explicit private inputs (no environment or filename
  guessing) and is launched by a `cargo xtask` command. Pure oracles, evidence
  interpretation and probe-catalog handling are ordinary `cargo test` cases
  that need no private inputs. Missing inputs remain unmet obligations with a
  nonzero exit.
- Chip addresses, source identities and expectations stay in the verification
  owner; generic Blobray gains no chip dependency. The new package declares its
  `open-radio` metadata and passes architecture and standalone checks.
- Acceptance: every scenario reproduces the Python case counts, independent
  expectations, MATCH/DIFF/INCOMPLETE outcomes, no-publication failures and
  source-free move/backup/restore/replay on the authenticated inputs under
  watchdog limits. All Python files are removed and owner docs updated.
  Formatting, strict Clippy, public/private docs, architecture, standalone and
  source-only checks pass. Partial migration does not close 12.R.

The JSON documents printed by the CLI are host wire formats assembled by hand
in `next`. A typed client needs one owner for them. Checkpoint 12.R therefore
has three acceptance units, all required:

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.R.1 | done | Host-owned wire types for record, run and inventory documents, used by the renderer and checked against CLI output. Scenario package, process runner, capture/import, probe catalog through `oer-probe-codegen` types, ABI lowering and request builders over domain types; `cargo test` covers the harness and gain oracles. `xtask` command. Gain arithmetic/publication and gain state/RF-test run in Rust with the Python case counts, verdicts, negatives, unmet-obligation exit and preservation; `phy_gain*.py` and their tests are removed. |
| 12.R.2 | done | I2C command memory and transport, harness call edges, calibration leaves/prefix and RFPLL runners in Rust with unchanged cases and outcomes; the corresponding Python is removed. |
| 12.R.3 | done | PHY research scenario (analysis, interfaces, IR/trace, navigation, flow, registers, memory slices, data/knowledge review and preservation) in Rust; `harness.py` and every remaining Python file are removed; owner docs, Blobray references and full checks close 12.R. |

12.R.1 acceptance: `blobray_next_host::wire` owns the record, run and
inventory documents. The renderer emits them, and execution/import CLI tests
decode real output into them. `oer-esp32s31-vendor-scenarios` builds requests
from domain types, reads the probe catalog through `oer-probe-codegen` types and
resolves image symbols with `object` instead of an external `nm`. It runs under
`cargo xtask vendor-scenario`. The Rust gain scenario reproduces the Python
runner on the authenticated inputs under watchdog limits: with `--rftest`, 237
operations and 216 restored replays, exit 0; without it, 204 replays and the
unmet RF-test obligation with exit 2. Harness, evidence, oracle and obligation
host tests pass without private inputs. `phy_gain.py`, `phy_gain_state.py` and
`test_phy_gain.py` are removed. Formatting, strict Clippy, architecture,
metadata, docs and standalone (532 tests) pass.

12.R.2 acceptance: the `i2c` scenario runs command memory, descriptors and
leaves, transport, harness call edges, calibration leaves, the PBus/DCODE
prefix and RFPLL in Rust over the shared session lifecycle. The SDK inputs are
explicit: `--sdk` enables leaves and prefix, and `--phy-sdk` adds RFPLL. Each
missing input is recorded as an unmet 12.1/12.2/12.3 obligation, and the
scenario exits with status 2. On the authenticated inputs under watchdog limits,
the Rust and Python runners perform the same 557 operations; they differ only
in one evidence label. Each has 138 restored replays and exit 0. The minimal run
exits 2 with three unmet obligations after its own source-free preservation. After
the move to the shared session, the gain scenario passed every execution assertion
and negative case again. At commit time its restore/replay phase was still running,
with 119 replays passed and none failed. The six migrated
Python files are removed. Host oracle tests, formatting, strict Clippy, docs and
architecture checks pass.


### Checkpoint 12.P: execution performance

User-authorized insertion before 12.R.3. A full authenticated gain run took about
33 minutes over 883 CLI operations. Profiling shows that process startup is about
5 % of that time. The dominant cost is per-case work inside Blobray. Each case and
side recreates a session, walks the complete 12 MB revision inventory to find one
input payload, and re-hashes that payload. The JSON scanner reads one byte per
dynamic call. The watchdog repeatedly reads `/proc`, and replay repeats the same
work. Emulation itself is negligible.

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.P | done | Resolve execution inputs and prepared images once per request and reuse them across cases, sides and phases. Find an input payload without walking the whole inventory. Remove redundant fixed costs of CLI operations: evidence returned by execute/compare/replay where a scenario would read it back, request size that forces tiny batches, and supervisor sampling overhead. Retained evidence, identities, verdicts and every negative/resource/cancellation contract stay unchanged or change only through a new rejected-without-conversion format. The authenticated gain scenario with `--rftest` completes in at most 30 seconds with the same cases, verdicts, negatives and per-execution restore/replay. Regressions show that inputs are prepared once per request; the I2C scenario and all Next suites pass. |

12.P acceptance: the authenticated gain scenario with `--rftest` took about
33 minutes over 884 operations before this checkpoint. It now completes in 27
seconds on an idle host (31–35 seconds while another build shares the CPU). It
keeps the same cases, verdicts, negatives, unmet-obligation behavior and
per-execution move/backup/restore/replay; each matrix is now one request. The
changes are:

- Execution resolves each image or captured input once per request and copies
  segments into fresh sessions.
- Storage indexes input payloads, run state and published executions, so no
  operation walks the inventory or scans the journal.
- Image and execution reads open only the payloads they use.
- Supervisors wait on process exit through pidfd; descendants come from
  per-task `children`.
- Reads are positional and 64 KiB-buffered, and run records decode without an
  intermediate `serde_json::Value`.
- Execution requests are canonical payloads retained by identity (execution
  schema 18, journal 38, storage 37) with named bounds `MAX_EXECUTION_REQUEST_BYTES`,
  `MAX_EXECUTION_CASES` and `MAX_EXECUTION_EVENTS`.
- Repeated literals became named domain constants: `CONTROL_MESSAGE_BYTES`,
  `DECODE_EXPANSION`, `STREAM_BLOCK` and the `DEFAULT_*` budget defaults.
- The CLI adds `--poll-ms` and `--grace-ms`. The scenario budget is a set of
  arguments, and shared scenario addresses live in `layout.rs`.

The I2C scenario with both SDK inputs passes in about 100 seconds. All 552 Next
and scenario tests, formatting and strict Clippy pass.

12.R.3 acceptance: the `research` scenario runs the authenticated PHY research
route in Rust. It covers:

- finite ROM address alternatives, the unresolved callback and its interface
  discovery;
- extent coverage, the linked command-memory image, and IR build with exact,
  DIFF and INCOMPLETE static traces;
- three repeated linked researches with phase measurements;
- navigation, flow targets and effects, the register catalogue, memory slices
  and storage usage;
- table, constant, pointer and callback review;
- manifest and span verification, doctor, move, backup/restore, and restored
  byte-identical exports and query documents.

All records decode into Blobray domain types. File exports use distinct
labels, so no retained stdout can overwrite an export. It passes on the
authenticated inputs under watchdog limits in 86 seconds; the Python runner
took 104 seconds. `harness.py`, `test_harness.py` and `phy_research.py` are
removed, and no Python scenario code remains. Owner documentation and the
design contract point to the Rust scenarios. The scenario package tests,
formatting, strict Clippy, the docs, architecture and standalone checks, and
the full source-only checkpoint pass.


### Checkpoint 12.M: mechanisms instead of handwritten knowledge

User-authorized insertion after 12.6 and before 12.7. A review of the
verification scenarios showed that about half of their handwritten content is
not vendor knowledge but work around missing Blobray mechanisms:

- most handwritten peripheral content enumerates registers that only retain
  written values, with the fill pattern as initial value;
- finite call models force exact requested-delay budgets per profile, found by
  exploratory runs;
- ROM companion lists of 20–26 names per scenario are found by repeated link
  failures.

Independent oracles, peripheral input values and comparison policies remain
handwritten; they are the evidence. Scenario 12.7 work in progress also needs
ROM data definitions as link companions and more than 65,536 recorded events.

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.M.1 | done | A retained MMIO aperture is a Blobray device: every aligned access in a declared range not claimed by an exact model is retained storage with a declared initial word, and every access stays an MMIO event. Scenarios declare the radio block once and model only semantic inputs; retained-register enumerations are removed. Both sides of a comparison must read the same never-written registers. Remaining addresses (inputs and independent expectations) are single named constants, shared ones in one owner; the published SVD is not a scenario dependency, because parts of it are recovered from the same vendor analysis. ROM storage constants name their ROM symbols and are checked against the captured ROM inventory. |
| 12.M.2 | done | A call declaration answers either a finite ordered response list or, for a single pure response without memory outputs or allocations, every call. Call counts and every observed argument and delay stay evidence. Execution schema changes without conversion. The per-phase event bound rises to 2^20, so a bounded 100,000-operation poll loop is observable without truncation. Scenario delay models use it, and exact per-profile delay budgets are removed; independent delay-sequence expectations stay. |
| 12.M.3 | done | A supervised query proposes ROM companions for a link request: a trial link reports every unresolved name, each resolved to exactly one defined function or data object in explicitly ordered candidate inputs, or reported unresolved. The retained link request still lists every exact companion. Companions may be ROM data objects without a load mapping. Scenario companion lists are replaced by proposals. |

Acceptance of the checkpoint: every existing scenario (gain with RF-test, i2c
with both SDK inputs, research and channel) passes on the authenticated inputs
with unchanged cases, verdicts, negatives and preservation. Blobray tests,
formatting, strict Clippy, docs, architecture and standalone checks pass.

12.M acceptance:

- **Proposals.** `propose-companions` trial-links a request with unresolved
  names permitted and resolves each undefined name in ordered candidate inputs.
  A companion may now be a ROM data object, such as `phy_param_rom`, without a
  load mapping. On the channel request the proposal reproduces all 26
  previously handwritten companions in 0.6 s. Gain, i2c, channel and RX gain
  keep no companion list. Research still exercises the named `--companion`
  CLI route.
- **Unbounded calls.** Call declarations are `finite` or `unbounded`, with
  execution schema 19 and call identity v2, rejected without conversion.
  Scenario delay models are unbounded; the per-profile delay budgets and their
  formulas are removed, and independent delay-sequence expectations remain.
  The per-phase event bound is 2^20.
- **Radio aperture.** `retained-aperture` is a device: exact ports take
  precedence, sub-word writes merge, misaligned accesses fail, and every access
  stays an MMIO event. Channel and RX gain model only semantic inputs and
  require both sides to read the same never-written radio registers; their
  retained-register enumerations are removed.
- **Addresses.** Shared addresses are single constants in `layout.rs`. The SVD
  is not a scenario dependency. ROM storage constants name `rom_phyFuns`,
  `phy_param_rom` and `g_phyFuns_instance` and are checked against the
  captured ROM inventory in every session.

On the authenticated inputs under watchdog limits, every scenario passes with
unchanged cases, verdicts, negatives and preservation:

| Scenario | Time |
| --- | --- |
| gain with RF-test | 52 s |
| i2c with both SDK inputs | 76 s |
| channel | 53 s |
| research | 86 s |

RX gain work in progress also passes on the aperture. All 2044 Blobray
workspace tests pass, as do formatting, strict Clippy, docs and the standalone
check.

12.7 acceptance: the typed `rx-gain` scenario links `phy_set_rx_gain_table`
with the real callback installer and proposed ROM companions. The co-located
diagnostics' `phy_printf` is proposed from `--phy-sdk`. The root is compared
with `open_phy_calibration_trace_rx_gain` over the radio aperture and explicit
inputs.

- **Profiles.** Sixteen publication profiles cover both DC/table guards, and
  forty calibration profiles cover samples 0, ±64 and ±2^24. Both include seeds
  1 and 17, both fills and both settle branches.
- **Checks.** Every profile matches on ordered effects, including readiness
  waits, and on the never-written registers read. Production stays within twice
  the vendor's steps. The 52 projected coefficients and two bank limits equal
  the vendor's committed state.
- **Failures.** Production-only failures preserve the seeded coefficients and
  gain memory: a channel that never becomes ready (4), an estimator that never
  completes (4), and slow successful minima that exhaust the shared budget
  (5, with more than one minimum).
- **Negatives.** A saturating production sample is DIFF, omitted installation
  is INCOMPLETE, and event exhaustion publishes nothing. All 8 retained
  executions replay after move and backup/restore.

Unit 12.7 also needed two Blobray changes, measured on the RX calibration
request:

- Evidence and query record spools were written unbuffered, with an `lseek`
  per fragment: about 15 million syscalls per request. They are now buffered.
- Checkpoints now sample the clock and report temporary usage on a stride,
  while cancellation and the work limit stay exact on every checkpoint.

The request's compare fell from 4.6 s to 0.75 s and its evidence read from
4.4 s to 1.1 s, with byte-identical execution identities and documents. Full
scenario runs:

| Scenario | Before | After |
| --- | --- | --- |
| rx-gain | 149 s | 25 s |
| gain | 52 s | 13 s |
| i2c | 76 s | 32 s |
| channel | 53 s | 20 s |
| research | 86 s | 82 s |

All 2049 Blobray tests, formatting, strict Clippy, docs and the standalone
check pass. This is RX calibration state, not RF qualification.

12.8 acceptance: the typed `tx-dc` scenario links `phy_txdc_cal_pwdet_init`
with the real callback installer and proposed companions (`phy_printf` from
`--phy-sdk`). It compares the root with `open_phy_calibration_trace_tx_dc_pwdet`
over the radio aperture.

- **Profiles.** 64 profiles cover Wi-Fi/BT selection, constant and
  alternating SAR samples, both tone-clear paths, both fills and both settle
  branches.
- **Checks.** Every profile matches on DC rows, ordered effects and
  environment-supplied registers, and the vendor keeps the seeded Wi-Fi gain
  adjustment. The only exclusions are PBus readiness waits and the three unused
  SAR result words, which remain explicit inputs.
- **Faults.** A stuck PBus (4) and a detector that never becomes ready (5, the
  SAR observation limit, distinct from timeout 2 and operation limit 6) publish
  no DC rows and never read the SAR result.
- **Negatives.** A changed tone-clear path is DIFF, omitted installation is
  INCOMPLETE, and event exhaustion publishes nothing. All 5 retained
  executions replay after move and backup/restore.

Unit 12.8 needed Blobray changes:

- **Boot-initialized data.** A writable `PROGBITS` section whose bytes the ELF
  carries inside a zero-filled load now starts with those bytes, as after ROM
  start-up. Before this, such data read as silent zeros; for example, the
  ROM's `g_phyFuns_instance` slot for the tone SAR reader was zero, and the
  vendor jumped to address 0.
- **Cyclic reads.** A `cyclic-read` device models periodic measurement streams
  without a declared read count.
- **Environment identity** is `boot-data-1`/`devices-4`/`external-calls-2`.
- **Faster reads.** Execution reads validate and emit records in one pass. A
  new `execution --summary` verifies payload digests without decoding
  records, and preservation now compares manifests with it.

TX-DC evidence is about 282 MB per matrix request; the scenario runs in about
45 s of operations. On the new environment every earlier scenario passes
unchanged:

| Scenario | Time |
| --- | --- |
| gain | 12 s |
| i2c | 30 s |
| channel | 19 s |
| rx-gain | 19 s |
| research | 83 s |

All 2055 Blobray tests, formatting, strict Clippy, docs and the standalone
check pass. This is TX calibration software state, not RF accuracy.


### Checkpoint 12.N: native claims

User-authorized insertion after 12.8 and before 12.9, following a review of
Next and of the verification scenarios. Blobray natively supports reviewed
effect contracts and layout projections. The scenarios bypass them:

- Blobray compares only MMIO writes;
- the actual claims live in scenario Rust code: filtered reads and delays,
  unused-read exclusions, `phy_param` ↔ production-output mappings and
  environment-supplied registers.

Retained verdicts are therefore weaker than the scenarios' claims, replay
re-checks only the weak part, and none of the stage-12 results reach
qualification. Stage 13 composes the evidence index from native claims, so
they must exist first.

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.N.1 | done | Runner-side effect filters become reviewed effect contracts evaluated by Blobray: transport plumbing, the single-microsecond delay before a transport or PBus status read, the vendor's unused SAR and skipped-DC snapshots, and readiness-wait visibility. A contract may declare `unclassified: required` (user decision): effects outside its rules then compare exactly in order and value, while the strict every-effect-classified policy remains available. An `ignored` disposition retains raw effects without pairing them, and a pattern may select an effect by its immediate successor. Blobray enters the selected root directly with deterministic callee-saved registers (user decision), so contracts and evidence bind the real vendor and production functions and the probe entry adapters are removed. Each contract is reviewed through knowledge assertions and bound by identity; verdicts carry the claim. |
| 12.N.2 | pending | Semantic output mappings (`phy_param` fields ↔ production output) become reviewed layout projections with final-state comparison. The aperture reports the never-written registers each side read as evidence, and a relation can require them equal. |
| 12.N.3 | pending | Execution capacity and cost: stack fill per case (no per-fill request split), lazy event-capacity admission, and a compact retained event encoding with an unchanged logical evidence contract. The largest current request (TX-DC) needs neither per-fill splitting nor hundreds of megabytes of evidence. |
| 12.N.4 | pending | Every existing scenario (gain, i2c, channel, rx-gain, tx-dc) moves to native claims. Scenario code keeps peripheral inputs, matrices and independent oracles; runner-side comparison code is removed. Cases, verdicts, negatives and preservation are unchanged; each scenario's retained verdict alone establishes its claim. |

Unit 12.N.1 acceptance: effect contracts gained the explicit
`unclassified: required` policy, the `ignored` disposition and the
`followed_by` successor selector. Verification and retained admission share a
one-event lookahead, so an unmatched effect under the explicit policy compares
exactly and ignored effects never shift alignment. Blobray enters roots
directly and the probe entry adapters are removed. The channel, RX-gain and
TX-DC scenarios propose their contracts through
`knowledge propose-effect-contract`, accept exactly those proposals and select
the reviews in every root relation with all four event channels. The runner-side
effect filters and environment-read comparisons are removed. The RX
skipped-DC snapshot is selected by its immediate successor, and the unused SAR
words are omitted reads. Temperature-prefix transitions now match completely.
Every retained root verdict carries `reviewed-effect-refinement`. All six
scenarios, cases, negatives and preservation pass; the i2c family's remaining
read exclusions move in 12.N.4. All Blobray tests, formatting, strict Clippy
and the docs check pass.
