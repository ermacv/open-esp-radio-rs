# Vendor Binary Workbench product TODO

This file is the durable product backlog. It records direction and acceptance
criteria; generated findings and one-off experiment notes do not belong here.

## Product priority

The first priority is a correct, understandable and functional end-to-end
tool for vendor-binary analysis and Rust replacement validation. Performance
work is important only when a real project demonstrates a bottleneck.

Do not trade analysis coverage, fail-closed behavior, evidence provenance,
deterministic output, or usable diagnostics for speed. An optimization is
accepted only when it has:

- a reproducible real-project benchmark and a stated bottleneck;
- explicit resource and completeness limits rather than silent truncation;
- equivalence or determinism tests against the previous execution mode;
- measured improvement in elapsed time or peak memory;
- no new chip, RTOS, or vendor semantics in the generic backend.

The normal user path must remain project-oriented:

1. bind private artifacts once with `project inputs init`;
2. generate all analysis evidence with `project analyze`;
3. inspect and edit reviewed project knowledge;
4. reproduce with `project analyze --check`;
5. publish SVD/PAC/bindings separately with `project publish`.

## Closed PAC migration (2026-08-11)

- [x] Split ESP32-S31 register access into private, non-publishable `pac-raw`
  and application-facing `pac` crates. Remove raw SVD and physical-register
  reexports from the HAL.
- [x] Rename project configuration and publication status to
  `[registers.pac-raw]` / `pac-raw-publication`; the old `[registers.pac]`
  key has no compatibility path.
- [x] Make new projects create an empty reviewed `registers/api.toml` and
  document the practical MMIO -> TOML -> SVD -> raw PAC -> closed PAC order.
- [x] Close MAC interrupt-enable writes behind `MacInterruptMask`; status and
  acknowledgement retain separate finite snapshot types.
- [x] Delete the legacy `power.rs` physical-address catalog. The remaining
  `Register32`/`Field32` catalog exists only behind the hidden host-test
  feature and is absent from ordinary PAC/HAL builds.
- [ ] Generate the closed PAC facade from reviewed API schema 2. Writable
  inputs must be flags, enums, bounded values, fixed images, or
  register-specific opaque domains; naked cross-register `u32` writers are
  forbidden by default.
  - [x] Generate evidence-backed flag, enum, bounded and register-specific
    opaque value domains into `[registers.api].output`; generated flags have
    no public integer constructor.
  - [x] Bind every complete-register write to a reviewed domain and generate
    its private typed bridge to `pac-raw`. Target-owned public capability
    methods keep lifecycle, sequencing and ownership policy handwritten.
    - [x] Prove the vertical path with `mac_interrupt_enable`: its reviewed
      operation names `MacInterruptMask`, the generator owns the typed raw
      bridge, and every ESP32-S31 caller uses that bridge.
    - [x] Migrate all fourteen ESP32-S31 full-register writes and all twelve
      caller-built complete-image writes, plus all five masked RMW inputs.
      Reject every direct raw-PAC path in those classes outside generated or
      validation code with an architecture test.
- [ ] Migrate the remaining ESP32-S31 public numeric transaction parameters
  to generated domains and add compile-fail fixtures for addresses, raw PAC
  imports, arbitrary mask construction and unreviewed writes.

Leaf commands are focused inspection and repair tools, not a second workflow
that every user must learn.

## Focused investigation migration (2026-08-11)

- [x] Replace the removed monolithic linked-IR JSON file with schema-v48
  random-access bundles. Functions, call graph, registers and data objects
  have deterministic indexes; status/doctor/cache/navigation/review consumers
  use only the required member.
- [x] Add the dedicated compact `function-overview.jsonl` member. Project
  status and the TUI no longer scan the 1.1 GiB lossless function stream;
  selected detail uses the exact function index and retains pseudo/scenarios.
- [x] Make `inspect function` lossless at semantic blockers while keeping the
  default human view bounded. `--full` joins raw instructions to exact-PC
  calls, semantic operations, decode blockers and stable blocker IDs;
  structured blockers name the reviewed model required to continue.
- [x] Add bounded inter-function slices (`--depth`, optional root callers)
  without reverse-expanding through common callees, and add indexed
  `inspect object SOURCE:SYMBOL` for global/object xrefs.
- [x] Add `inspect function --path FROM:TO` as a shortest directed CFG slice.
  Absolute PCs and explicit `+OFFSET` locations are accepted, and the report
  states `feasibility_claim = false` so graph navigation cannot be mistaken
  for a satisfiable symbolic path or concrete execution proof.
- [x] Upgrade reviewed event routes to schema 6. Dispatcher, consumer/delivery
  boundary, output field and selector-specific case handler are distinct. The
  ESP32-S31 RX-success route records `lmacProcessRxSucData ->
  ppTask(queue_recv item-out) -> wdevProcessRxSucDataAll` without treating
  reviewed edges as execution proof.
- [x] Add a safe lifecycle for artifact-bound reviewed code boundaries.
  `code rebase --apply` refreshes guards only when every decision still maps
  to a valid current candidate; structural changes require a separate review
  candidate and cannot overwrite the reviewed pack implicitly.
- [x] Add bounded indexed `inspect flow` target/effect/event-route modes. The
  real selector `0x19` report loads three function records in 0.07 seconds at
  7.8 MiB RSS, preserves the observed post and queue call, and names the two
  missing executable models instead of recursively serializing the graph.
- [x] Surface exact direct and pointee constant domains in the compact flow
  table. The real Wi-Fi key paths now show AP `a0=1 -> a1=1 ->
  a3+0:u8=1` and STA `a0=0 -> a1=0 -> a3+0:u8=0` through
  `ic_set_key`, `wDev_Insert_KeyEntry` and `hal_crypto_set_key_entry`.
- [x] Make the `0x19` route executable.
  - [x] Generalize external caller-stack outputs to reviewed 8/16/32-bit
    little-endian writes and bind `queue_recv(item-out)` to a 32-bit ABI model.
    This is call knowledge only and deliberately leaves `event-delivery=false`.
  - [x] Add scenario-owned stateful FIFO instances with reviewed, mechanism-
    neutral enqueue/dequeue/length ABI bindings and ordered lifecycle evidence.
  - [x] Recover the real ELF `.L1019` selector table as bounded schema-v51
    `indexed-dispatch` edges. Selector `0x19` reaches `.L1026` and
    `wdevProcessRxSucDataAll` in the local linked `libpp` oracle.
  - [x] Add the concrete multi-phase replay that posts selector `0x19`,
    preserves it in the FIFO, executes `ppTask` until the handler boundary and
    proves delivery. `advanced execute replay` now runs the checked-in
    `replays/pp-signal-25.toml` against an execution-oriented link unit: the
    real `pp_post` enqueue, latch transition, `ppTask` dequeue, indexed jump
    and `wdevProcessRxSucDataAll` boundary execute in one persistent session.
  - [x] Join the successful replay document into `inspect flow --event-route`.
    Exact FIFO delivery now sets `event-delivery`; `path-feasibility` remains
    false because the replay starts at `pp_post`, below the structural root.
  - [x] Retain and validate the counted-latch state transition around the FIFO:
    the producer must increment the reviewed byte, the consumer must observe
    that result and decrement it, and both writes retain exact instruction PCs.
  - [x] Continue from the executed selector handler to a reviewed synchronous
    terminal and join its production Rust replacement. Relocation-backed calls
    from archive origins survive earlier semantic blockers as explicitly
    structural edges; missing ABI values still keep the complete route
    fail-closed.
- [x] Generalize instruction-site evidence from calls/diagnostics to every
  MMIO and memory-object read/write, preserving the originating basic block
  and value provenance in persistent IR.
- [x] Bound zero-sized `.L*`/`.LANCHOR*` data objects by the next symbol and
  deduplicate aliases at one location. This removes duplicated section tails
  from memory and turns compiler anchors into usable constant/jump-table
  objects without dropping initializer or relocation evidence.
- [x] Join the stored project Replacement Graph into `inspect function` and
  the TUI instead of adding a parallel inspection engine. The report keeps
  exact/unique-symbol association, production component, probes, proofs and an
  explicit `freshness_claim = false` boundary.
- [x] Add the compact `inspect function SOURCE:SYMBOL --replacement` view.
  It skips function-body analysis and joins binding scope, exact production
  source item, compiled/DWARF status, proof claim strength and every consuming
  required feature. `bounded-match` is rendered as a property proof, never as
  whole-function equivalence.
- [x] Join the same Replacement Graph boundary into reviewed event-flow
  reports. An absent production mapping is a typed blocker rather than a
  guessed Rust function based on a vendor symbol name.
- [ ] Extend that joined replacement view with ordered effect/RAM-transition
  diffs and accepted differences from the concrete comparison report. Do not
  build a separate comparison engine for the TUI. The focused view now joins
  adapter/scenario cases from the existing suite report plus current reviewed
  effect selectors and dispositions as an explicitly unordered policy table.
  Matching scenarios expose event/RAM-change counts and failed scenarios keep
  the comparator's first-difference index; complete ordered matching traces
  still need a compact sidecar or bounded on-demand projection rather than
  duplicating large traces into every aggregate report.

## P0 — functional analysis workflow

- [x] Audit a complete real project from inputs through MMIO, linked IR,
  pseudo-code, interface/function review, verification and publication;
  record every incomplete or misleading stage. The verification segment now
  has thirteen typed project suites and a reproducible aggregate gate. The
  2026-08-12 ESP32-S31 run matches 149 whole-function replacements plus
  two explicit bounded feature properties with zero mismatch, incomplete or
  implemented-unqualified results. `wDev_AppendRxBlocks` now reproduces the
  complete fence sequence in all nine cases. The aggregate `project check`
  also requires 3/3 feature qualifications before publication may pass.
  Analysis inventory backlogs remain visible but do not masquerade as failed
  replacement proofs. Verification artifact paths are canonical, so report
  freshness is independent of the current working directory.
- [x] Make real-project verification one project operation rather than a set of
  remembered leaf commands. Suites own source roles, probe roles/prefixes,
  profile/disposition fragments, baseline and gate; `project verify --check`
  reproduces one aggregate report. Candidate evidence is emitted only to a
  separate review directory and cannot overwrite accepted baselines.
- [x] Add a project-wide Replacement Graph. It deduplicates `(source, symbol)`
  across suites, rejects conflicting reviewed mappings, and links disposition,
  Rust component, probe, proof and qualification blockers. The 2026-08-09 real
  current run has 1,839 unique vendor nodes, 149 whole-function matches, two
  bounded properties and 53 reviewed Rust components.
- [x] Separate production ownership from verification-only probes in the
  replacement graph. Probe symbols never imply a production item; reports now
  distinguish `production_matches`, `probe_only_matches` and
  `unmapped_matches`.
- [x] Separate whole-function production replacements from bounded production
  properties. Verification schema 10 emits `bounded-match`, replacement-graph
  schema 3 emits `production-feature`, review-scope schema 8 cannot promote it
  to a whole replacement, and `project check` schema 3 evaluates required
  features as its own gate.
- [ ] Assign reviewed production component identities where the 97 passing
  probe-only matches correspond to actual production replacements. Keep true
  reference/probe-only functions classified as such instead of inventing a
  production module from their symbol names.
- [x] Join reviewed Rust component identities to the Cargo workspace source
  AST and exact suite ELF/DWARF evidence without adding another project
  manifest. The current project resolves 49/53 component source items and 38
  compiled identities with 175 DWARF locations. Compile-time types and host
  semantic owners without a target symbol remain explicit rather than being
  inferred from their probes. PHY transition types
  still have no target-ELF occurrence because their executable authority is a
  host semantic harness; keep that gap explicit until host harness artifacts
  or target composition probes are indexed.
- [x] Make artifact-wide IR the checked ESP32-S31 project default. The
  `rom-all` and `archive-all` profiles use `roots = "all"`; named local and
  externally visible code symbols plus reviewed recovered boundaries are
  retained with explicit selection provenance.
- [ ] Finish the definition of "analyze all functions" for aliases and
  overlapping symbols; named ELF roots, archive members, local/internal
  symbols, recovered code boundaries and decode blockers are covered.
- [ ] Implement semantics for the remaining ISA selected by the real target.
  ESP32-S31 declares
  `riscv32imafc`, while the current `rv-asm` boundary only decodes I/M/A/C;
  the 2026-08-09 inventory found 155 affected ROM function definitions, including
  valid floating-point, CSR and vendor/system instructions.
- [x] Recover structural memory effects for RV32F `flw`/`fsw`, including all
  compressed forms used by the ROM. Addresses reuse integer base provenance;
  loaded bits flow through a separate floating-register state, while the F
  blocker remains until arithmetic semantics are modeled.
- [x] Preserve exact bit provenance for `fmv.w.x`, `fmv.x.w` and `fsgnj*`
  (`fmv.s`/`fabs.s`/`fneg.s` aliases), with ABI caller-saved invalidation at
  calls. These operations remain visible F blockers but no longer destroy an
  unrelated integer register or force a later `fsw` value to unknown.
- [x] Recover `fle.s`, `flt.s` and `feq.s` integer results when both source bit
  patterns are exact. IEEE NaN comparison behavior is preserved; the reached
  F instruction remains a blocker because exception flags and full floating
  execution are not modeled.
- [x] Continue structural analysis past unresolved returning relocations and
  indirect `jalr ra` calls as opaque ABI boundaries. Caller-saved integer and
  floating registers are invalidated, escaped private-stack pointers remain
  conservative, and an explicit reference blocker prevents proof-quality use;
  callee-saved context pointers can still expose later memory/MMIO evidence.
- [x] Preserve unsupported instructions as per-PC fail-closed blockers instead
  of discarding the entire function. F/CSR permit conservative linear
  continuation; system/vendor/invalid instructions stop only their current CFG
  path. All-zero illegal encodings are distinguished as ambiguous
  zero-fill/trap evidence instead of decoder failures. Concrete execution
  remains strict and cannot consume this tolerant structural stream.
- [x] Restrict linked-IR decode blockers to instructions reachable from the
  function entry with a lightweight address-only CFG walk. Do not call the
  provenance-heavy interface analysis from parallel IR workers: that trial
  reproduced a ~2.9 GiB peak, while the dedicated pass retained the normal
  ~355 MiB resource envelope.
- [x] Decode review-only mnemonics for the remaining F/CSR/system encodings so
  the TUI can distinguish `flw`, `fsw`, comparisons, arithmetic and CSR access
  instead of showing only a class plus raw word. Mnemonic recognition does not
  promote the instruction to modeled semantics.
- [x] Make project IR root selection explicit: `roots = "all"` is the normal
  full-symbol mode and `roots = "symbol-prefix"` requires a non-empty prefix;
  an empty-string convention is not part of project configuration.
- [x] Make classified physical MMIO usable without requiring SVD names. SVD or
  the reviewed register model should enrich an exact address, not decide
  whether the observed access exists. Concrete execution records unnamed
  accesses inside a declared MMIO region as ordered effects; comparison keeps
  their addresses in coverage diagnostics but does not make them incomplete.
- [x] Make the generated register inventory a practical review loop: address,
  width, read/write/RMW patterns, bit candidates, callers, evidence and a clear
  path into the editable register model. The real report identified the two
  release blockers as diagnostic-only snapshots in
  `libpp:wdev_record_rx_linked_list`; the reviewed non-operational policy now
  preserves their evidence without inventing SVD identities.
- [x] Add project review scopes as a persisted generated artifact. A scope
  joins configured IR roots with their reachable closure, static and linked
  MMIO, all blocker classes and replacement coverage. `project status` reads
  the compact artifact instead of repeating scope reconstruction, while
  `ir build` refreshes it after a focused profile rebuild.
- [x] Separate the recursive analysis closure from the Rust replacement
  boundary. Review-scope schema 8 requires reviewed production coverage only
  for explicit roots; reachable private helpers remain full blocker/MMIO/call
  inventory and can be absorbed by a root-level composition. On the real
  project this removed 85 false 1:1 uncovered replacements and left only the actual
  uncovered TX roots `hal_mac_tx_config_edca` and
  `hal_mac_tx_get_blockack`.
- [x] Turn publication scopes into an actionable root-cause queue. Linked-IR
  schema 48 carries typed diagnostic kind/site/root IDs, review-scope schema 8
  groups repeated causes and joins replacement coverage, and the read-only TUI
  exposes a Blockers view with function navigation. Parallel legacy string
  blocker arrays were removed from the persistent IR schema.
- [x] Separate replacement qualification from artifact-wide analysis inventory.
  Schema-v8 scopes qualify only explicit production replacement roots; every
  reachable vendor-helper blocker remains visible in the inventory and review
  queue without making an otherwise proven Rust composition incomplete.
- [x] Gate SVD/PAC publication by explicit `publication-scopes`, not every
  artifact-wide observation. The 2026-08-09 ESP32-S31 run reduced 17 global
  unreviewed observations to two release-relevant RX registers,
  `0x20104090` and `0x20104094`; all other findings remain visible in review.
- [x] Classify diagnostic bulk MMIO reads separately from operational driver
  accesses without hiding them. On ESP32-S31, `phy_reg_check` deliberately
  walks hundreds of consecutive registers for logging; these are real reads,
  but they should not look equivalent to control-path dependencies. The
  reviewed non-operational function policy now classifies an address only when
  every observed reader/writer is non-operational; mixed-use addresses remain
  operational and stale function identities fail validation.
- [x] Add focused `inspect register ADDRESS` over the same typed detail used by
  the TUI. It separates operational/non-operational users, exact PCs,
  publication debt and neighboring reviewed identities without inventing a
  hardware name from address adjacency.
- [x] Preserve semantic operation IDs on compact linked-IR call records. The
  schema-v52 function projection can validate reviewed event consumers without
  loading the lossless function stream or falsely reporting a missing OSI
  delivery call.
- [ ] Run the checked-in `rx-descriptor-pipeline` HIL probe through cold,
  published and active RX states. Promote `0x20104090/0x20104094` to reviewed
  read-only descriptor addresses only if their values repeatedly match the
  logged software node/next topology; otherwise retain them as diagnostic-only
  unknown observations. The isolated runtime currently passes a target
  `cargo check`, but the normal host runner intentionally has no legacy
  `cargo hil oracle` subcommand; add an explicit authenticated build/flash/run
  entry point before calling this a reproducible product workflow.
- [x] Separate project-owned register ranges from external MMIO observations;
  external system-register evidence remains visible but cannot block the
  radio-only SVD/PAC publication gate.
- [ ] Make pseudo-code/function review practical for full artifacts: navigation
  by source and function, calls, MMIO, contexts, blockers and exact evidence,
  without treating best-effort reconstruction as decompilation proof.
  The TUI and generated function/register review now expose typed per-PC decode
  blockers and exact schema-v5 MMIO access sites even when CFG recovery is
  incomplete. The Functions snapshot unions those sites with linked-IR MMIO,
  so partial pseudo-code no longer produces a false zero-register view.
  Navigation and filtering across the full set still need a real-project pass.
- [x] Add lossless project-aware `inspect function SOURCE:SYMBOL`. One typed
  report keeps every symbol byte, instruction, relocation, label and CFG block,
  then overlays linked-IR pseudo-code, explicit blockers, archive-origin
  evidence, reviewed preconditions/path classes and external-call knowledge.
  Semantic incompleteness never truncates the raw body. The real
  `wDev_AppendRxBlocks` report accounts for 334/334 linked bytes and 378/378
  origin bytes in 2.4 seconds; the TUI consumes the same lazy report.
- [x] Make driver-feature qualification cover the complete explicit vendor
  effect boundary. Review-scope schema 8 stores exact replacement function
  keys and feature-pack schema 3 distinguishes complete `review-scopes` from
  narrow `bounded-evidence`. Scope features require every discovered key to be
  proven or policy-excluded; bounded properties require explicit replayed
  proofs and forbid exclusions. Missing and stale dispositions fail closed.
  The STA beacon-filter scope exposes all three set/enable/disable transactions
  with 3/3 dispositions, while the AP/STA key-role feature qualifies only its
  pinned two-bit property without claiming whole-function completeness.
- [x] Add a real AP/STA hardware-key role oracle and Rust-conformance gate.
  Immutable raw-archive instruction/relocation shapes pin vendor role
  propagation and AP=1/STA=0 constants; the adapter executes the production
  Rust CCMP builders, checks the resulting control field and proves that the
  two role images differ only in the reviewed context bit.
- [x] Preserve the distinction between a recognized external operation and an
  executable external-call model. A semantic label alone does not make
  execution complete.
- [x] Distinguish a modeled external `void` call from unmodeled behavior.
  `Void` records the ordered call without inventing a dummy return; reviewed
  ESP32-S31 critical-section, timer, queue, tick and selected coexistence/NVS
  boundaries now use 27 behavior-only models. Enabling them exposed deeper
  station-state call-graph evidence instead of stopping at the first adapter
  call.
- [x] Model reviewed directly relocated platform inputs separately from
  diagnostics and table slots. The structural executor can propagate an
  explicit constant or symbolic return and codegen calls a distinct platform
  boundary; ESP32-S31 now declares the fixed 40 MHz
  `rtc_clk_xtal_freq_get`, while `phy_printf` remains a diagnostic-only ABI.
- [x] Provide typed first-difference trace presentation for vendor/Rust
  comparison, including nearby effects, producer PCs and linked offsets.
- [ ] Exercise that trace presentation on real ESP32-S31 mismatch and
  incomplete cases and fix any evidence that is still hard to interpret.
- [ ] Exercise the read-only TUI on the real ESP32-S31 project and make it a
  useful optional frontend over the same reports; it must not introduce a
  second analysis implementation.
- [x] Add a scope-first TUI view over the persisted review-scope artifact.
  Release membership, completeness, replacement gaps and exact function/MMIO
  identities are visible without rerunning reachability, and Enter follows a
  scope member into the existing lazy Functions view.
- [x] Verify the real ESP32-S31 Overview, Functions and Registers views at
  80×24; keep compact tabs/status/help visible, redraw only on change, and use
  distinct header-aware table viewport math so active Register and Comparison
  rows cannot scroll into a border. Functions remains a header-free list and
  has its own long-scroll regression case.

## P1 — generic model gaps

- [x] Inventory named static data objects from linked ELF and relocatable
  archive members. Schema-v42 retains uninterpreted initializer bytes,
  symbolic relocations, `.LANCHOR*` aliases and per-function read/write xrefs
  without pretending that archive offsets are runtime addresses. The real
  COEX archive exposes 119 named objects plus 42 anchor-only section objects,
  including `coex_pti_tab`, `g_coex_param`, `coex_schm_env` and the scheduler
  scheme family.
- [x] Preserve byte-indexed relocated data accesses such as
  `coex_pti_tab[arg0]` as an indexed memory object with stride one. This is
  provenance only; a finite selector domain still requires reviewed evidence
  or a recovered guard.
- [x] Build and bind an authoritative linked COEX oracle ELF while retaining
  `libcoexist.a` as inventory/origin authority. `coex_hw_timer_set` and the
  four timer-control leaves have exact production matches; the latter form
  the required `coex-hardware-timer-control` feature across all five banks.
- [ ] Build an authoritative linked BLE oracle ELF. Keep the BLE archives as
  inventory/origin authority, then bind only the table instances and external
  execution models required by reviewed `ble-advertising` scenarios.
- [x] Promote the structurally complete four-leaf COEX timer-control scope to
  a required feature with production equivalence over all five timer banks.
  `coex_hw_timer_set` itself has an exact concrete production comparison, but
  remains in the wider non-gating scope because focused structural inspection
  still exposes three conservative `coex_hw_timer_tick_get`/division blockers.
  Do not promote that wider scope merely from the successful bounded scenarios.
- [ ] Turn reviewed static objects into editable logical type/table bindings.
  Initializer bytes and xrefs are evidence; field names, element counts and
  nominal type unification must remain explicit review claims.
- [x] Generalize context recovery into reviewed memory objects covering
  arguments, globals, absolute objects, indexed objects and pointers loaded
  from any exact known memory object. Dynamic pointer-dependent addresses stay
  visible as RAM evidence without becoming false fixed fields. On the real
  `wDev_AppendRxBlocks` slice this reduced diagnostics from 78 to 23.
- [x] Add explicit reviewed logical-type unification across functions and
  globals without inferring nominal identity from matching offsets.
- [x] Separate reviewed function-table layout from runtime table instances and
  retain lifecycle evidence for slot and pointer installation.
- [x] Represent bounded indexed table calls (`base + index * stride`) through
  reviewed index domains without guessing a single fixed slot.
- [x] Make resolved reviewed interface contracts the structural ABI registry.
  Reviewed slots now become named opaque calls in linked IR on both discovered
  and alternative CFG paths, while only explicit compiled models authorize
  execution. The real `rom-all` profile contains 20 named opaque calls and no
  `unregistered-external-abi-slot`; 16 calls correctly remain blocked as
  `unmodeled-reviewed-external-call`.
- [x] Remove table layout, pointer-symbol, version, magic and ordinary slot ABI
  duplication from compiled harnesses. The reviewed interface pack is now the
  only owner of anchors, container paths, guards, layout and slot ABI. Compiled
  harnesses expose behavior-only `ExternalCallModelSet` values, joined solely
  by the reviewed slot's explicit model foreign key; generated references no
  longer reproduce table version/magic/size guards.
- [x] Treat a linked ELF as authoritative link selection and archives as source
  inventory; add origin provenance instead of implementing a linker. Direct
  absolute LinkUnit symbols are now preserved: 15 archive calls formerly
  rendered as `sub_2f80003c` are typed `ets_delay_us` boundaries and the PHY
  scope's unresolved count fell from 20 to 7. Symbol-inventory schema v4 now
  associates externally selectable linked-ELF text definitions with exact
  same-source archive member candidates by name and kind, while retaining
  `linker_resolution_claim = false`. The real project has 2,897 unique member
  associations, zero ambiguous associations and 1,673 definitions with no
  archive origin.
- [x] Project reviewed archive interface evidence onto authoritative linked
  ELF calls without turning archive association into linker truth. The join is
  fail-closed over unique symbol origin, reviewed pointer-cell association and
  identical decoded indirect-target/call shape; linker-relaxed instruction
  positions may differ. The real 2026-08-10 profiles now name 175 `libpp` and
  626 `libnet80211` calls, including RTOS, timer, NVS, allocation, clock and
  coexistence operations, with explicit
  `archive-origin-interface-association` provenance. Semantic annotation still
  does not authorize execution.
- [x] Preserve relocation provenance for linker-relaxed archive data loads in
  linked structural analysis. The real `pp_create_task` load of
  `g_osi_funcs_p` now resolves reviewed slot `+0x90` as
  `task_create_pinned_to_core`; target proof and argument recovery remain
  independent claims.
- [x] Add focused `inspect function --calls` and `--call TARGET` reports. The
  real callsite recovers all seven ABI arguments, including `ppTask`, the
  stack-size result, `max-priority - 2`, a null handle output and
  `g_wifi_menuconfig + 0x34`. Narrow human output is a vertical inventory and
  machine output is a compact versioned callsite document rather than the
  complete function/CFG payload.
- [x] Add pluggable peripheral execution models for W1C, read-to-clear,
  self-clearing bits, FIFO and indexed banks while retaining simple scripted
  MMIO as the generic baseline.
- [x] Generate candidate scenarios from recovered branch/MMIO predicates, with
  concrete executor replay remaining the validation authority.
- [ ] Validate memory objects, logical types, table instances, device models
  and generated scenario drafts in the real ESP32-S31 reviewed workflow; code
  presence alone is not product completion. The 2026-08-10 real `libpp-all`
  pass now recovers `wdev_record_rx_linked_list` as a structured branch with
  seven MMIO reads and eleven fields in
  `0x1002f560[arg0 * 0x2c]`; the unresolved callback and two unknown values
  remain explicit completeness blockers. This validates indexed absolute
  objects, but reviewed nominal types and executable models still need real
  project instances.
- [x] Validate the first real linked-image logical type. Absolute RAM evidence
  is now rebased onto the narrowest sized ELF data symbol, the reviewed
  `VendorPhyParameterImage` binds 194 exact `(offset, width)` observations for
  `phy_param`, and 19 high-confidence fields are named from executable
  semantic contracts. Overlapping byte/halfword/word observations remain
  distinct evidence instead of being rejected as false width conflicts.
- [x] Complete semantic annotation of the reviewed ESP32-S31 Wi-Fi OS adapter
  surface. All 54 named slots and all resolved call sites link to reusable
  catalog operations; only explicit compiled call models authorize execution.
- [x] Complete executable external-call behavior beyond scalar results. The
  generic contract, structural analysis, generated reference ABI, concrete
  executor, profile schema 2 and linked-IR schema 48 now preserve independent
  RV32 `a0`/`a1` returns and reviewed private-stack byte outputs. This covers
  `queue_send_from_isr`, coexistence PTI output and `esp_timer_get_time`
  without weakening them to one `SymbolicU32`. Reviewed zeroing allocators now
  produce affine allocation objects in static IR and fresh zeroed CPU-owned
  arenas in concrete execution; allocation site, requested size and capacity
  are retained as environment evidence.
  The schema-v46 real-project run persists 148 modeled libpp call sites (75
  `symbolic-u32`, 17 `symbolic-u64`, 55 `void`, one constant) and 18 independent
  private-stack outputs. It removes two Wi-Fi RX call-graph blockers and one
  Wi-Fi interrupt blocker; remaining release blockers are predominantly memory
  ownership, branch-aware stack composition and unresolved indirect control
  flow rather than scalar ABI loss.
- [ ] Complete allocator lifetime after allocation. Add reviewed deallocation
  behavior for `free`, close object lifetime at the exact call site, and reject
  use-after-free/double-free in concrete scenarios. Static IR must continue to
  preserve allocation provenance without guessing heap addresses or aliases.
- [x] Qualify the first recovered parent composition rather than only its
  leaves. `phy_bt_tx_gain_init` now locks its linked direct-call topology and
  arguments, drives `PhyBluetoothTxGainInitTransition` through the same
  deterministic child models, and matches both cold and retained state. The
  real linked image completed 506,725 concrete steps across the two cases.
- [x] Qualify the release parents hierarchically without duplicating leaf
  effect models. `phy_bb_init` locks its linked 26-call topology, arguments
  and reviewed RAM footprint in cold and retained cases (959,177 steps).
  `register_chipv7_phy` additionally locks its 19 direct calls, outer
  prelude/RF/baseband/temperature/tail order, complete `phy_param` transfer
  and the exact 524-byte caller-owned calibration image on the supported cold
  full-calibration path (868,143 steps). Retained-cache hardware replay remains
  intentionally outside the production transition and therefore outside this
  contract.

## P1 — project usability and maintainability

- [ ] Continue the component audit: locate oversized/mixed-responsibility
  modules, legacy paths, duplicate report models, handwritten serialization,
  stale vocabulary and configuration that is not reachable from the project
  workflow.
- [x] Keep `vendor-project.toml` as the normal entry point and document every
  other TOML file by ownership: composition, private input, reviewed knowledge
  or generated evidence. `project files` now reports entrypoint/local/external/
  reviewed/generated ownership, producer, consumers and presence; the concise
  getting-started guide uses only project-first commands.
- [ ] Ensure every failed/blocked project stage prints one concrete next action
  and the exact responsible file.
- [x] Add typed per-component `next_action` data to project status and collapse
  duplicate actions in the human frontend. The real ESP32-S31 status now links
  register, interface and function review counts to their report and editable
  pack, and explains the common publication blocker once.
- [x] Keep human output bounded and task-oriented; one typed JSON document is
  the only machine result on stdout, while diagnostics/progress remain on
  stderr. TSV and user-facing JSONL were removed without compatibility paths.
  Status/doctor/files lead with outcome, problems and ordered next actions;
  focused function inspection separates a concise pseudo-Rust/call/problem
  view from the lossless `--full` body.
- [ ] Load the four linked-IR inputs once per project operation and share one
  typed workspace snapshot between validation, review, navigation and TUI.
  On the 2026-08-10 real run, `functions review` alone spent about 100 seconds
  reparsing and projecting already generated IR; the full pipeline repeats
  equivalent loads in multiple stages. Function validation/review and
  interface-backed function review/interface validation now share their typed
  workspaces during one cold pipeline run. The TUI receives code, functions,
  registers, interfaces, root-cause queue and comparisons through one typed
  snapshot, but its collectors still parse some IR inputs independently and
  project navigation remains a separate load/projection stage. The
  function/interface join no longer performs a nested all-functions scan:
  indexing concrete callers reduced real `functions review` time from
  102.57 s to 3.62 s with byte-identical output. Remaining repeated loads are
  now the dominant cost.
- [ ] Stream large persistent JSON documents to files and hash them while
  writing instead of materializing a complete `String` and reparsing it for a
  downstream stage. The schema-38 four-profile run peaked at 1,252,836 KiB;
  correctness is stable, but the current ownership unnecessarily retains
  duplicate document trees/strings.
- [x] Bound `ir export` human function rows after the real all-ROM run emitted
  1,935 rows; show the 64 most active functions and an explicit omitted count.
- [x] Apply a matching `source-companion:ID` to leaf IR export when its resolved
  run spec contains exactly one primary source; never attach source companions
  ambiguously to a multi-primary analysis.
- [x] Replace generated-in-reviewed-pack synchronization with sparse overlays.
  Function schema v3 and interface schema v2 store only reviewed/ignored human
  decisions; omitted facts remain computed backlog. The obsolete `interfaces
  sync-pack` path and its compatibility machinery were removed.
- [x] Avoid premature pack fragmentation: the real function/interface packs
  fell from 421 KiB to 11 KiB once generated `unreviewed` rows were removed.
  Stable source/subsystem fragments are unnecessary until actual reviewed
  knowledge grows enough to justify them.

## P2 — justified performance work

- [x] Bound symbolic expressions, exploration states, trace steps/events and
  retained diagnostics after reproducing multi-gigabyte growth on the real
  ESP32-S31 project.
- [x] Add bounded MMIO-function workers and verify serial/parallel result
  equality. Automatic mode uses up to four available workers after the real
  optimized project stayed below 400 MiB; expose explicit `--jobs N`.
- [x] Add bounded function-local linked-IR workers for artifact-wide roots.
  Workers cover symbols across ROM ELF and archive members, then join before
  deterministic SCC/fixed-point summaries and publication. Prefix-discovered
  reachable closures remain serial.
- [x] Add a small real-project resource regression record for the supported
  analysis path so later changes cannot silently restore unbounded RAM use.
- [x] Measure profile-level parallelism before implementing it. `rom-all`
  dominates `archive-all` (47.32 s versus 10.71 s in debug), so profile workers
  would save at most the smaller profile while retaining both large documents;
  function-local scheduling gives better load balance.
- [x] Parallelize independent per-root linked-IR effect summaries after the
  deterministic function join. Workers only read the immutable graph and
  publish `(root, summary)` pairs that are sorted before mutation; serial and
  parallel reports are equality-tested. A one-artifact merge also reuses its
  already computed report instead of calculating every summary twice.
- [x] Use `petgraph` for standard SCC analysis through an adapter while keeping
  the serialized/domain graph model independent of the crate.
- [x] Make `project analyze` dependency-aware and incremental. The expanded
  four-source ESP32-S31 write workflow measured 6:43.71 and 1,236,708 KiB peak
  RSS on 2026-08-09, even when only review scopes needed refresh after a schema
  fix. A focused `ir build --profile archive-all` refreshed the same dependent
  scope artifact in 8.96 s / 224,980 KiB. The 2026-08-10 schema-38 run still
  needed 5:46.95 / 1,252,044 KiB merely to rebuild a stale navigation index,
  although all-profile IR itself completed in 1:58.29. Reuse validated
  unchanged stage outputs by content identity; do not add more user-facing
  repair commands. The content-addressed implementation hashes the executable,
  declared inputs and outputs, bypasses reuse in `--check`, and reports a
  distinct `up-to-date` state. On the real schema-38 project, the cache-seeding
  run completed all 13 stages in 5:56.13 / 1,249,016 KiB; an unchanged repeat
  completed with 13 cache hits in 10.82 s / 410,896 KiB.
- [x] Add source/ELF/DWARF enrichment where it strengthens navigation without
  changing proof semantics. The project verification report now resolves all
  42 reviewed Rust components in source, 34 in configured target ELFs and 200
  DWARF locations; the remaining eight PHY components explicitly identify the
  host-harness/target-artifact boundary.
- [ ] Add property-based executor testing where generated and shrunk cases
  strengthen instruction-semantic correctness; dependencies are not goals by
  themselves.

## Later, after the functional base

- [ ] Optional solver-assisted scenario suggestions with concrete replay.
- [ ] Extend the active Rust DWARF/demangling index to vendor C++ names only
  when a real artifact needs it.
- [ ] Shell/man/wizard polish beyond the existing project happy path.
- [ ] Additional ISA/lifting backends only when a second architecture provides
  a concrete maintenance and coverage requirement.

## Real-project resource baseline

### Active memory-safety regression (2026-08-11)

An unrestricted artifact-wide run exhausted the host and killed the enclosing
tmux/Codex session. Until a new cold baseline is recorded, historical
measurements below are diagnostic history, not permission to run the current
schema without isolation.

- [x] Make one worker the CLI default and reject `--jobs 0`; parallelism is an
  explicit measured opt-in in the range `1..=8`.
- [x] Add `scripts/run-limited`: prefer a 1-GiB/no-swap user systemd scope and
  fall back to a Linux RSS watchdog; both paths have a 15-minute wall limit
  and there is no unrestricted fallback. `RLIMIT_AS` was removed after it
  rejected a 474-MiB real run solely because glibc retained released virtual
  address ranges.
- [x] Remove project-wide concatenated pseudo-Rust outputs and their generated
  ESP32-S31 files. Pseudo remains per function and as a focused prefix export.
- [x] Stream every linked-IR bundle member into a private staging directory,
  compare check-mode output in 64-KiB buffers, enforce a 512-MiB bundle limit,
  and publish only a complete directory.
- [x] Make recursive IR records stack-safe and bounded on read, and cap only
  nominal pointer/type projection at 16 dereferences. Linked-list execution
  evidence, instructions and blockers remain lossless; the real
  `libnet80211-all` overview shrank from 11.38 to 6.54 MiB and its largest
  record from 1,087,615 to 77,694 bytes without changing symbolic execution.
- [x] Remove quadratic copies of reachable-function identities and transitive
  blocker messages from every function summary. The graph and direct typed
  diagnostics remain the lossless sources; focused analysis materializes the
  selected closure without copying it into every artifact-wide root.
- [x] Stop retaining every transitive projected semantic action in every root
  function. The current checked artifacts show why: effect summaries account
  for 81% of `libnet80211-all/functions.jsonl`, and projected semantic actions
  alone account for about 91 MiB. Spool root projections or derive them for a
  selected review/inspection root while preserving direct call arguments and
  guard evidence.
  - [x] Schema v48 persists direct facts and the call graph once, computes
    exact closure scalars, and marks heavyweight root projections as
    deferred. Prefix-focused exports retain complete paths/actions;
    artifact-wide indexes retain direct guard-backed semantic/event evidence
    and reconstruct transitive paths only for focused investigation.
- [x] Bound the complete set of scheduled affine-projection states, not only
  the number already popped from the queue. The old check allowed a branching
  call graph to enqueue millions of paths before reaching its 4096-state
  processing limit. The schema-v51 real `wifi-sta-lifecycle` profile now
  completes all 2997 functions in 61 seconds with a measured 474,448-KiB peak;
  compact summary construction takes 0.37 seconds and exact direct facts plus
  `graph.json` remain the transitive source of truth.
- [x] Record a cold, non-cached ESP32-S31 `--jobs 1` baseline through
  `scripts/run-limited`. The worst isolated profile (`wifi-sta-lifecycle`,
  2997 functions) completed in 9.72 s at 498,096 KiB and emitted a 151.5-MiB
  bundle. No functions, direct paths, MMIO, memory, call or blocker facts were
  dropped; transitive root projections are explicitly deferred to focused
  analysis.
- [x] Preflight every selected IR artifact, inventory and companion before
  loading catalogs. Missing generated images now identify the profile, typed
  run-spec role and exact path. The schema-v48 real `btbb-all` write/check
  smoke test reproduces 186 functions, 111 registers and 277 field candidates
  in 0.40/0.35 s at 168,588/168,048 KiB through the hard-limit runner.
- [x] Measure the read-only real-project status path after the UI migration.
  It completes in 1.53 s at 266,952 KiB RSS through `scripts/run-limited`.
  Deduplicating repeated physical artifact paths reduced the measured baseline
  from 1.61 s while preserving one explicit report row per input role. An
  allocation-light function-pack projection produced no measurable RSS/time
  improvement and was deliberately not retained.
- [ ] Reduce the all-profile single-process high-water from 580,824 KiB to the
  isolated-profile target (at most 512 MiB). Every profile is dropped and
  glibc-trimmed to about 53 MiB before the next one; remaining allocator
  fragmentation can be removed by profile process isolation if measurements
  justify the extra worker protocol. The current 1-GiB hard runner remains
  mandatory meanwhile.

Recorded 2026-08-09. The old feature-enabled debug path, excluding Cargo
compilation/linking, was:

```console
/usr/bin/time -f 'elapsed=%e max_rss_kb=%M' \
  target/debug/vendor-binary-workbench project analyze --check --jobs 2 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

With both project profiles in artifact-wide mode, the write run completed in
89.31 s at 253,148 KiB peak RSS and the reproducing check completed in 87.63 s
at 299,552 KiB. All 12 stages passed. Before sequential profile publication,
the same workload retained every generated JSON/pseudo document and peaked at
about 2.5 GiB; `ir build` now drops each profile's documents before generating
the next one.

The normal optimized incremental alias now completes the same twelve-stage
check in 15.11 s at 288,764 KiB with `--jobs 1`, 8.90–9.06 s at roughly
345–355 MiB with `--jobs 2`, and 5.50 s at 379,420 KiB with `--jobs 4` (the
automatic ceiling). An explicit eight workers measured 4.66 s at 456,204 KiB.
The isolated `rom-all` profile takes 6.61 s / 269,252 KiB serial and 4.51 s /
348,432 KiB with two function workers. Generated output is byte-identical in
all runs. The first `workbench` profile compilation is an explicit one-time
build cost; subsequent source changes remain incremental.

After CFG-reachable blocker filtering, floating-memory provenance and opaque
returning-call continuation, the same optimized `--jobs 4` check completed in
5.30 s at 435,624 KiB on 2026-08-09. That historical profile predates the
four-source schema-38 workspace; Cargo compilation remains excluded.

The artifact-wide schema-38 model changed that workload materially on
2026-08-10. Before profiling, `libpp-all` still ran after seven minutes.
`perf` showed repeated 32-bit rediscovery of unchanged ABI arguments and an
eager four-way recursive affine-address query. Canonical `Input` values,
lazy single-pass address matching, bounded parallel root summaries and removal
of the duplicate one-artifact summary reduced the same 1,312-function debug
build to 16.69 s at 591,684 KiB. The complete four-profile `ir build --jobs 4`
finished in 1:58.29 at 1,252,836 KiB; the thirteen-stage `project analyze`
finished in 5:49.29 at 1,252,008 KiB after final canonical-input regeneration.
That was the baseline at this point in the schema history. Subsequent evidence
growth invalidated it; the active regression section above owns the current
safety criteria.

ROM IR contains 1,935 roots, 453 MMIO identities and 426 complete functions;
schema 38 inventories 593 CFG-reachable unsupported instruction sites across
exactly 155 functions and additionally preserves indexed memory-object
provenance. Of those sites, 116 are all-zero illegal encodings now
separated as `zero-fill-or-illegal-trap`; no site remains in the generic
`invalid` class. A whole-symbol scan found 945 unsupported byte sequences, but
352 occur after a return or another path terminator and are intentionally not
presented as function blockers.

The typed `PhyState` probe refresh exposed a separate provenance-cost
regression on 2026-08-10. A cold four-profile `project analyze --jobs 4`
required 478.49 s / 1,428,996 KiB. Profiling showed recursive
`MemoryObjectRoot` clone/drop work consuming most linked-IR CPU: every pointer
chase copied the complete immutable `Dereferenced`/`Indexed` ancestry.
Sharing recursive roots with `Arc` retained the exact serialized model while
reducing `libpp-all` from about 127 s to 2.83 s, all four IR profiles from
about 291 s to 20.45 s, and MMIO discovery from about 166 s to 10.20 s. The
complete non-cached `project analyze --check` now takes 51.48 s / 1,248,392
KiB and verifies all thirteen outputs byte-for-byte. Navigation root matching
was also changed from a symbols-per-call scan to exact artifact/name/address
indexes; the generated navigation SHA-256 remained unchanged.
The linked archive image contains 171 roots, 547 MMIO identities, 66 complete
functions and no decode blockers. `libpp-all` contains 1,312 roots, 566
complete functions, 561 MMIO identities and 1,588 memory fields;
`libnet80211-all` contains 1,714 roots, 209 complete functions, four MMIO
identities and 3,342 memory fields. Interface schema 5 retains 593 reached
blocker sites across all three scanned containers, including 116 zero/trap
sites, and reports zero analysis failures. These are regression references,
not universal performance promises: artifact hashes, build profile and host
matter, and best-effort partial functions remain incomplete.
