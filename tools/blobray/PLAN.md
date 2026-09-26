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
- Breaking formats are permitted until a stable persistence contract is
  established (backlog 21); unsupported versions must be rejected without
  mutation. Do not add converters, compatibility readers or old-reader bundles.
- New Blobray mechanisms are added only when a concrete claim about the
  production driver cannot be expressed without them (user decision 2026-09-25).

## Definition of done

Every product stage must have a complete application scenario and matching
frontend; explicit inputs/results, ownership/lifetime/errors and claim scope;
positive and meaningful negative regression tests; shared resource admission and
atomic publication; source-free reopening of retained results; current docs;
passing required checks; and a completed commit. Skipped checks are not passes.

Stages use `pending`, `ready`, `active`, `needs-replan`, `done`; former stages
moved outside the plan are `backlog`. A capability has
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
| 12 | done | Real PHY calibration/RF workflow, required intrinsics and reviewed summaries. Each declared calibration case has explicit conditions and expected outcome; coefficients/models/production changes alter dependency identity; reconstruction never impersonates captured execution. |
| 13 | done | Qualification reads typed-scenario verdicts. Each scenario package publishes a machine-readable evidence index of its retained executions: execution identities, reviewed effect-contract and projection identities, verdicts, claim ceilings and scope. `qualification/evaluator` and its targets, the `hil` runner tests and the `tools/xtask` vendor checks read this index instead of `vendor-project.toml`, `verification-addon.toml` and the legacy `project verify` index. Stale, unreviewed or incomplete evidence cannot satisfy a gate. A capability whose legacy evidence has no typed scenario yet is unclaimed, never inherited. |
| 20 | done | Legacy removal, right after stage 13. No consumer reads legacy formats: remove the old engine, launcher, `src/`, the legacy crates (`contracts`, legacy `backend-riscv`, `analysis-model`, `semantics`, `execution-model`), `addons`, ecosystem packs and catalogs, the legacy documents, the Next legacy capture adapter (`import-legacy`, `legacy`, storage table), the `verification/vendor/*/blobray-provider` crates with their tests, the chip/knowledge/esp32c5 provider data, the TOML profiles/dispositions/baselines, `vendor-project.toml` and `verification-addon.toml`. Legacy claims without a typed scenario stay unclaimed in qualification until stage 14 decides them. Workspace, CI, architecture checks and documentation no longer mention the removed parts. The evidence index derives its tool sources from the scenario package's Cargo path closure, and all targets validate with the regenerated index. |
| 24 | done | Lean verification engine (user decision 2026-09-25). A driver verification run executes only what its claims need: vendor and compiled production execution, comparison under typed contracts, and claim results. Contracts and projections are typed values reviewed through git; evidence is results only. Measured: the full run takes 3.0 s warm, against 231 s before. The interpreter throughput target moved to the backlog (24.3) and vendor result caching became stage 25.1 (user decision 2026-09-25). |
| 25 | done | Verification without rebuilds (user decision 2026-09-25). Executor semantics are checked against the official RISC-V architectural tests; vendor results are computed once and reused; sensitivity comes from the dependence of compared observations on executed production instructions, confirmed where needed by mutants that patch the production image in emulator memory, never from rebuilding binaries. No third-party runtime dependency is added. |
| 22 | done | Verification coverage and sensitivity (22.1–22.5 done). Wide-input leaves that neither cases nor exhaustive input enumeration can close decide whether an SMT solver is added; none remained when 22.5 closed, so no solver is added. Every typed scenario reports the vendor branch coverage of the root closures it exercises (basic blocks reached and both directions of each conditional branch), retained with its evidence and carried into the qualification index. Observation dependence (stage 25) reports the executed production PHY lines no compared observation depends on; unobserved lines and uncovered branches become explicit follow-up cases, relations that compare their effect, or recorded out-of-scope decisions, and point mutants confirm individual findings. |
| 14 | active | Remaining suites required by qualification targets or by production firmware paths, as typed scenarios on the stage-12 mechanisms. Each legacy TOML profile/disposition/baseline suite is triaged: ported with checked expected MATCH/DIFF/INCOMPLETE, or recorded as a dropped claim in qualification (user decision 2026-09-25). Compiled production paths are required; exclusions and claim strength are preserved. Split independent large groups into separately accepted sub-stages before activation. |
| 23 | pending | Hardware calibration cross-check. On the same ESP32-S31 board, HIL runs the vendor firmware calibration and the production driver calibration and compares the committed calibration state (DC rows, gain coefficients, temperature references, RFPLL results). Independent of Blobray peripheral models; differences beyond reviewed tolerances fail; evidence is dated. |

### Stage 01: existing checkpoint

| Former stage | Status | Delivered scenario and required acceptance |
| --- | --- | --- |
| 15 | backlog | Vendor revision snapshots/prepare-update/diff, symbol correspondence/lineage and reviewed rebase of assertions/boundaries. Rename is evidence, not acceptance; changed bodies/layouts/applicability invalidate dependencies; ambiguous mappings require decisions; A→B→reviewed rebase retains A. |
| 16 | backlog | Incremental reuse and bounded parallelism with complete dependency identities. Identical work reuses results; independent edits invalidate only dependents; missing-dependency regressions, serial/parallel equivalence, cancellation/capacity atomicity and measured work reuse are required. |
| 17 | backlog | Permanent storage: transitive retention roots/pins, reachability/reclaimable report, GC preview/apply and compaction. Review evidence/active readers survive; roots are revalidated at apply; interruption preserves committed projects; reclaimed bytes are verified separately from temporary quotas. |
| 18 | backlog | Pseudo-Rust and separate executable-reference consumer, single/batch generated/blocked manifest with provenance. Pseudocode is not executable evidence; generated supported code compiles and is tested; unsupported semantics block generation; no production substitution/qualification. |
| 19 | backlog | TUI over shared application/read APIs; completions/manpage and consistent diagnostics/progress/details. No frontend analysis/workflow duplication; partial/stale/conflicted states, empty/large streams, cancellation and errors are tested; help/examples/generated docs agree. |
| 21 | backlog | Stable persistence after stages 15–19: freeze a research fixture corpus and transitive persistence contract that subsequent versions must read without conversion. |
| 24.3 | backlog | Interpreter throughput of at least 100 million guest instructions per second, which needs an executor monomorphized over the session memory (application/backend boundary change); the measured 22 million per second keeps the full run at 3 s. |
| 22.M | backlog | Whole-crate mutation campaign of the production PHY. Needs mutant cost cut by one to two orders of magnitude first, for example reuse of unchanged vendor-side executions (with backlog 16) instead of rerunning every scenario. |

| Legacy leaf or leaves | Completion owner |
| --- | --- |
| project init; project inputs init | 01: native init/import |
| project configure | backlog: not required by qualification |
| project doctor; project files; project status | native `doctor` (files/status: backlog) |
| project cache stats; project cache gc; project cache compact | backlog 17 (existing logical usage checkpoint: 01) |
| project revision snapshot; project revision prepare-update; project revision diff; project revision rebase | backlog 15 |
| project research next; project audit bindings | backlog: not required by qualification |
| project browse | backlog 19 |
| project analyze | backlog (analysis components: 01–06; reuse: backlog 16) |
| project verify; project check | 13: typed-scenario evidence index |
| project publish | 05: independent register publication |
| advanced functions init-pack; advanced functions validate; advanced functions review | 04 |
| advanced code init-pack; advanced code validate; advanced code review | 02 |
| advanced code rebase | backlog 15 |
| advanced symbols inventory | 02 |
| advanced symbols correlate; advanced symbols lineage | backlog 15 |
| advanced interfaces discover; advanced interfaces init-pack; advanced interfaces validate | 03 |
| registers list; registers coverage; registers evidence | 05 |
| registers init-model; registers import-svd; registers validate; registers review | 05 |
| registers export-svd; registers generate-pac-raw; registers generate-pac-api; registers generate-bindings | 05: existing separate owner |
| inspect function; inspect flow; inspect object; inspect register | 04 (register catalog: 05) |
| inspect scope | backlog: not required by qualification |
| inspect analyze | 06 (local profile checkpoint: 01) |
| inspect trace; inspect compare | 06 |
| advanced mmio discover | 05 |
| advanced ir export; advanced ir build | 06 |
| advanced reference generate; advanced reference generate-batch | backlog 18 |
| advanced execute run; advanced execute replay | 09 (sessions: 07; models: 08) |
| advanced execute compare | 10 |
| advanced verify profiles; advanced verify source; advanced verify inventory; advanced verify evidence | 13: typed-scenario evidence index |
| advanced image audit-targets | 01: native audit |
| tooling completions; tooling manpage | backlog 19 |

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
| Vendor branch coverage and mutation sensitivity | 22 |
| Hardware calibration cross-check | 23 |
| Declared remaining radio suites/mechanisms | 14 (triaged) |
| Cross-revision correspondence and assertion applicability | backlog 15 |
| Dependency-aware incremental cache and bounded jobs | backlog 16 |
| Retention, reclaimability, pins and compaction | backlog 17 |
| Pseudo-Rust and executable reference generation | backlog 18 |
| Human/JSON diagnostics, details/progress/color and TUI | backlog 19 |

### Stage 02 refinement

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 02.1 | done | Range-local relocation effects: unaffected integer bytes decode; intersecting/unknown transformations remain explicitly unresolved; preserve all source relocations and initialization classification; boundary/width/unknown cases and review/export/reopen pass. |
| 02.2 | done | Exact dynamic-symbol occurrences: selected physical tables/indices are validated, data/functions use the correct table, duplicates and aliases do not merge, malformed/unsupported ELF is explicit; ordinary static-symbol scenarios remain correct. |
| 02.3 | done | Native symbol-less executable identities and reviewed boundaries across request/recipe/knowledge/planning/query/coverage. No synthetic SymbolId, no inference from neighboring symbols, no dual legacy resolver. Analyze/export/reopen retains exact ranges and explicit claim scope. |

### Stage 03 refinement

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 03.1 | done | Captured pointer tables: explicit layout and RV32 pointer profile, physical relocation target/addend or image address, null/external/ambiguous/unsupported distinctions. Discovery/data query → proposal → acceptance → export/reopen on synthetic and authenticated real bytes. Preserve initialization and relocation evidence; invalid/overlapping relocation extents, overflow, budget failure and unsupported encodings cannot yield a complete resolved table. |
| 03.2 | done | Bounded alternatives for addresses/call targets: stable finite join, operation-scoped admitted storage, cycles/convergence and explicit overflow-to-unknown/incomplete evidence. Read-only analyses and exported facts preserve all retained alternatives and provenance; no arbitrary target chosen. Real and synthetic callbacks plus budget/cancellation checks pass. |
| 03.3 | done | Native interface contracts and discovery: physical table/argument/address roots, access paths and slots, layout/ABI, guards, finite index domains and semantic bindings. Observation → proposal → review → query/export/reopen is one application workflow. Missing/rejected bindings, invalid guards/index domains, overlapping/conflicting layouts and unsupported ABI remain explicit; no model execution or hardware qualification is implied. |

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 03.3.1 | done | Native interface declaration → physical/pure validation → proposal/review → knowledge query/export/source-free reopening. Table/argument/address roots, explicit paths, layout/slot ABI, guards, finite index domains and semantic bindings have bounded typed contracts. Bad identities, invalid guards/domains, unsupported ABI and conflicting accepted layouts fail without publication. No declaration claims runtime guard satisfaction or executes a model. |
| 03.3.2 | done | Discover table/callback observations from captured data and saved function facts; match only explicitly selected accepted contracts. Export exact slot/path/evidence, bindings and guard applicability; missing/rejected/ambiguous/unsupported cases remain explicit. Synthetic and authenticated real observation→review→query/export/reopen scenarios pass without hidden analysis, legacy resolution or runtime-model execution. |

### Stage 14 partitions fixed before activation

These partitions list the legacy suites stage 14 triages: each is ported or
recorded as a dropped claim in qualification.

Each partition inherits the stage 14 criteria and completes independently. Suite
IDs below are the existing `verification-addon.toml` declarations, not inferred
new qualification scope. PHY suites belong to stages 11/12.

| Sub-stage | Status | Exact suites |
| --- | --- | --- |
| 14.1 | active | `libpp-interrupt`, `libpp-power-interrupt`, `wifi-ap-tsf-stop`, `wifi-ap-tsf-start`, `rom-sta-tsf-snapshot`, `libpp-tx-dma`, `libpp-rx-dma`, `libpp-tx-retry`, `wifi-interface-context`, `wifi-sta-ap-receive`, `wifi-sta-beacon-filter`, `ordinary-tx-ownership`, `tx-protection-control`. Ported as the `wifi-mac` typed scenario over the pinned `libpp.a` (exact register effects over four radio register fills and every production-admitted argument; claims in the index): `hal_mac_interrupt_get_event`/`clr_event`, `hal_pwr_interrupt_get_event`/`clr_event` (the clears add one reviewed ordering fence), `hal_disable_softap_tsf`, `hal_mac_tsf_reset` (fresh AP epoch, selector 0), the five `hal_mac_rx_*` leaves, `hal_mac_tx_set_cca`, `hal_mac_get_txq_in_trig_flow_state`, `hal_mac_is_txq_enabled`/`valid`, `hal_mac_set_txq_invalid`, `hal_mac_txq_disable` (both probes), `hal_mac_clr_txq_state` (ordinary completion, selector 2) and `hal_disable_sta_beacon_filter`. Observation now covers every ESP32-S31 hardware crate line, so these claims report their HAL/PAC lines. Also ported: `hal_mac_set_addr`/`set_bssid`, `hal_he_set_tx_protection`, `hal_he_disable_rts_threshold`, `hal_mac_tx_get_blockack` (output record compared), `hal_mac_tx_config_edca` (vendor objects built from the semantic words), ROM `hal_get_sta_tsf` (nullable outputs) and the publication prefix of `hal_mac_txq_enable` before `GetAccess` (Blobray now resolves symbol goals in process and compares a vendor prefix with a complete replacement; production adds two ordering fences). `hal_mac_tx_set_ppdu` compares over the reviewed ordinary-queue-zero HT40 MCS7 A-MPDU fixture: seeded ROM `pTxRx`, `s_phy_get_max_pwr` and `g_osi_funcs_p` data, the OSI `coex_pti_clamp` slot answered by a call model at an unmapped address, and `putchar` declared absent (Blobray `LinkRequest.absent`). Its closure's other rates, formats and HE paths (421 locations) stay untriaged until a builder derives the vendor PP objects from the canonical parameters, so further HT configurations can be compared and non-HT paths reviewed. `rcGetRate` compares up to its `rcGetSMPDURate` call over four retry-counter states of the normal 802.11g 54M schedule (the selected-rate byte of the shared descriptor image); its fixed-rate, other-schedule and null-context branches (20 locations) stay untriaged until the probe takes other schedules. Remaining, each needing a mechanism the `wifi-mac` scenario lacks: `lmacProcessAckTimeout` and `lmacProcessTxError` (stateful queue entries after vendor `wdev_data_init`/`lmacInit` setup phases, compared through retry-counter state and the `lmacRetryTxFrame` republication), `wifi_set_rx_policy` (a `libnet80211.a` root reading `g_ic`, whose production probe lives in the register probe image the session does not load) and `wDev_ProcessFiq` (the whole interrupt dispatcher, not yet triaged against production). Open outside these suites: the crystal-duty calibration-tone disable branch (`pac/src/phy/baseband.rs` `if !enabled`) is executed by no compared path. |
| 14.2 | pending | `ble-interrupt-prefix`, `ble-scheduler-table-prefix`, `btbb-v2-init-arg-one`, `ble-memory-list-selector-one`, `ble-phy-register-init`, `ble-memory-list-selector-two`, `ble-memory-list-selector-three` |
| 14.3 | pending | `ieee802154-btbb`, `ieee802154-zb`, `ieee802154-coex`, `coex-timer-control`, `coex-timer-set`, `coex-core` |

Stage 01 fixture correction: the originally planned `phy_bias_reg_set` callee
is a four-byte ROM tail trampoline. Current call composition does not expand
that transfer, so it cannot meet the composed-effect acceptance criterion. The
replacement root above has ordinary calls to two concrete, small ROM bodies;
no acceptance obligation was removed.


### Stage 04 refinement

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 04.1 | done | Physical function signature/context declaration → validate/propose/review → query/export/source-free reopening. Share call ABI vocabulary with interfaces. Argument/return roles, context extents/fields/access roles and explicit preconditions remain conditional claims. Bad selectors, field bounds/types, contradictory preconditions and conflicting accepted contracts fail without publication; symbol/range, ordinary/thin/image and resource regressions pass. |
| 04.2 | done | Shared saved-research navigation for functions/callers/callees, data-object readers/writers and context-field accesses. Explicit selected analyses/publications/knowledge, bounded operation indexes, physical selectors and evidence references; ambiguous/unresolved targets remain visible, cycles terminate, no hidden analysis. CLI/API/export/reopen agree on synthetic and real queries. |
| 04.3 | done | Flow/effect slices and reviewed paths/event routes: selector delivery, static callback registration/delivery and broker subscription with exact participants, sites, fields/selectors and evidence. Structural paths, conditional reviewed routes and executable evidence remain distinct. Missing/ambiguous/mismatched steps, cycles and bounds cannot claim completion; complete query/review/export/reopen scenarios close the remaining stage 04 obligations. Runtime replay remains a later execution consumer, never synthesized by navigation. |

### Stage 04.3 executable partitions

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 04.3.1 | done | Exact saved root → selected function target or reachable effect inventory, with bounded call graph, evidence hops, ambiguous/missing/partial frontiers and explicit structural scope. Native ordered path proposal/review revalidates every exact saved step, rejects missing/mismatched/ambiguous steps and preserves roots. CLI/API/query/export/source-free reopening, cycles/diamond/depth/resource cases and real linked target/effects pass. |
| 04.3.2 | done | RAM definitions reaching an exact saved call/store publication anchor: incoming, must/alternative/candidate last writes; partial overlap, unknown alias and call clobbers remain explicit. Iterative bounded CFG/dataflow, local witnesses and values; joins, loops, killed definitions, unknown calls, resource failures and source-free query/export pass. It does not invent interprocedural memory effects. |
| 04.3.3 | done | Native reviewed selector delivery, static callback registration/delivery and broker subscription routes with exact participants, sites, object/queue/domain/selector fields, callback identity, case handler and optional terminal. Query authenticates physical evidence and reports condition/lifetime/order blockers separately; review never proves actual asynchronous delivery. Positive/negative/ambiguous/source-free review/query/export cases for all three close stage 04; runtime replay remains stages 07–10. |

### Stage 06 execution boundaries

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 06.1 | done | Native configured semantic IR build from explicit saved publications/analyses: named profiles, all/prefix/exact roots and optional resolved call closure; original function facts, exact/ambiguous links, coverage and transitive saved provenance. One supervised build owns memory/work/disk, stages and atomically publishes one immutable bundle. API/CLI show/export, duplicate/missing profiles, ambiguity/cycles, capacity/cancellation, source removal and restore pass. No hidden binary analysis, second semantic engine or legacy adapter. |
| 06.2 | done | Static observable trace extraction/comparison over saved IR with explicit physical observation scope. Ordered exact traces, symbolic/may-effects and concrete execution remain distinct. Unknown branches/addresses/calls, loops and unsupported effects retain blockers and cannot MATCH; exact equal/different cases yield MATCH/DIFF. Trace evidence/conditions/provenance and exports survive source removal/restore; real saved scope plus full source-only checkpoint close stage 06. |

### Stage 07 execution boundaries

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 07.1 | done | Explicit known/unknown RV32 integer argument words, register/stack ABI placement and bounded stack capacity. Shared request/backend/memory contracts, API/CLI execution/comparison/replay, source-free restore, more than eight arguments, unknown consumption and malformed/resource failures pass. No implicit zero arguments or stack initialization. |
| 07.2 | done | RV32 LR/SC/AMO through an explicit atomic memory port with owned reservation state. Single-hart ordering, reservation invalidation, unknown/unaligned/unsupported locations and all supported operations are tested; no nonatomic fallback or invented peripheral behavior. |
| 07.3 | done | Multiple exact entry/setup phases, explicit region ownership/persistence and cold/warm reset transitions. One application budget and publication; dependent-phase blocking, isolated/stateful data and exact source-free replay pass. |
| 07.4 | done | Explicit return, reach-symbol and observe-call completion goals over physical captured identities. Goals, premature returns, unresolved targets and phase failure have distinct evidence; API/CLI/comparison/replay and negative cases close stage 07 without claiming unobserved completion. |

### Stage 08 execution boundaries

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 08.1 | done | Native register-bank, constant/sequence read, W1C, read-clear, self-clearing, FIFO and indexed-bank mechanisms with explicit identity/applicability and phase/session lifetime. One supervised execution owns state, resource admission and model participation/closure evidence. Missing/extra/mismatched accesses, exhausted/unconsumed sequences, invalid geometry/overlap and cold/warm closure are tested; unmet model obligations cannot MATCH even if the entry returned. All mechanisms work through API/CLI/query/replay/source-free restore, with code outcomes distinct from model coverage. |
| 08.2 | done | Explicit external-call return words, private-stack/normal-memory outputs, bounded allocation and delay events. Exact selected bindings, ABI clobbers/stack words, consumed responses, modeled/code boundaries and provenance are checked. No implicit return, MMIO output fallback or hidden allocator. Unknown pointers, wrong ownership, capacity/exhaustion and unconsumed obligations remain explicit. A composed call/device/phase scenario, all mechanisms' positive/negative regressions and source-free restore/replay close stage 08. |

### Stage 09 runtime interfaces and services

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 09.1 | done | Explicit selected accepted interface contract → runtime table/pointer placement → captured or explicitly modeled callback execution → retained lifecycle and source-free replay. Resolve physical roots and bounded paths, validate layout/ABI/index/guard conditions and exact slots; null/missing/ambiguous targets remain distinct. Track initialization, pointer installation, writes and indirect-target associations with bounded operation-owned indexes; association never claims unsupported pointer provenance. Wrong review/source/layout/guards, overlapping ownership, unknown/partial writes, alias targets, cold/warm state and resources fail explicitly. All root/path forms admitted by the reviewed contract retain an implementation or a named unmet criterion; no silent supported-profile reduction. API/CLI/reopen agree. |
| 09.2 | done | Stateful FIFO enqueue/dequeue/length through explicitly selected reviewed slot bindings, with argument/private-stack input, output and wake behavior. Bounded isolated queues persist by declared lifetime; full/empty/order/wrong handle/width and cross-phase failures are checked. Observe-dequeue service goals stop only after the selected successful event; failed phases never imply completion. A reviewed table → service → event-goal scenario, API/CLI/source-free restore/replay, resource/cancellation/closure and all positive/negative mechanisms close stage 09. |

### Stage 10: comparison checkpoints

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.1 | done | Explicit per-case comparison selection and bounded final normal-memory observations, alongside selected return words and existing ordered MMIO/fence/delay events. Physical paired ranges have exact lengths and identities; all selected bytes, including unchanged bytes and unknowns, are represented. RAM-only and high/low-return differences DIFF; equal known completed selections MATCH; unknown/unavailable/unfinished results remain INCOMPLETE. Excluded observations remain retained. Invalid/overlapping/overflowing selections fail, phase lifetime/resource/cancellation atomicity and source-free replay pass. No inferred layout equivalence. |
| 10.2 | done | Ordered captured-code, modeled and service call observations with physical targets, ABI words and exact interleaving with selected observables. Explicit reviewed semantic call pairs support exact/selected/ignored argument policies; unlisted calls remain retained and their exclusion is visible. Missing/ambiguous/unreviewed pairs fail; reordered/missing calls and selected argument-only differences are caught; unknown words stay incomplete. Shared application API/CLI and source-free retained comparison pass. |
| 10.3 | done | Ordinary RAM access/atomic and branch timeline observations plus explicit reviewed ABI/layout projections for corresponding memory/arguments and control observations. Validate exact domains, widths, aliases/overlap, offsets and applicability; no dropped unknown/missing fields or automatic pointer normalization. Final-state-only equivalence stays distinct from ordered internal-state equivalence. Different layouts can match only under the selected valid projection; invalid/stale projection and branch/RAM-order changes are tested. |
| 10.4 | done | Reviewed effect contracts classify required, omitted, replaced and added MMIO/delay/fence effects, preserving reason and claim ceiling. Absent/unclassified/conflicting rules fail closed; known effect differences DIFF and incomplete execution cannot MATCH. Policy identity, raw excluded evidence, provenance and source-free replay remain retained. Compose all stage-10 relations, run the full execution/comparison/recovery suites and source-only checkpoint; close stage 10 only after all four sub-stages pass. |

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.2.1 | done | Explicit bounded physical call/tail-transfer capture, known/unknown register and selected stack ABI words, exact target comparison and interleaving with selected MMIO/fence/delay. Captured code, call models and FIFO service boundaries are represented; observe-call goals retain their pre-dispatch semantics. Site/transfer provenance and excluded observations remain available. Target/order/argument-only differences, unknowns, invalid capture, resource/cancellation atomicity, API/CLI/source-free replay and all gates pass. No semantic pair is inferred. |
| 10.2.2 | done | Native proposal/review of semantic call correspondence anchored to exact captured occurrences or explicit modeled binding identity. Explicit exact/selected/ignored word policies and selected pair scope; no name fallback or implicit ABI projection. Invalid, unaccepted, ambiguous, mismatched/stale pairs fail; all listed and unlisted calls remain provenance. Ordered reviewed comparison, generic/specialized API/CLI, source-free query/replay, resource/atomicity checks and all gates close 10.2. |

| Sub-stage | Status | Acceptance |
| --- | --- | --- |
| 10.3.1 | done | Explicit bounded normal-memory read/write/atomic and conditional-branch observations, with exact ordered physical comparison interleaved with selected calls/MMIO/fence/delay. Include declared model/service normal-memory effects once; setup/inspection reads are provenance rather than invented guest transactions. Capture/compare selections are explicit; unknown/inaccessible/unfinished states cannot MATCH, excluded evidence stays available, and equal final state cannot hide a selected timeline difference. All widths, LR/SC/RMW, branch choices, model effects, phase ownership, resource/cancellation atomicity, retained validation and source-free API/CLI restore/replay pass with all gates. |
| 10.3.2 | done | Native reviewed ABI/layout projections for corresponding final-memory fields, call words and internal memory/control observations. Exact endpoint domains/widths/offsets/aliases/applicability and every selected field are validated; no implicit pointer normalization, dropped unknown/missing field, unreviewed mapping or name fallback. Different layouts/ABI word positions can match only under the selected valid projection; reordered RAM/branches and known projected differences remain DIFF. Generic/specialized API/CLI, immutable review/source/definition retention, invalid/stale/conflicting mappings, resource atomicity and source-free replay pass. Compose physical and projected relations, then close 10.3. |

### Corrective checkpoint before continuing 10.4

| Correction | Status | Required acceptance |
| --- | --- | --- |
| 06.2 transfer state | done | Keep pre-transfer CallInputs; apply saved typed link writes on callee entry under trace policy 2. User-authorized exact saved indirect links may close only structural indirect CFG gaps; all other gaps remain blocking. ELF-to-IR-to-trace and concrete execution agree for x1/x5, 2/4-byte calls, tails, nested/repeated calls and selected effects. Section-relative addresses never become physical offsets; malformed facts fail closed. Source-free restore and existing real PHY/ROM trace acceptance pass before reclosing 06.2. |
| Local expression index | done | Operation-owned full-key interning preserves IDs, records, provenance and admission rollback. Collision, cancellation, capacity/release and 512/1024/2048 work-growth regressions pass without raising limits. |
| Runtime interface preparation | done | Group snapshot/object requests with admitted sorting and linear group passes; retain individual physical validation, result order and release before sessions. Work growth, cold/warm ownership, invalid selections and publication atomicity pass. Mixed callback/captured indirect calls retain the explicit strict-profile INCOMPLETE outcome and replay. |
| Documentation and full checkpoint | done | One current-format reference and coherent execution position; focused/public/private docs, formatting, Clippy, all eight core package unit suites, functions/images/execution/architecture, standalone, source-only and authenticated PHY/ROM watchdog acceptance pass. Artifacts remain ignored; unavailable inputs/checks keep obligations open. |

### Review corrections for completed stages 06 and 09

| Correction | Status | Required acceptance |
| --- | --- | --- |
| Static trace prefixes | done | Known differences in observed prefixes yield DIFF despite later blockers. Length differences require a completed shorter side; symbolic uncertainty alone cannot prove DIFF and incomplete paths cannot MATCH. Summary validation and trace policy agree. ELF-to-IR API/CLI, empty/unequal prefixes, symbolic values, source removal and backup/restore pass. |
| Retained interface validation | done | Store groups selected tables by frozen snapshot with admitted O(n log n) sorting and linear group traversal, retaining every per-declaration check. Grouping, publication and reopening work-growth tests, one history load per selected snapshot, cancellation/capacity release, invalid late entries and source-free replay pass without raising budgets. |
| Review correction acceptance | done | Both corrections pass core/Next tests, strict Clippy, formatting, owned public/private docs, standalone, authenticated PHY/ROM watchdog trace/restore and the final source-only checkpoint. Integration with the selected parallel snapshot is checked before closure; missing inputs or gates remain open obligations. |

### Stage 11: authenticated PHY I2C execution

| Sub-stage | Status | Required acceptance |
| --- | --- | --- |
| 11.1 | done | Authenticate archive/ROM and exact PHY object/table; link the real command-memory root with captured ROM callees. Execute all 45 command-RAM stores and descriptor/no-op leaves against freshly compiled production on zero, mixed and boundary parameters, with independent expected RAM/MMIO values. Retain source/probe identities, explicit ABI/layout/observation scope, known differences and unknown/resource outcomes. Source removal, move, backup/restore and replay reproduce records. Native API/CLI, focused regressions, documentation and required gates pass. |
| 11.2 | done | Bounded explicit packed-command peripheral responses for both hosts over a shared seeded analog bank, retained/scripted reads, completion and pending/busy ownership. Model identity and unused/pending obligations survive warm/cold lifetimes. Unknown registers/commands, widths, overwrites, exhausted samples, cancellation and capacity fail closed. Captured host-selection/read/write/masked/reset paths execute against compiled production under explicit observation scopes, including immediate/delayed completion and timeout cases with independently checked MATCH/DIFF/INCOMPLETE expectations. No peripheral model substitutes for code or RF/calibration algorithms. Native API/CLI, retained evidence, source-free restore/replay, real acceptance and gates close stage 11. |

### Stage 12: calibration/RF acceptance units

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
| 12.9 | done | Combined calibration and tracking parents: `phy_rfpll/combined.rs`, `phy_rfpll/parent.rs`, `phy_rfpll/graph.rs`. Execute real children, guards/grant order, channel 13/HT40, client/thermal domains, RFPLL disabled/enabled and signed corrections. Failed TX preserves pre-calibration state and earlier completed power/RFPLL state. Modeled child completions cannot satisfy complete-parent acceptance. |
| 12.10 | done | Combined practical-PHY checkpoint: all units and stage-11 scenarios work together under native identities, shared budgets and preservation. Complete required intrinsic/reviewed-summary coverage with direct semantic/unknown/resource tests and explicit applicability; changing model, summary or production invalidates identity. No summary impersonates executed capture. All relevant integration suites, standalone, formatting, strict Clippy, owned public/private docs and the repository CI checks pass before closing stage 12. |

### Checkpoint 12.R: typed Rust verification scenarios

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.R.1 | done | Host-owned wire types for record, run and inventory documents, used by the renderer and checked against CLI output. Scenario package, process runner, capture/import, probe catalog through `oer-probe-codegen` types, ABI lowering and request builders over domain types; `cargo test` covers the harness and gain oracles. `xtask` command. Gain arithmetic/publication and gain state/RF-test run in Rust with the Python case counts, verdicts, negatives, unmet-obligation exit and preservation; `phy_gain*.py` and their tests are removed. |
| 12.R.2 | done | I2C command memory and transport, harness call edges, calibration leaves/prefix and RFPLL runners in Rust with unchanged cases and outcomes; the corresponding Python is removed. |
| 12.R.3 | done | PHY research scenario (analysis, interfaces, IR/trace, navigation, flow, registers, memory slices, data/knowledge review and preservation) in Rust; `harness.py` and every remaining Python file are removed; owner docs, Blobray references and full checks close 12.R. |

### Checkpoint 12.P: execution performance

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.P | done | Resolve execution inputs and prepared images once per request and reuse them across cases, sides and phases. Find an input payload without walking the whole inventory. Remove redundant fixed costs of CLI operations: evidence returned by execute/compare/replay where a scenario would read it back, request size that forces tiny batches, and supervisor sampling overhead. Retained evidence, identities, verdicts and every negative/resource/cancellation contract stay unchanged or change only through a new rejected-without-conversion format. The authenticated gain scenario with `--rftest` completes in at most 30 seconds with the same cases, verdicts, negatives and per-execution restore/replay. Regressions show that inputs are prepared once per request; the I2C scenario and all Next suites pass. |

### Checkpoint 12.M: mechanisms instead of handwritten knowledge

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.M.1 | done | A retained MMIO aperture is a Blobray device: every aligned access in a declared range not claimed by an exact model is retained storage with a declared initial word, and every access stays an MMIO event. Scenarios declare the radio block once and model only semantic inputs; retained-register enumerations are removed. Both sides of a comparison must read the same never-written registers. Remaining addresses (inputs and independent expectations) are single named constants, shared ones in one owner; the published SVD is not a scenario dependency, because parts of it are recovered from the same vendor analysis. ROM storage constants name their ROM symbols and are checked against the captured ROM inventory. |
| 12.M.2 | done | A call declaration answers either a finite ordered response list or, for a single pure response without memory outputs or allocations, every call. Call counts and every observed argument and delay stay evidence. Execution schema changes without conversion. The per-phase event bound rises to 2^20, so a bounded 100,000-operation poll loop is observable without truncation. Scenario delay models use it, and exact per-profile delay budgets are removed; independent delay-sequence expectations stay. |
| 12.M.3 | done | A supervised query proposes ROM companions for a link request: a trial link reports every unresolved name, each resolved to exactly one defined function or data object in explicitly ordered candidate inputs, or reported unresolved. The retained link request still lists every exact companion. Companions may be ROM data objects without a load mapping. Scenario companion lists are replaced by proposals. |

| Scenario | Time |
| --- | --- |
| gain with RF-test | 52 s |
| i2c with both SDK inputs | 76 s |
| channel | 53 s |
| research | 86 s |

| Scenario | Before | After |
| --- | --- | --- |
| rx-gain | 149 s | 25 s |
| gain | 52 s | 13 s |
| i2c | 76 s | 32 s |
| channel | 53 s | 20 s |
| research | 86 s | 82 s |

| Scenario | Time |
| --- | --- |
| gain | 12 s |
| i2c | 30 s |
| channel | 19 s |
| rx-gain | 19 s |
| research | 83 s |

### Checkpoint 12.S: revision lookup index

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.S.1 | done | Scoped revision reads use a per-revision object index: each object's record span in the authenticated manifest. A scoped read decodes the header, the selected input and the selected object only, verifies the selected capture, and presents the same records to the sink as the full walk. The index is derived, never trusted: every decoded span must carry the requested identity or the read fails as an integrity error, and a missing or unusable index is rebuilt from a full validated walk. Occurrence validation, scoped inspection and knowledge proposal/review use it; full-revision queries keep the full walk. Tests cover equality with the filtered full walk, forged spans, rebuild and budget accounting. A reviewed-contract proposal on captured input symbols costs a small fraction of a full walk, and the I2C scenario returns to its pre-12.N.4 duration or better. |

### Stage 25: verification without rebuilds

No third-party runtime dependency is added. The RISC-V architectural tests are
input data: assembly tests with reference signatures from the Sail formal model.

| Unit | Status | Acceptance |
| --- | --- | --- |
| 25.0 | needs-replan | ISA conformance: the RV32 I, M, A and C suites of the official RISC-V architectural tests, built once with the existing RISC-V toolchain into ignored outputs, run through the Blobray executor, and every signature equals its reference. Tests that need privileged state the executor does not model are listed with the reason; a mismatch is an executor defect fixed before 25.1. Done for I (38), M (8) and C (26 of 27) of riscv-arch-test 2.7.4: `next/tests/riscv_conformance.rs` passes 72 tests; `cebreak-01` (machine-mode breakpoint trap) and `Fencei` (stores into its own code, which image loading rejects) are excluded and must keep failing, so Zifencei (its only test) is not covered. Unmet: the A suite. It exists only in 3.x and later releases, which ship no reference signatures; references require running the Sail model. The user chose to record this obligation instead of adding Sail now. |
| 25.1 | done | Vendor results are computed once and reused in memory by comparisons of the same vendor side within one process, such as one request over many patched production images; only production reruns. Records and verdicts equal a full execution byte for byte; results of another vendor side are rejected, and a replacement case that blocks the vendor side forces full execution. `in_process::vendor` and `verify(vendor_results)`; regression test `reused_vendor_results_yield_the_records_of_a_full_execution`. Measured on the full scenario run: the vendor side is 86M of 148M guest instructions. A disk cache across runs (user decision 2026-09-25 to drop it) needed 353 MB of serialized results and about 1 s to read them, slowing the warm run from about 3.0 s to 3.5 s, and a scope for the calling build; it was removed. |
| 25.2 | superseded | Cancelled by user decision 2026-09-25 after measurement: on the full scenario run, cases whose cold-to-warm chain repeats an earlier chain of the same request are 4,408 of 8,321 cases but execute only 1.9M of 148.7M guest instructions (1.3%), so copy-on-write prefix snapshots would add session complexity for no material gain. Any prefix reuse for mutants is decided in 25.3 from its own measurement. |
| 25.3 | done | Observation dependence (user decision 2026-09-25, after measuring that one byte-patch mutant per executed production instruction costs about 410G guest instructions over the reaching case chains). In-process verification can report, per request, which executed replacement instructions reach an observation its relations compare. On request only, the session records a replacement step log (instruction, memory accesses, events, modeled-call results); the executor is unchanged. After execution, dependence follows ISA-neutral semantics: data dependence through registers and memory bytes, and control dependence on a conditional branch until its immediate post-dominator in the statically recovered CFG of the executing function (an unresolved transfer extends the region to the function exit). Sinks are exactly the replacement observations each case's relation compares (returns, selected event channels, selected final memory, calls, projected fields), enumerated by the comparison owner. Synthetic tests cover register, memory, control and call dependence and each sink kind, including an instruction that is executed but unobserved. The full scenario run with dependence is measured and stays within a small multiple of the plain run. Implemented as `in_process::verify` with `dependence` (`execution_steps`, `dependence`, `compared_observations`); regression tests in `next/tests/execution/dependence.rs`. Measured on the full scenario run with dependence enabled in every request, under the same load: 5.9 s instead of 3.8 s wall, tracking 4.8 s instead of 2.8 s; about 23,800 of 31,000 executed production instructions (per-scenario sums) are observed. |
| 25.4 | done | Unobserved executed production PHY instructions map to source lines through debug information. Index entries carry the observation summary (executed, observed, reviewed). Each unobserved PHY line needs a reviewed decision in git; a decision whose line is observed or no longer executed fails the run. The source mutator, its campaign and its worktrees are removed; documentation and the evaluator follow. Implemented in the scenarios' `observation.rs` and evidence index schema 4 (`observation` per entry, top-level `unobserved`); qualification validates the counts. A line another claim observes can be unobserved within one entry, so only lines no scenario observes are listed. Measured on the full run: 1,882 of 2,062 executed production PHY lines observed, 180 untriaged (22.5 obligations); warm run about 7 s. `vendor-scenario mutants`, `--reach`, `mutation.rs`, `mutation_campaign.rs` and the 12 GB of mutation worktrees are removed. |
| 25.5 | done | Point mutants: in-process verification applies byte patches `(address, original bytes, replacement bytes)` to the loaded replacement image; a patch whose original bytes differ is invalid. Used on named addresses to confirm a dependence finding, never as a campaign; no binary is rebuilt. Implemented as `ImagePatch` in `InProcessComparison` (applied to each replacement session after loading) and `vendor-scenario --patch`, which checks each patch against the probe ELF first; regression test `point_mutants_patch_the_loaded_replacement_image`. Checked on the full run: dropping a store on unobserved `analog/dcode.rs:131` survives, changing an immediate on observed `analog/dcode.rs:119` is killed by tracking in 2.9 s, and a patch with wrong original bytes is rejected before running. |

### Stage 24: lean verification units

The baseline full run takes about 231 s. About 53 s of that is comparison. The rest is replay (60 s), preservation (38 s), repeated image setup (35 s), knowledge reviews (16 s), and evidence and coverage reads (26 s). Blobray operations spend 30 s starting processes, 19 s revalidating inputs and 33 s retaining evidence, against 56 s executing. The interpreter runs about 5 million guest instructions per second over 146 million instructions.

| Unit | Status | Acceptance |
| --- | --- | --- |
| 24.1 | done | In-process verification: scenarios call Blobray execution and comparison as a library over loaded segments, typed contracts, projections and resolved goals, without a project, CAS, journal or CLI per operation. Private inputs are authenticated once per run. Per-run replay, backup/restore/move and knowledge reviews leave the scenarios; Blobray synthetic tests keep covering those properties. Every claim, negative case and expected verdict is unchanged. Measured: the full run takes 49 s instead of 231 s; each scenario now spends about 5 s on image setup and the rest executing. |
| 24.2 | done | Setup results (capture, inventory, probe catalog, linked images and data exports) are memoized below the scenario output, keyed by the Blobray executable, the authenticated input bytes and the operation request; a warm run creates no Blobray project, and a miss creates it lazily and checks its revision. Measured: 51 s cold, 14.3 s warm for the full run. |
| 24.3 | backlog | Interpreter throughput: see the backlog entry. Delivered here: a direct-mapped decode cache keyed by instruction bytes, last-region lookup, one-lookup instruction fetch and 256-instruction accounting intervals; the full warm run went from 14.3 s to about 9 s at about 22 million guest instructions per second. |
| 24.4 | done | Independent work runs in parallel with deterministic result order. The scenarios of a full run share no state and run concurrently, joining their results in the declared order; any failure fails the run. Measured: the full warm run takes 3.0 s, bounded by the tracking scenario. Parallel cold case chains inside one request would shorten that critical path further and are not needed for the stage target. |
| 24.5 | done | Superseded by stage 25.1, which caches vendor results as the basis of production-only reruns and mutation runs. |
| 24.6 | done | Evidence index schema 3 carries claim results, contract and projection hashes, coverage and input/source digests, without retained execution identities; the qualification evaluator and documentation follow. Measured: the full run takes 3.0 s warm, against 231 s before stage 24; a run after a Blobray rebuild relinks its images once. |

### Stage 22: coverage and sensitivity units

| Unit | Status | Acceptance |
| --- | --- | --- |
| 22.1 | done | Every execution retains, per side, the instructions and conditional-branch directions it reached in executable captured segments, accumulated across cold resets and independent of the selected timeline (execution schema 21). The store validator requires one ordered, self-consistent record per side after the last case. |
| 22.2 | done | A read query reports the vendor coverage of the root closures an execution exercises: from each distinct vendor entry, the statically reachable code through direct branches, jumps and calls, excluding callees answered by declared call models; basic blocks reached and both directions of each conditional branch, with uncovered blocks and directions, modeled callees and unresolved indirect transfers named by function and offset. Several executions of one vendor target combine. Synthetic tests cover closure construction, modeled and indirect boundaries, union and rejection of mixed targets. |
| 22.3 | done | Every typed scenario reports the coverage of its claimed roots; index entries carry blocks and directions reached out of all, with the excluded and untriaged counts, and the index lists every untriaged location by function, offset and kind (index schema 2). The qualification evaluator validates that coverage accounts for every uncovered location. Reviewed decisions in the scenario package exclude locations with a reason; a decision that no longer excludes anything fails its scenario. Runtime helpers are excluded as whole functions; the remaining locations stay untriaged and explicit (user decision 2026-09-25: triage after the mutation run). |
| 22.4 | done | Targeted mutation run: `vendor-scenario mutants --target FILE[:START-END]` mutates executed production PHY lines of the named regions (constants, comparisons, guards, min/max bounds, adjacent register-write order), rebuilds the probe incrementally in per-worker worktrees and runs the scenarios reaching each mutant; results are journaled per commit. A whole-crate campaign costs hours and moved to the backlog (user decision 2026-09-25). A partial whole-crate run killed 94 of 129 mutants; its 26 survivors are 22.5 obligations. The source mutator was removed in 25.4. |
| 22.5 | done | Triage every untriaged location of the index, starting with functions that have a production counterpart: a follow-up case that reaches it with an expected verdict, or a reviewed decision with its reason. Production-implemented vendor paths (for example the 802.11p configuration) are cases, not exclusions; vendor configurations production rejects fail-closed (the channel-14 MIC option) take reviewed exclusions, which may name an offset range of a function whose other paths are compared. Each untriaged unobserved production PHY line of the index (180 when 25.4 landed; 97 after dependence follows projected final state and unconditional transfers, all reaching nothing: statistics counters of the RX-gain and TX-DC executors, capability references the probes bind to no-op or zero-sized implementations, transition bookkeeping, and computations whose results no executed case uses, such as the non-converged baseband RX-DC corrections with `gain_index > 1`, the RFPLL SDM bytes other than the third, and the Bluetooth TX-DC path force-test) becomes a case or relation whose compared observations depend on it, or a reviewed decision in `observation.rs`. Tracking now compares committed state: the eight DCODE codes at `phy_param[0x1a1..]` (ROM `phy_dcode_cal_init`), the RX-gain DC and table completion bits 0x80 and 0x200 of the status word at 0xa4 (`phy_set_rx_gain_table`) and the shared and Wi-Fi RX-gain table last indices at 288-289, seeded equally on both sides. The tracking progress word at 0x1fe (RFPLL 0x1, Wi-Fi and Bluetooth TX power 0x2/0x4, common and transmit calibration 0x8/0x10) is compared too, now that the production tracking outcome reports the same progress (user decision 2026-09-26). Unprojected vendor state is reported mechanically: Blobray reports the persistent bytes each compared case writes (`observe_timeline.written`), each claim counts the bytes its relations compare through projection fields, memory pairs or the write timeline, and the index (schema 5) lists the rest that no claim-scoped decision in `state.rs` reviews. The first report found 29 untriaged bytes. The temperature-sensor index at `phy_param[0x16]` is now compared by the channel, temperature, combined and parent claims, and the RFPLL thermal claim compares the reference temperature and progress it commits through a projection instead of host assertions. Reviewed: the `s_track_result` debug copy, the estimate-scoped activity count at 0x1ac, the unchanged upper half of the status word, the RF-test-only bandwidth flag at 0x11e, the RFPLL reentrancy guard, and commits a claim's production child does not own that another claim compares. Production state is compared, not waived (user decision 2026-09-25): the probes keep `PhyState` in session memory and relations compare its fields with the vendor PHY globals through reviewed layout projections where a vendor counterpart exists; statistics without a vendor counterpart and prologue lines take reviewed decisions by category. Each open survivor of the removed source-mutant run below becomes, when its line is unobserved, part of that list, and otherwise a 25.5 point mutant that a case kills or a reviewed decision. Reviewed in `observation.rs`: executor statistics, failure payloads, capability references the probes bind to no-op implementations, the power sentinel of an exhausted RX-DC minimum search (every consumer thresholds below the power that exhausts it) and the initial correction of the event-driven RX-DC host model. The RX-gain calibration matrix adds a periodic estimator stream (samples in whole estimate units, a period odd against the baseband low/high read pair), so corrections, the `gain_index > 1` saturation and the shared-bank low-estimate correction reach the published DC codes; the direct RX-DC executor no longer writes back host-model fields after its terminal step. Table-selected RFPLL channels no longer compute an SDM image they never write, and channel selection no longer carries the 802.11p bytes, whose vendor step only writes them back; the channel matrix starts both sides with 802.11p enabled and compares the committed bytes at `phy_param[0x28..0x2a]`. The index lists only vendor locations that no claim whose closure contains their function reaches (user decision 2026-09-26): 185, down from 342 listed as the union over claims. The TX-DC matrix adds a SAR stream whose period is odd against the precheck pair, so a component's precheck count restarts observably; the per-component resets of scan state that `begin_scan` repeats are removed. New reviewed categories cover the calibration-tracking action payloads that the dispatch builds and discards (instruction-level dependence shows `commit` observes its own inlined outcome), the RFPLL correction report (the counterpart of the vendor's optional diagnostic `phy_printf`), temperature acquisition provenance and async future bookkeeping attributed to signatures and closing braces. The tracking probes' snapshot read of `parameter_002` and the TX-power child's unread diagnostic selectors are reviewed too. The channel matrix adds MHz requests, including one between channel centers, which reach `phy_mhz2ieee`; the direct RFPLL programming chain reachable only for a frequency outside the channel table, and the 2484-MHz and 5-GHz conversions, are excluded because production rejects those requests fail-closed. The RX-gain matrix adds readiness activity admitted by a detected RX saturation; it found two production deviations, now fixed: an RX-DC minimum search that admits no estimate returns the I/Q values its caller's slot holds (ROM `phy_pbus_rx_dco_cal_1step_new` keeps one slot for the baseband low search and one for the radio or high search across iterations), and the reference searches honor the detected saturation. Activity without saturation is characterized as a DIFF, not claimed: the ROM never initializes the radio slot, so its first unadmitted radio search returns stack contents, which production replaces with a cleared slot (user decision 2026-09-26); the exhausted-search locations only it reaches, and the nonzero estimator mode no claimed caller passes, are excluded. Readiness waits also poll once without activity. Independent I, Q and power estimator streams found a third deviation, now fixed: the ROM publishes the codes forced at the start of each iteration, so an unconverged calibration's final correction is never published. Coverage decisions are checked for staleness over the complete run, since every scenario shares them. RX gain table generation over the fixed tables `phy_set_rx_gain_table` builds, and its diagnostic print, are excluded. The TX-DC diagnostic print, the teardown no claimed caller skips and detector-ready poll repetition are excluded. The channel matrix adds other bandwidth request bytes; diagnostic prints, busy-poll repetition, unreachable temperature clamps and argument values only unclaimed callers pass are excluded. Constant arguments only unclaimed callers pass (linear-to-dB scale 3, PBus read selectors above 4) are excluded. Parent tracking cases start from each retained non-nominal I2C band, held or left for nominal. Parent cases also run with Bluetooth/802.15.4 TX-power tracking disabled, and the TX-power tracking diagnostic print is excluded. The RFPLL tracking and correction diagnostic prints, the Wi-Fi TX gain print, software-frequency poll repetition, sensor DACs outside the calibrated windows (production rejects them), the TX gain publication skip option production fixes disabled, functions reachable only through excluded paths, RX gain memory writes over the fixed-input tables and TX baseband indices no claimed caller passes are excluded. The RFPLL correction reentrancy guard and read-mask defaults for absent I2C blocks are excluded. The TX gain table walk's step limit and the channel-register argument only `phy_wakeup_init` passes are excluded. The Wi-Fi publication cases use every baseband gain `phy_index_to_txbbgain` encodes, including indices 3 and 4, whose seed halfwords the publisher reads from the calculated bytes after the seed words, as production models. Initial fields of freshly built tracking children that are written before any read are reviewed. The index lists no untriaged vendor location and no untriaged production line. Parent cases also run under the tracking inhibit (`phy_param[0x17]`/`[0x195]`) and with calibration tracking disabled (`[0x192]`), which script no DCODE reads because no RX calibration runs.  Done: `analog/temperature.rs` (exhaustive sensor comparison kills the conversion constants; the three reset-DAC constants are killed by a host test; the ten `selected_dac` bounds are reviewed equivalent). Killed by point mutants (`--patch`, tracking scenario DIFF): `analog/rfpll.rs` 304 (the channel-table bound, as `frequency < 2484`) and `analog/dcode.rs` 194, 204 (both `value <= 0x3e`). Killed by a host test: `analog/rfpll.rs` 485 (the channel-readiness sample count reported at the production deadline). Direct RFPLL programming (`RfpllFrequencyTransition::new`, used by the RX and TX IQ, crystal-duty, Bluetooth and frequency calibrations) is claimed: ROM `phy_set_rfpll_freq` against the production target executor over 100 cases (frequencies on both sides of the 4000-MHz divider split, every crystal selector, byte offsets, late and missed lock, and capacitor-search status profiles including signed wrap and the high capacitor byte). It found a production deviation, now fixed: the upward capacitor search continued from the downward offset instead of restarting at `initial + 1`. The lock and search limits are compared through exactly consumed finite read scripts (a missed lock reads all 100 samples). Killed by point mutants: the SDM divisors, shifts, masks, `0xff00_0000` bias, offset scaling and 4000-MHz split. The fifth SDM byte (bit 27), which the ROM stores only into a caller scratch buffer nothing reads, is removed; the two surviving mutants on it are now moot. The ready-flag take of a completed delay future at its `.await` is reviewed as async bookkeeping. The stage closes with empty untriaged, unobserved and unprojected lists and no untriaged survivor; the index generated after the unprojected-state triage lists none and every survivor above is killed or moot. |

### Checkpoint 12.N: native claims

| Unit | Status | Acceptance |
| --- | --- | --- |
| 12.N.1 | done | Runner-side effect filters become reviewed effect contracts evaluated by Blobray: transport plumbing, the single-microsecond delay before a transport or PBus status read, the vendor's unused SAR and skipped-DC snapshots, and readiness-wait visibility. A contract may declare `unclassified: required` (user decision): effects outside its rules then compare exactly in order and value, while the strict every-effect-classified policy remains available. An `ignored` disposition retains raw effects without pairing them, and a pattern may select an effect by its immediate successor. Blobray enters the selected root directly with deterministic callee-saved registers (user decision), so contracts and evidence bind the real vendor and production functions and the probe entry adapters are removed. Each contract is reviewed through knowledge assertions and bound by identity; verdicts carry the claim. |
| 12.N.2 | done | Semantic output mappings (`phy_param` fields ↔ production output) become reviewed layout projections with final-state comparison. The aperture reports the never-written registers each side read as evidence, and a relation can require them equal. |
| 12.N.3 | done | Execution capacity and cost: stack fill per case (no per-fill request split), lazy event-capacity admission, and a compact retained event encoding with an unchanged logical evidence contract. The largest current request (TX-DC) needs neither per-fill splitting nor hundreds of megabytes of evidence. |
| 12.N.4 | done | Every existing scenario (gain, i2c, channel, rx-gain, tx-dc) moves to native claims. Scenario code keeps peripheral inputs, matrices and independent oracles; runner-side comparison code is removed. Cases, verdicts, negatives and preservation are unchanged; each scenario's retained verdict alone establishes its claim. |
