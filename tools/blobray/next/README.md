# Blobray Next: captured inputs and bounded operations

`blobray-next` imports immutable inputs, analyzes RV32 libraries and images, prepares synthetic images, executes explicit RV32 scenarios,
compares compiled observations and
preserves projects through backup/restore. The API and CLI share supervised
work, cancellation, publication and recovery; `cargo blobray` selects this host.
The [architecture](../docs/design/architecture.md) owns module authority;
[contracts](../docs/design/contracts.md) owns identities, assessment, lifetime and
resource rules; [workflows](../docs/design/workflows.md) lists the supported
scenarios. This page indexes the current command references.

The current data model separates captured source revisions, selected analysis
recipes and immutable result publications. A run
records termination independently of scoped result coverage, policy checks and
comparison verdicts. `Completed` can legitimately describe partial research or a
`DIFF`/`INCOMPLETE` comparison. See [result assessment](../docs/design/contracts.md#result-assessment).

Implemented profiles include archive/thin-archive inventory, RV32 ELF inspection,
whole-library static analysis, linked images with explicit ROM companions,
bounded integer execution/comparison/replay, target auditing and project
preservation. Unsupported ISA semantics remain explicit gaps. There is
no general equivalence proof, TUI, CAS pruning or allocation-free core.

## Reference navigation

Choose a question in the [task map](../README.md#choose-a-task), then use the
relevant reference. CLI syntax is also available through `cargo blobray --help`
and each command’s `--help`. Commands below are the current Next interface.

`blobray __command-tree` prints every visible command with its long flags as
JSON, tracked in [`command-tree.json`](command-tree.json) so documentation
checks need not build Blobray. `cargo test -p blobray-next` fails when the
tracked tree differs from the command line; run it with
`BLOBRAY_COMMAND_TREE_UPDATE=1` to rewrite the file.

| Reference | Use it to |
| --- | --- |
| [Capture, selection and linked images](reference/capture-images/README.md) | Capture caller-owned artifacts, choose exact code/data scope and prepare linked images. |
| [Function and library analysis](reference/analysis/README.md) | Analyze captured code and inspect coverage, symbolic values and explicit gaps. |
| [Registers and captured data](reference/registers-data/README.md) | Inspect MMIO candidates and captured data. |
| [Preservation](reference/preservation/README.md) | Back up, restore and export retained projects and payloads. |
| [Semantic IR and static traces](reference/ir-traces/README.md) | Package saved facts and extract or compare selected static observable paths. |
| [Concrete execution and comparison](reference/execution/README.md) | Run explicit RV32 scenarios with selected inputs, device models and comparison observations. |
| [Comparison relations](reference/comparison/README.md) | Choose call, memory, branch, layout and effect relations. |
| [Resources, storage and recovery](reference/resources-storage/README.md) | Operate bounded jobs, inspect resource use and recover or preserve retained projects. |
| [Interfaces, identities and formats](reference/interfaces-formats/README.md) | Reference application owners, durable identities, result assessment and native formats. |

## Section links

These entry points retain existing bookmarks. Detailed contracts have one
canonical location in the references above.

## Use

See [Use](reference/capture-images/README.md#use).

## Coverage and storage observations

See [Coverage and storage observations](reference/resources-storage/README.md#coverage-and-storage-observations).

## Owners and interfaces

See [Owners and interfaces](reference/interfaces-formats/README.md#owners-and-interfaces).

### Shared operation lifecycle

See [Shared operation lifecycle](reference/interfaces-formats/README.md#shared-operation-lifecycle).

## Synthetic prepared images

See [Synthetic prepared images](reference/capture-images/README.md#synthetic-prepared-images).

### Link policy and evidence

See [Link policy and evidence](reference/capture-images/README.md#link-policy-and-evidence).

### Ownership, limits and persistence

See [Ownership, limits and persistence](reference/capture-images/README.md#ownership-limits-and-persistence).

## Selection and inspection plans

See [Selection and inspection plans](reference/capture-images/README.md#selection-and-inspection-plans).

## Identity and schema 1

See [Identity and schema 1](reference/interfaces-formats/README.md#identity-and-schema-1).

## Job outcomes and publication

See [Job outcomes and publication](reference/interfaces-formats/README.md#job-outcomes-and-publication).

## Preparation and measurements

See [Preparation and measurements](reference/resources-storage/README.md#preparation-and-measurements).

## Cooperative control and failure diagnostics

See [Cooperative control and failure diagnostics](reference/resources-storage/README.md#cooperative-control-and-failure-diagnostics).

### Current memory boundary

See [Current memory boundary](reference/resources-storage/README.md#current-memory-boundary).

## Temporary storage and crash cleanup

See [Temporary storage and crash cleanup](reference/resources-storage/README.md#temporary-storage-and-crash-cleanup).

## Diagnosis and recovery

See [Diagnosis and recovery](reference/resources-storage/README.md#diagnosis-and-recovery).

## JSON and checks

See [JSON and checks](reference/interfaces-formats/README.md#json-and-checks).

### Current formats

See [Current formats](reference/interfaces-formats/README.md#current-formats).

## Function analysis contract

See [Function analysis contract](reference/analysis/README.md#function-analysis-contract).

### Reopen a function analysis

See [Reopen a function analysis](reference/analysis/README.md#reopen-a-function-analysis).

### Analyze a linked image

See [Analyze a linked image](reference/analysis/README.md#analyze-a-linked-image).

### Structural decoding

See [Structural decoding](reference/analysis/README.md#structural-decoding).

### Values and memory effects

See [Values and memory effects](reference/analysis/README.md#values-and-memory-effects).

## Explicit ROM companions

See [Explicit ROM companions](reference/analysis/README.md#explicit-rom-companions).

## Library investigations

See [Library investigations](reference/analysis/README.md#library-investigations).

### Library workflow

See [Library workflow](reference/analysis/README.md#library-workflow).

### Library ownership and failure boundaries

See [Library ownership and failure boundaries](reference/analysis/README.md#library-ownership-and-failure-boundaries).

## Preservation

See [Preservation](reference/preservation/README.md#backup-and-restore).

## Concrete execution and comparison

See [Concrete execution and comparison](reference/execution/README.md#concrete-execution-and-comparison).

### Packed-command bank

See [Packed-command bank](reference/execution/README.md#packed-command-bank).

### External-call responses

See [External-call responses](reference/execution/README.md#external-call-responses).

## Final-image target audit

See [Final-image target audit](reference/analysis/README.md#final-image-target-audit).

## Captured data, tables and coefficients

See [Captured data, tables and coefficients](reference/registers-data/README.md#captured-data-tables-and-coefficients).

## Saved register research

See [Saved register research](reference/registers-data/README.md#saved-register-research).

## Saved semantic IR profiles

See [Saved semantic IR profiles](reference/ir-traces/README.md#saved-semantic-ir-profiles).

## Static observable traces

See [Static observable traces](reference/ir-traces/README.md#static-observable-traces).

## Physical call capture and comparison

See [Physical call capture and comparison](reference/comparison/README.md#physical-call-capture-and-comparison).

## Internal physical timeline

See [Internal physical timeline](reference/comparison/README.md#internal-physical-timeline).

### Reviewed layout comparison

See [Reviewed layout comparison](reference/comparison/README.md#reviewed-layout-comparison).

### Reviewed effect comparison

See [Reviewed effect comparison](reference/comparison/README.md#reviewed-effect-comparison).
