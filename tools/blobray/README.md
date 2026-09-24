# Blobray

Blobray investigates captured RV32 ELF/archive inputs and retains analysis,
reviewed knowledge and concrete comparison evidence. `cargo blobray` runs the
new application directly; its Linux supervisor owns memory/time limits and child
process cleanup. No external limiter is required.

## Choose a task

Start with [the host tutorial](../../docs/first-contribution.md) for a synthetic
exercise, or [the hardware route](../../docs/station-hardware.md) for real input
and board prerequisites. Commands below name current families; follow each
reference for required selectors, request files and supported subcommands.

| Task | Current command families | Reference |
| --- | --- | --- |
| Capture and inspect inputs | `init`, `import`, `inventory`, `select`, `doctor` | [Capture](next/README.md#use), [selection](next/README.md#selection-and-inspection-plans), [diagnosis](next/README.md#diagnosis-and-recovery) |
| Investigate code | `analyze-function`, `analyze-project`, `research` | [Function analysis](next/README.md#function-analysis-contract), [library research](next/README.md#library-investigations) |
| Find accesses and relationships | `find-accesses`, `find-references`, `navigate`, `flow`, `memory-slice` | [Navigation](next/README.md#navigation-over-saved-research), [flow](next/README.md#structural-flow-and-effect-inventory), [memory](next/README.md#memory-definitions-at-a-publication-point) |
| Investigate registers and data | `registers`, `data`, `export-data`, `knowledge` | [Register research](next/README.md#saved-register-research), [tables and coefficients](next/README.md#captured-data-tables-and-coefficients) |
| Prepare a linked image | `link-plan`, `prepare-image`, `images` | [Prepared images](next/README.md#synthetic-prepared-images) |
| Inspect static representations | `ir`, `trace` | [Semantic IR](next/README.md#saved-semantic-ir-profiles), [static traces](next/README.md#static-observable-traces) |
| Execute and compare | `execute`, `compare`, `replay` | [Execution and comparison](next/README.md#concrete-execution-and-comparison), [effect contracts](next/README.md#reviewed-effect-comparison) |
| Preserve research | `backup`, `restore`, explicit export commands | [Knowledge and preservation](next/README.md#knowledge-and-preservation) |

## Three different choices

- **Research method:** static analysis, concrete scenario execution or comparison.
  A static trace is not an executed trace or an equivalence proof.
- **Output format:** `--format human` or `--format json`; this changes presentation,
  not the research method or verdict.
- **Resource enforcement:** `--limit-mode kernel` requires delegated cgroups;
  `--limit-mode watchdog` explicitly chooses sampled process-tree enforcement.
  There is no automatic fallback.

Related tools have different owners: `cargo registers` publishes reviewed
hardware interfaces; `cargo xtask` checks and builds repository compositions;
`cargo hil` obtains device observations; `cargo qualification` evaluates a
selected set of requirements. Blobray does not decide product readiness.

## Start an investigation

```console
cargo build --profile blobray -p blobray-next --bin blobray
cargo blobray init --project /path/to/research
cargo blobray import --project /path/to/research --input vendor=/path/to/lib.a --limit-mode watchdog
cargo blobray analyze-project --project /path/to/research --limit-mode watchdog
cargo blobray status --project /path/to/research --limit-mode watchdog
```

Kernel enforcement requires a delegated cgroup. `--limit-mode watchdog` explicitly
selects sampled process-tree RSS enforcement when that is the desired policy;
there is no automatic fallback. See the [operator reference](next/README.md) for
linking, research, knowledge review, execution/comparison, replay and preservation.
Explicit executable ranges and physical static/dynamic symbols support retained
code research without inferred boundaries. Exact data ranges, integer-table review and instruction-derived constants can be
exported with captured bytes and provenance; see the operator reference.
CLI/JSON share the [application](crates/application/README.md) operations.
Saved interface discovery, function/context review, structural paths, memory slices
and conditional event routes retain their evidence. The register catalogue reports
MMIO candidates and masks separately from reviewed physical declarations.
Configured semantic IR profiles retain original facts and provenance. Static trace
queries compare explicitly selected physical MMIO/fence observations with visible
path blockers and assumptions; they are distinct from concrete execution.

Register source publication belongs to the independent
[register tool](../registers/README.md). It consumes reviewed hardware models and
policies, without loading an investigation or executing vendor code.

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
cargo xtask check blobray-standalone
```

Concrete execution supports phased RV32 integer/stack arguments, atomics, explicit
goals, device/call models and reviewed runtime interfaces/FIFO services. Comparison
selects MMIO/fence/delay, low/high return words, final normal-memory ranges,
physical or reviewed call pairs, and the internal memory/branch timeline. Explicit
accepted projections map byte fields/arrays, branch coordinates and ABI word
positions while preserving unknowns and raw physical evidence.
Unknown selected values and unmet goals/model obligations cannot MATCH. Results
remain conditional on the selected cases and explicit modeling assumptions.

[Reviewed effect contracts](next/README.md#reviewed-effect-comparison) are
implemented and retain their conditional claim ceiling. Cross-revision
correspondence/rebase, retention GC, reference-code generation and TUI remain
pending. Unsupported execution remains `INCOMPLETE`; these limitations do not
enable the old engine implicitly.

The old facade sources still contain unported responsibilities. They are not the
normal command, and their command grammar is not supported by `cargo blobray`.
The [architecture](docs/design/architecture.md), [contracts](docs/design/contracts.md)
and [workflows](docs/design/workflows.md) distinguish implemented scope from target
obligations. Qualifying production behavior remains an external responsibility.
