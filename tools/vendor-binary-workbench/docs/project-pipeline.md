# Project analysis pipeline

`project analyze` is the reproducible, project-wide entry point for generated
reverse-engineering evidence. Its `--check` mode performs the same analyses
but only compares their rendered results with existing files. It never updates
an output.

```console
cargo vendor-binary-workbench project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run

cargo vendor-binary-workbench project analyze --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

Use `project doctor` first when diagnosing configuration, backend, catalog, or
private-artifact readiness. Use write mode after changing inputs or the
analyzer, and `project analyze --check` in CI when generated evidence is
retained.

## Pipeline boundary

The pipeline owns only reproducible evidence and read-only validation:

| Stage | Project configuration | Writes in default mode | Behavior with `--check` |
| --- | --- | --- | --- |
| `symbol-inventory` | `[analysis.symbols]`, run spec | complete symbol/linkage facts JSON | render and compare facts |
| `mmio-discovery` | `[registers].facts`, memory map, run spec | MMIO facts JSON | render and compare facts |
| `interface-discovery` | `[interfaces].facts`, run spec | interface facts JSON | render and compare facts |
| `linked-ir` | `[[analysis.ir]]`, run spec | linked-IR JSON and optional pseudo-Rust | render and compare every profile output |
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

## Dependencies and failure behavior

The four analysis roots run independently:

```text
symbol inventory ─────────> immutable navigation/linkage facts

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

Each stage emits a stable, single-line status:

```text
PROJECT-STAGE name=mmio-discovery status=verified reason=-
PROJECT-STAGE name=register-review status=blocked reason=linked-ir did not complete
```

The actual output uses tab separators. Status values are:

- `written`: analysis rendered and wrote the configured generated output;
- `verified`: check mode reproduced the exact existing output, or a read-only
  validation succeeded;
- `failed`: the stage ran but analysis, comparison, or validation failed;
- `blocked`: a required input or upstream stage was unavailable;
- `not-configured`: the optional project feature is absent.

The final `PROJECT-ANALYSIS` line aggregates all stages. In JSON and JSONL
modes the same data is a typed `project-analysis` record with schema,
`command`, `mode`, `status`, ordered `stages`, reasons and aggregate counts; it
is not encoded as presentation text. A `failed` or `blocked` stage produces
the normal unsuccessful-result exit status. Detailed configuration parsing
errors that prevent constructing the project at all are reported before the
analysis begins.

## Strict review coverage

By default, workspace validation checks schema, identities, provenance guards,
catalog references, and internal consistency, while allowing discoveries that
have not yet been reviewed. Add `--deny-unreviewed` when review coverage itself
is a gate:

```console
cargo vendor-binary-workbench project analyze --check \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.run \
  --deny-unreviewed
```

The option applies to register, interface, and function/context validation. It does not
change discovery, IR generation, or review rendering.

## Private inputs

A public project normally omits `run-spec`; callers supply an untracked file
with authenticated artifact paths. In that case, running `project analyze
--check` without `--run-spec` reports analysis roots and their dependants as
`blocked`. That is intentionally stricter than `project doctor`, where an
omitted local run spec is only a readiness warning.

The artifact inventory and both discovery commands expose the same
non-mutating primitive for narrow workflows:

```console
cargo vendor-binary-workbench mmio discover \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.run \
  --check

cargo vendor-binary-workbench interfaces discover \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.run \
  --check

cargo vendor-binary-workbench symbols inventory \
  --project path/to/vendor-project.toml \
  --run-spec /path/to/local.run \
  --check
```

`--check` requires a JSON destination, supplied explicitly or defaulted from
the corresponding project table. A missing or byte-different file fails
without creating directories or changing the existing file.
