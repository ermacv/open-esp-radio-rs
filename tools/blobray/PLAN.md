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
| 10 | active | Comparison relations: selected final RAM, ordered calls/reviewed argument pairs, RAM/branch timeline, ABI/layout projections and effect contracts. Explicit relation selects returns/memory/events; RAM-only and call-only differences are caught; invalid projections fail; all three verdicts and retained excluded observations are tested. |
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

Stages 00–09 and the corrective transfer-state obligation in 06.2 are complete.
The local expression index correction is active before continuation of 10.4.
Stages 10.1–10.3 are complete.
The corrections below preserve all original stage obligations and 10.4 WIP.
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
| 10.4 | active | Reviewed effect contracts classify required, omitted, replaced and added MMIO/delay/fence effects, preserving reason and claim ceiling. Absent/unclassified/conflicting rules fail closed; known effect differences DIFF and incomplete execution cannot MATCH. Policy identity, raw excluded evidence, provenance and source-free replay remain retained. Compose all stage-10 relations, run the full execution/comparison/recovery suites and source-only checkpoint; close stage 10 only after all four sub-stages pass. |


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
| Local expression index | active | Operation-owned full-key interning preserves IDs, records, provenance and admission rollback. Collision, cancellation, capacity/release and 512/1024/2048 work-growth regressions pass without raising limits. |
| Runtime interface preparation | ready | Group snapshot/object requests with admitted sorting and linear group passes; retain individual physical validation, result order and release before sessions. Work growth, cold/warm ownership, invalid selections and publication atomicity pass. Mixed callback/captured indirect calls retain the explicit strict-profile INCOMPLETE outcome and replay. |
| Documentation and full checkpoint | ready | One current-format reference and coherent execution position; focused/public/private docs, formatting, Clippy, all eight core package unit suites, functions/images/execution/architecture, standalone, source-only and authenticated PHY/ROM watchdog acceptance pass. Artifacts remain ignored; unavailable inputs/checks keep obligations open. |
