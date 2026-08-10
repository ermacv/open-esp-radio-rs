# Project analysis pipeline

`project analyze` is the reproducible, project-wide entry point for generated
reverse-engineering evidence. Its `--check` mode performs the same analyses
but only compares their rendered results with existing files. It never updates
an output.

This is the normal user interface. The leaf `symbols`, `mmio`, `interfaces`,
`ir`, `functions`, and `registers` commands expose the same components for
focused inspection and repair; they are not a second mandatory workflow.

```console
cargo vendor-binary-workbench project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project analyze --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`--jobs N` applies bounded concurrency to independent MMIO functions and to
function-local linked-IR recovery when a profile selects all named symbols.
Workers analyze individual functions across ELF/ROM symbols and archive
members; they do not mutate the shared call graph. Results are joined first,
then reachability/SCC effect summaries, indexes and serialization run in one
deterministic order. Prefix-root reachability remains serial because its root
set grows during discovery. Zero automatically uses up to four available
workers; use `--jobs 1` for minimum memory or an explicit `2..=8` after
measuring the target project.

The repository cargo aliases use the optimized incremental `workbench`
profile. Artifact-wide analysis at dev `opt-level=0` is intentionally not the
normal product path: it is useful for debugging but is several times slower.

Use `project doctor` first when diagnosing configuration, backend, catalog, or
private-artifact readiness. Use write mode after changing inputs or the
analyzer, and `project analyze --check` in CI when generated evidence is
retained.

Write mode maintains a local content-addressed stage cache at
`generated/.project-analyze-cache.json`. It is derived evidence, not another
project configuration file. A stage is `up-to-date` only when the analyzer
executable, all declared inputs, and every output still have their recorded
SHA-256 identity. Editing one reviewed pack therefore invalidates only stages
that consume it. `--check` never accepts cache hits: it always reproduces and
compares the complete configured analysis for CI.

## Pipeline boundary

The pipeline owns only reproducible evidence and read-only validation:

| Stage | Project configuration | Writes in default mode | Behavior with `--check` |
| --- | --- | --- | --- |
| `symbol-inventory` | `[analysis.symbols]`, run spec | complete symbol/linkage facts JSON | render and compare facts |
| `mmio-discovery` | `[registers].facts`, memory map, run spec | MMIO facts JSON | render and compare facts |
| `interface-discovery` | `[interfaces].facts`, run spec | interface facts JSON | render and compare facts |
| `linked-ir` | `[[analysis.ir]]`, run spec | linked-IR JSON and optional pseudo-Rust | render and compare every profile output |
| `navigation-index` | `[analysis.navigation]` | cross-report symbol navigation JSON | render and compare the index |
| `register-validation` | `[registers].model` | nothing | load and validate facts plus model |
| `register-review` | `[registers.review]` | generated Markdown review | render and compare the review |
| `function-validation` | `[functions].pack` | nothing | load and validate selected IR facts plus pack |
| `function-review` | `[functions.review]`, optional validated interface pack | generated Markdown reading view with exact interface call sites | render and compare the review |
| `interface-validation` | `[interfaces].pack` | nothing | load and validate facts, pack, and semantic catalogs |

The pipeline deliberately does not initialize or edit the reviewed register
model, interface pack, or function pack. It also does not export release SVD,
run svd2rust, generate a PAC or publish its binding index. Those are explicit
publication steps because changing a clean hardware interface or reviewed safe
API pack has a different review boundary from refreshing analysis evidence:

Use `project publish` to preflight and write all configured outputs, or
`project publish --check` to verify them without mutation. See
[project publication](project-publication.md). The individual `registers`
commands remain available for inspecting or overriding one output.

This separation prevents an ordinary vendor-artifact refresh from silently
turning inferred names, field partitions, or semantics into a public API.
When a configured function or interface pack is still missing, the human
summary prints the exact `functions init-pack` or `interfaces init-pack` next
step after generating the required facts.

## Dependencies and failure behavior

The four analysis roots run independently. The optional navigation index is a
derived reading view over the configured symbol, IR, and interface roots:

```text
symbol inventory ───────┐
interface discovery ────┼─> navigation index
linked IR ──────────────┘

MMIO discovery ─────┬─> register validation
                    └─> register review <── linked IR (when linked by the project)

interface discovery ──┬─> interface validation
                      └─> function review (when a reviewed pack is present)

linked IR ────────┬─> function validation
                  └─> function review
```

A failure in one root does not hide results from another root. Dependent stages
are not run against stale evidence and are reported as `blocked`. Optional
features that are absent from the manifest are `not-configured` and do not make
the pipeline fail.

The navigation index never feeds analysis or validation back into those
roots. Its dependency arrows mean only that it must not join stale reports.

The default human view renders one compact project summary. JSON and JSONL
serialize the typed report for scripts. Stage status values are:

- `written`: analysis rendered and wrote the configured generated output;
- `verified`: check mode reproduced the exact existing output, or a read-only
  validation succeeded;
- `up-to-date`: write mode proved that the tool, inputs and outputs have the
  same content identities as the last successful execution and skipped it;
- `failed`: the stage ran but analysis, comparison, or validation failed;
- `blocked`: a required input or upstream stage was unavailable;
- `not-configured`: the optional project feature is absent.

In JSON and JSONL modes the data is the typed `project-analysis` report with schema,
`command`, `mode`, `status`, ordered `stages`, reasons and aggregate counts; it
is not encoded as presentation text. Nested command presentation is suppressed
so the project report is the sole stdout result; diagnostics and tracing remain
on stderr. A `failed` or `blocked` stage produces
the normal unsuccessful-result exit status. Detailed configuration parsing
errors that prevent constructing the project at all are reported before the
analysis begins.

In an interactive terminal the active workflow and stage are shown on stderr.
`--progress auto` is the default and is disabled automatically for JSON,
JSONL, redirected stderr, and `--quiet`. Use `--progress always` only when a
caller explicitly wants terminal progress despite those defaults, or
`--progress never` for deterministic silent stderr apart from diagnostics.

## Strict review coverage

By default, workspace validation checks schema, identities, provenance guards,
catalog references, and internal consistency, while allowing discoveries that
have not yet been reviewed. Add `--deny-unreviewed` when review coverage itself
is a gate:

```console
cargo vendor-binary-workbench project analyze --check \
  --project path/to/vendor-project.toml \
  --deny-unreviewed
```

The option applies to register, interface, and function/context validation. It does not
change discovery, IR generation, or review rendering.

## Private inputs

A public project normally omits `run-spec`; callers create an untracked
`local.toml` beside the project manifest with `project inputs init`. The
resolver discovers that file automatically. Explicit `--run-spec` remains an
override for CI or for credentials stored elsewhere. With neither source,
`project analyze --check` reports analysis roots and their dependants as
`blocked`. That is intentionally stricter than `project doctor`, where absent
bindings are only a readiness warning and produce `valid-with-warnings`, not
`invalid`.

The artifact inventory and both discovery commands expose the same
non-mutating primitive for narrow workflows:

```console
cargo vendor-binary-workbench mmio discover \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --check

cargo vendor-binary-workbench interfaces discover \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --check

cargo vendor-binary-workbench symbols inventory \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --check
```

`--check` requires a JSON destination, supplied explicitly or defaulted from
the corresponding project table. A missing or byte-different file fails
without creating directories or changing the existing file.
