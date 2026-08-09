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
  record every incomplete or misleading stage.
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
  the 2026-08-09 inventory found 155 affected interface functions, including
  valid floating-point, CSR and vendor/system instructions.
- [x] Preserve unsupported instructions as per-PC fail-closed blockers instead
  of discarding the entire function. F/CSR permit conservative linear
  continuation; system/vendor/invalid instructions stop only their current CFG
  path. All-zero illegal encodings are distinguished as ambiguous
  zero-fill/trap evidence instead of decoder failures. Concrete execution
  remains strict and cannot consume this tolerant structural stream.
- [x] Make project IR root selection explicit: `roots = "all"` is the normal
  full-symbol mode and `roots = "symbol-prefix"` requires a non-empty prefix;
  an empty-string convention is not part of project configuration.
- [ ] Make classified physical MMIO usable without requiring SVD names. SVD or
  the reviewed register model should enrich an exact address, not decide
  whether the observed access exists.
- [ ] Make the generated register inventory a practical review loop: address,
  width, read/write/RMW patterns, bit candidates, callers, evidence and a clear
  path into the editable register model.
- [ ] Classify diagnostic bulk MMIO reads separately from operational driver
  accesses without hiding them. On ESP32-S31, `phy_reg_check` deliberately
  walks hundreds of consecutive registers for logging; these are real reads,
  but they should not look equivalent to control-path dependencies.
- [x] Separate project-owned register ranges from external MMIO observations;
  external system-register evidence remains visible but cannot block the
  radio-only SVD/PAC publication gate.
- [ ] Make pseudo-code/function review practical for full artifacts: navigation
  by source and function, calls, MMIO, contexts, blockers and exact evidence,
  without treating best-effort reconstruction as decompilation proof.
  The TUI and generated function review now expose typed per-PC decode blockers;
  navigation and filtering across the full set still need a real-project pass.
- [x] Preserve the distinction between a recognized external operation and an
  executable external-call model. A semantic label alone does not make
  execution complete.
- [x] Provide typed first-difference trace presentation for vendor/Rust
  comparison, including nearby effects, producer PCs and linked offsets.
- [ ] Exercise that trace presentation on real ESP32-S31 mismatch and
  incomplete cases and fix any evidence that is still hard to interpret.
- [ ] Exercise the read-only TUI on the real ESP32-S31 project and make it a
  useful optional frontend over the same reports; it must not introduce a
  second analysis implementation.
- [x] Verify the real ESP32-S31 Overview, Functions and Registers views at
  80×24; keep compact tabs/status/help visible, redraw only on change, and use
  distinct header-aware table viewport math so active Register and Comparison
  rows cannot scroll into a border. Functions remains a header-free list and
  has its own long-scroll regression case.

## P1 — generic model gaps

- [x] Generalize context recovery into reviewed memory objects covering
  arguments, globals, dereferenced globals and absolute objects.
- [x] Add explicit reviewed logical-type unification across functions and
  globals without inferring nominal identity from matching offsets.
- [x] Separate reviewed function-table layout from runtime table instances and
  retain lifecycle evidence for slot and pointer installation.
- [x] Represent bounded indexed table calls (`base + index * stride`) through
  reviewed index domains without guessing a single fixed slot.
- [ ] Treat a linked ELF as authoritative link selection and archives as source
  inventory; add origin provenance instead of implementing a linker.
- [x] Add pluggable peripheral execution models for W1C, read-to-clear,
  self-clearing bits, FIFO and indexed banks while retaining simple scripted
  MMIO as the generic baseline.
- [x] Generate candidate scenarios from recovered branch/MMIO predicates, with
  concrete executor replay remaining the validation authority.
- [ ] Validate memory objects, logical types, table instances, device models
  and generated scenario drafts in the real ESP32-S31 reviewed workflow; code
  presence alone is not product completion.

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
- [x] Bound `ir export` human function rows after the real all-ROM run emitted
  1,935 rows; show the 64 most active functions and an explicit omitted count.
- [x] Apply a matching `source-companion:ID` to leaf IR export when its resolved
  run spec contains exactly one primary source; never attach source companions
  ambiguously to a multi-primary analysis.
- [ ] Decide whether large reviewed function/interface packs need composable
  fragments by source or subsystem. Split only with stable identity, validation
  across fragments and one project-level view.

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
- [x] Keep mutation of linked-IR shared summaries serial. Parallel workers emit
  function-local facts only; call-graph linking, SCC/fixed-point summaries,
  indexing and rendering happen after the deterministic join.
- [x] Use `petgraph` for standard SCC analysis through an adapter while keeping
  the serialized/domain graph model independent of the crate.
- [ ] Consider debug/source enrichment and property-based executor testing
  where they replace standard algorithms or strengthen correctness;
  dependencies are not goals by themselves.

## Later, after the functional base

- [ ] Optional solver-assisted scenario suggestions with concrete replay.
- [ ] Optional DWARF/source-line and Rust/C++ demangling enrichment.
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

ROM IR contains 1,935 roots, 492 MMIO identities and 416 complete functions;
schema 37 inventories 945 unsupported instruction sites across exactly 155
functions. Of those sites, 464 are all-zero illegal encodings now separated as
`zero-fill-or-illegal-trap`; no site remains in the generic `invalid` class.
The linked archive image contains 171 roots, 106 MMIO identities, 66 complete
functions and no decode blockers. Interface schema 5 retains 593 reached
blocker sites across all three scanned containers, including 116 zero/trap
sites, and reports zero analysis failures. These are regression references,
not universal performance promises: artifact hashes, build profile and host
matter, and best-effort partial functions remain incomplete.
