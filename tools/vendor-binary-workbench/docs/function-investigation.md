# Focused function and scope investigation

`inspect function` is the lossless reading surface for one project function. It
does not invoke `objdump` and does not stop when symbolic analysis becomes
incomplete:

```console
cargo vendor-binary-workbench inspect function \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  libpp:wDev_AppendRxBlocks
```

The run spec normally supplies both `source-artifact:libpp`, the authoritative
linked image, and `source-inventory:libpp`, the raw archive used as origin
evidence. `--artifact`, `--inventory`, `--member`, and `--origin-member` are
available for focused experiments.
When a linked image contains duplicate symbol names, use the persistent exact
identity form `SOURCE:SYMBOL@0xADDRESS`; this selects the matching linked
definition without relying on archive-member names.

One typed report contains:

- every byte and decoded or explicitly unsupported instruction in the linked
  symbol;
- relocations, local labels, conservative basic blocks and successors;
- the uniquely associated raw archive member when the symbol inventory proves
  one;
- a grouped inventory of the archive member's relocation-backed global and
  call dependencies; `--full` also prints the complete origin instruction
  body with its relocation sites;
- each schema-validated linked-IR projection, pseudo-Rust and its exact
  blockers;
- every recovered direct MMIO/RAM effect joined to its exact instruction PC
  and conservative basic block;
- direct call knowledge (`unknown`, `annotated`, `executable`, or resolved
  code), recovered reachable call-graph edges, and reviewed asynchronous event
  dispatch receiver/selector evidence;
- reviewed preconditions and operational/diagnostic/timeout/error/recovery
  paths from the function pack;
- a proof ledger that keeps container, decode, CFG, link-origin and semantic
completeness separate.

A reviewed event route is joined only when the generated dispatcher proves the
same mechanism, execution context, optional receiver and constant selector.
The report then loads the exact handler identity from its named IR profile and
shows its direct effects, calls, reachable inventory and semantic blockers in
the same result. This closes the asynchronous navigation edge; it does not
claim that the handler's selector-specific internal branch is feasible. A
missing queue output model or scenario therefore remains visible at the
handler boundary instead of being hidden by the reviewed route.

The default human output is a bounded semantic summary. Add `--full` for the
complete CFG and lossless instruction listing; instruction rows then carry
exact-PC call, semantic and blocker annotations. `--depth N` selects the
outgoing call-graph slice and `--callers` adds incoming edges to the root
without recursively expanding reverse edges through common utilities such as
logging. Machine-readable output always keeps the complete selected report.

`--path FROM:TO` isolates one shortest directed CFG path. Locations are
absolute PCs or explicit function offsets such as `+0x0:+0x14`. This is a
navigation view, not symbolic execution: the result always carries
`feasibility_claim = false`, while concrete scenario replay remains the proof
that branch predicates and runtime state make a path executable.

Blockers are structured by stable root ID, layer, kind and instruction site.
Their explanation names the missing input: a logical memory object, runtime
table instance, external-call model, scenario/precondition, device read
sequence, or a specific callee whose summary must be closed.

A semantic blocker never truncates the raw body. Preconditions and path labels
are reviewer assertions, not permission to discard control-flow evidence and
not execution proof. JSON and JSONL serialize the same report shown by the
human renderer. The Functions tab in `project browse` loads this report lazily
for the selected function and therefore uses the same evidence path as the CLI.

The report never associates archive instruction offsets with linked PCs by
simple arithmetic. Linker relaxation can shrink or rewrite instruction
sequences (for example a relocated call pair). A bounded monotonic structural
alignment records only same-shape relocation sites and recognized linker
relaxations. It is navigation evidence with `semantic_equivalence_claim =
false`; runtime linked IR and relocation-rich archive facts remain separate
truth domains. The alignment makes otherwise stripped globals actionable in
blocker explanations without turning a heuristic offset projection into proof.

`inspect object SOURCE:SYMBOL` uses the data-object index rather than scanning
the function corpus. It reports linked address/size/initializer/relocations and
all recovered per-function read/write offsets:

```console
cargo vendor-binary-workbench inspect object \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  wifi-sta-lifecycle:ap_no_lr
```

`inspect scope` reads one already generated review-scope artifact:

```console
cargo vendor-binary-workbench inspect scope \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  wifi-sta-beacon-filter-policy
```

It shows the exact explicit vendor roots, reachable inventory, MMIO coverage,
blockers, and every feature qualification that consumes the scope, including
covered/total effects and missing proofs. It does this without reparsing
artifact-wide IR. Run `project analyze` after changing scope roots or generated
IR; stale scope schemas and configuration are rejected rather than recomputed
implicitly.

Use the raw listing as the decode/container fallback, not as a second manual
analysis workflow. Semantics still come only from reviewed packs, linked IR,
execution models and concrete verification.
