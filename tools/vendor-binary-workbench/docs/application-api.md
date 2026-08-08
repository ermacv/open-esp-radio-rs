# Application API and alternate frontends

The workbench has one project/application layer and may have multiple
frontends. The command-line interface, JSON publication and interactive TUI
must not implement independent analysis paths.

```text
vendor-project.toml
        |
        v
WorkbenchApplication
        |
        +-- WorkspaceSnapshot ------> CLI human/JSON
        |                         \--> read-only TUI
        +-- AnalysisReport
        `-- ExecutionComparisonReport
                    `-- TraceDiffReport per case
```

`WorkbenchApplication` is the public, stateful facade for a resolved project.
It owns reload-scoped caches and exposes data-only request/report types. It
does not parse CLI arguments, parse rendered command output, write stdout, or
change reviewed project files.

## Opening and inspecting a workspace

```rust,no_run
use std::path::Path;
use open_radio_vendor_binary_workbench::WorkbenchApplication;

let mut application = WorkbenchApplication::open(Path::new(
    "verification/vendor/targets/esp32s31/vendor-project.toml",
))?;
let snapshot = application.snapshot()?;

println!("{}", snapshot.project_status.project_id);
println!("{} functions", snapshot.functions.len());
println!("{} registers", snapshot.registers.registers.len());
# Ok::<(), open_radio_vendor_binary_workbench::ApplicationError>(())
```

`WorkspaceSnapshot` combines:

- lifecycle readiness and component diagnostics;
- recovered and reviewed function summaries, context and generalized memory
  fields, typed replay-required scenario suggestions, pseudo-Rust, and
  explicitly reviewed logical types;
- the resolved register catalog and register-review counts;
- resolved interface contracts, slot ABI, semantic annotations, executable
  model links and concrete call sites;
- checked-in comparison profile identities and their source/symbol/scenario
  summary.

Missing generated facts are represented as incomplete components and
diagnostics. They do not prevent a frontend from opening the rest of a valid
project. Invalid project, target, memory-map, SVD, or reviewed-model inputs fail
at `open`/`reload`, because continuing with a partially resolved project would
mix incompatible state.

## Reload and cache ownership

`reload` resolves the manifest and all referenced configuration again,
atomically replaces application state, clears analysis caches and increments
`WorkspaceSnapshot::generation`. A failed reload leaves the previous resolved
state intact. Frontends should use generation as the identity of all rows and
selection state derived from a snapshot.

`analyze` caches identical `AnalyzeRequest` values within one generation.
`compare` returns the full `ExecutionComparisonReport`, rather than only one
`TraceDiffReport`, because a comparison can contain multiple scenarios and
argument-domain cases. Each different case carries its typed first-difference
report. `ComparisonScenario` also accepts source-specific
`ExecutionTableInstance` values, so an alternate frontend can model concrete
callback-table placement and symbol-backed slots without writing raw pointer
bytes or pinning linked addresses.
`ExecutionScenario::device_models` accepts shared `ExecutionDeviceModel`
factories. Every side gets fresh runtime state, while the typed execution
report retains each model descriptor and configuration as environment
provenance.
`compare_profile(name)` is the project-oriented form used by interactive
frontends. It loads a profile declared by `[verification]`, resolves its exact
vendor/Rust artifacts from the caller-owned run spec, validates runtime table
instances against the reviewed interface workspace, and returns the same typed
report as `compare`.

## Frontend contract

All frontends follow these rules:

1. Treat reports as immutable snapshots. Do not keep references to internal
   project models across reloads.
2. Run expensive `analyze` and `compare` calls outside the terminal event loop.
3. Render command results on stdout only in the CLI. Diagnostics, tracing and
   progress remain stderr concerns.
4. Do not infer semantics from display names. Interface semantics and compiled
   execution models are explicit fields of resolved contracts.
5. Do not mutate reviewed packs from a read-only browser. Future editing flows
   should produce explicit proposed patches and reuse the normal validators.

The [`project browse`](tui.md) TUI is intentionally a read-only project
browser. It complements the scriptable CLI; it is not a second command grammar
or a replacement for checked-in project data.
