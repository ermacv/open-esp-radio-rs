# Blobray

Blobray investigates captured RV32 ELF/archive inputs and retains analysis,
reviewed knowledge and concrete comparison evidence. `cargo blobray` runs the
new application directly; its Linux supervisor owns memory/time limits and child
process cleanup. No external limiter is required.

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
physical or reviewed call pairs, and the internal physical memory/branch timeline.
Unknown selected values and unmet goals/model obligations cannot MATCH. Results
remain conditional on the selected cases and explicit modeling assumptions.

Reviewed cross-layout/ABI projections, effect contracts, cross-revision
correspondence/rebase, retention GC, reference-code generation and TUI remain
pending. Unsupported execution remains `INCOMPLETE`; these limitations do not
enable the old engine implicitly.

The old facade sources still contain unported responsibilities. They are not the
normal command, and their command grammar is not supported by `cargo blobray`.
The [architecture](docs/design/architecture.md), [contracts](docs/design/contracts.md)
and [workflows](docs/design/workflows.md) distinguish implemented scope from target
obligations. Qualifying production behavior remains an external responsibility.
