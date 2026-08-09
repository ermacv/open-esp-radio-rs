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
- [ ] Define and test what "analyze all functions" means for ELF, linked ELF,
  archive members, local/internal symbols, recovered code boundaries, aliases,
  overlapping symbols and decode failures.
- [x] Make project IR root selection explicit: `roots = "all"` is the normal
  full-symbol mode and `roots = "symbol-prefix"` requires a non-empty prefix;
  an empty-string convention is not part of project configuration.
- [ ] Make classified physical MMIO usable without requiring SVD names. SVD or
  the reviewed register model should enrich an exact address, not decide
  whether the observed access exists.
- [ ] Make the generated register inventory a practical review loop: address,
  width, read/write/RMW patterns, bit candidates, callers, evidence and a clear
  path into the editable register model.
- [x] Separate project-owned register ranges from external MMIO observations;
  external system-register evidence remains visible but cannot block the
  radio-only SVD/PAC publication gate.
- [ ] Make pseudo-code/function review practical for full artifacts: navigation
  by source and function, calls, MMIO, contexts, blockers and exact evidence,
  without treating best-effort reconstruction as decompilation proof.
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
  equality. Keep conservative automatic mode; expose explicit `--jobs N`.
- [x] Add a small real-project resource regression record for the supported
  analysis path so later changes cannot silently restore unbounded RAM use.
- [ ] Consider parallel linked-IR profiles because profiles are independent.
  Implement only after measuring per-profile peak memory and defining a total
  memory/concurrency budget.
- [ ] Do not parallelize mutation of one linked-IR reachable graph until its
  shared summaries have an explicit deterministic ownership model.
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

Recorded 2026-08-09 with the already-built feature-enabled debug binary, so
Cargo compilation/linking is excluded:

```console
/usr/bin/time -f 'elapsed=%e max_rss_kb=%M' \
  target/debug/vendor-binary-workbench project analyze --check --jobs 2 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Result: all 12 configured stages verified, elapsed 50.84 s, peak RSS 219,164
KiB. The facts contain 2,128 register-width observations across four MMIO
ranges and 25,854 bounded diagnostics. Treat this as a regression reference,
not a universal performance promise: artifact hashes, build profile and host
matter. Future optimizations must compare equivalent generated outputs and
record both time and peak RSS.

The same host also completed explicit all-symbol linked IR for both real source
artifacts. ROM: 1,935 roots, 488 MMIO identities, 416 complete functions,
45.66 s, 216,488 KiB peak RSS, 64 MiB JSON and 7.0 MiB pseudo-Rust. Linked
archive image: 171 roots, 106 MMIO identities, 66 complete functions, 11.60 s,
92,036 KiB peak RSS, 19 MiB JSON and 2.5 MiB pseudo-Rust. These results prove
artifact-wide generation is functional; they do not turn best-effort partial
functions into completeness claims.
