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

Leaf commands are focused inspection and repair tools, not a second workflow
that every user must learn.

## P0 — functional analysis workflow

- [ ] Audit a complete real project from inputs through MMIO, linked IR,
  pseudo-code, interface/function review, verification and publication;
  record every incomplete or misleading stage. The verification segment now
  has nine typed project suites and a reproducible aggregate gate: the
  2026-08-09 ESP32-S31 run matched all 138 reviewed proofs with zero mismatch,
  incomplete result or orphan probe. Project status still correctly reports
  the interface/function review backlog, so the end-to-end audit remains open.
- [x] Make real-project verification one project operation rather than a set of
  remembered leaf commands. Suites own source roles, probe roles/prefixes,
  profile/disposition fragments, baseline and gate; `project verify --check`
  reproduces one aggregate report. Candidate evidence is emitted only to a
  separate review directory and cannot overwrite accepted baselines.
- [x] Add a project-wide Replacement Graph. It deduplicates `(source, symbol)`
  across suites, rejects conflicting reviewed mappings, and links disposition,
  Rust component, probe, proof and qualification blockers. The 2026-08-09 real
  run has 3,203 unique vendor nodes, 138 qualified matches and 42 reviewed Rust
  components.
- [x] Separate production ownership from verification-only probes in the
  replacement graph. Probe symbols never imply a production item; reports now
  distinguish `production_matches`, `probe_only_matches` and
  `unmapped_matches`.
- [ ] Assign reviewed production component identities where the 97 passing
  probe-only matches correspond to actual production replacements. Keep true
  reference/probe-only functions classified as such instead of inventing a
  production module from their symbol names.
- [x] Join reviewed Rust component identities to the Cargo workspace source
  AST and exact suite ELF/DWARF evidence without adding another project
  manifest. The real project resolves all 42 component source items and 34
  compiled identities with 200 DWARF locations. Eight PHY transition types
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
- [x] Turn release scopes into an actionable root-cause queue. Linked-IR
  schema 39 carries typed diagnostic kind/site/root IDs, review-scope schema 3
  groups repeated causes and joins replacement coverage, and the read-only TUI
  exposes a Blockers view with function navigation. Parallel legacy string
  blocker arrays were removed from the persistent IR schema.
- [x] Gate SVD/PAC publication by explicit `release-scopes`, not every
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
- [x] Preserve the distinction between a recognized external operation and an
  executable external-call model. A semantic label alone does not make
  execution complete.
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
- [ ] Remove table layout, pointer-symbol, version, magic and ordinary slot ABI
  duplication from compiled harnesses. Execution-model resolution now has its
  own module; next reduce the harness contract to executable return/RAM/event
  behavior joined by the reviewed pack's explicit model foreign key.
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
  `VendorPhyParameterImage` binds 196 exact `(offset, width)` observations for
  `phy_param`, and 21 high-confidence fields are named from executable
  semantic contracts. Overlapping byte/halfword/word observations remain
  distinct evidence instead of being rejected as false width conflicts.
- [x] Complete semantic annotation of the reviewed ESP32-S31 Wi-Fi OS adapter
  surface. All 54 named slots and all 176 resolved call sites now link to one
  of 57 reusable catalog operations; only the existing 18 explicitly compiled
  call models authorize execution.
- [x] Qualify the first recovered parent composition rather than only its
  leaves. `phy_bt_tx_gain_init` now locks its linked direct-call topology and
  arguments, drives `PhyBluetoothTxGainInitTransition` through the same
  deterministic child models, and matches both cold and retained state. The
  real linked image completed 506,725 concrete steps across the two cases.
- [ ] Qualify the two remaining release parents hierarchically:
  `phy_bb_init` over the reviewed baseband children, then
  `register_chipv7_phy` over RF-init and baseband. Do not duplicate leaf
  effect models in the parents. The shared deterministic completion layer is
  now split by TX, RX-IQ, RX-gain and parent responsibilities; both cold and
  retained `PhyBbInitTransition` paths complete through it, and the existing
  Bluetooth parent still matches the real linked image. The vendor projection
  now locks all 26 direct-call sites, ordering and reviewed arguments without
  promoting that structural evidence to a verdict. The remaining work is its
  concrete cold/retained execution plus reviewed footprint comparison and then
  the outer registration composition.

## P1 — project usability and maintainability

- [ ] Continue the component audit: locate oversized/mixed-responsibility
  modules, legacy paths, duplicate report models, handwritten serialization,
  stale vocabulary and configuration that is not reachable from the project
  workflow.
- [ ] Keep `vendor-project.toml` as the normal entry point and document every
  other TOML file by ownership: composition, private input, reviewed knowledge
  or generated evidence.
- [ ] Ensure every failed/blocked project stage prints one concrete next action
  and the exact responsible file.
- [x] Add typed per-component `next_action` data to project status and collapse
  duplicate actions in the human frontend. The real ESP32-S31 status now links
  register, interface and function review counts to their report and editable
  pack, and explains the common publication blocker once.
- [ ] Keep human output bounded and task-oriented; JSON/JSONL retain complete
  machine evidence on stdout, while diagnostics/progress remain on stderr.
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
This is the current baseline. The gap is
now repeated document parsing, review/navigation projection and non-streaming
serialization, not instruction analysis or runaway symbolic recursion.

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
