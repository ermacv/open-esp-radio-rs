# Project register publication

`project publish` is the release boundary between a reviewed register project
and its derived public artifacts. It does not analyze vendor binaries and does
not require a local run spec:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Use the non-mutating form in CI:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

## Boundary

Publication consumes only the checked project manifest, register model,
optional API/lint/evidence packs, and memory map. It never consumes ELF files,
archives, linked IR, interface semantics or a platform harness.

The command always enforces the equivalent of `registers validate
--deny-unreviewed`; release publication cannot weaken review coverage through
a command-line option. Configured outputs are then derived from the same
schema-2 model:

| Stage | Configuration | Output |
| --- | --- | --- |
| `register-validation` | `[registers]`, optional API/lint/evidence packs and memory map | read-only validation |
| `svd-publication` | `[registers.svd]` | clean release SVD |
| `pac-raw-publication` | `[registers.pac-raw]`, optional `[registers.api]` | internal formatted `svd2rust` source and reviewed low-level transactions |
| `pac-api-publication` | `[registers.api].pack` plus `[registers.api].output` | closed public value domains with no integer constructors |
| `binding-publication` | `[registers.bindings]` | address-to-PAC-path index |

An absent output table is reported as `not-configured`, not as a failure. This
allows SVD-only and SVD-plus-PAC projects without inventing unused paths. A
missing `[registers]` workspace is an error because there is no publication
root. Configured publication paths must be distinct and cannot overwrite the
register model, facts, review inputs, API/lint packs or evidence catalogs.

## Preflight and failure behavior

After validation, every configured output is rendered in memory before the
first output is written. If PAC formatting, binding generation, or another
preparation step fails, all other prepared write stages are reported as
`blocked` and no generated output is changed. Filesystem failures during the
subsequent writes are still reported normally; a filesystem cannot provide a
portable multi-directory transaction.

`--check` never writes. It compares each prepared artifact byte-for-byte with
the configured file. Missing and stale files fail with the same diagnostics as
the corresponding individual `registers ... --check` command.

The default human view is a compact stage summary. `--format json` emits the
typed `project-publication` report containing the
ordered stages and aggregate counts. Stage statuses are `written`, `verified`,
`failed`, `blocked`, and `not-configured`; any failed or blocked stage makes
the process unsuccessful. Output from the
nested validation and generation commands is intentionally suppressed at this
composition boundary; diagnostics and tracing still go to stderr.
Interactive runs also show the active publication stage on stderr under the
global `--progress auto|always|never` policy; this never changes the typed
stdout report.

## Relationship to other commands

`project analyze` and its `--check` mode own generated reverse-engineering
evidence: MMIO facts, interface facts, linked IR and review reports. They
intentionally cannot publish a public hardware API. `project publish` owns
reviewed register outputs and intentionally cannot refresh evidence from
proprietary inputs.

The individual `registers validate`, `export-svd`, `generate-pac-raw`, and
`generate-bindings` commands remain useful for debugging one stage, selecting
an audit SVD profile, disabling an API pack for inspection, or overriding one
output path. Project CI and release checks should prefer `project publish
--check` so no configured artifact is accidentally omitted.
