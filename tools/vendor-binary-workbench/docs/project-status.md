# Project status and lifecycle readiness

`project status` is the stable read-only inventory of a workbench project. It
separates configuration, private inputs, generated analysis, human review and
publication instead of reducing the whole project to one warning count.

```console
cargo vendor-binary-workbench project status \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The text output is a compact summary intended for terminals. Use a schema-1
JSON report for tools and dashboards:

```console
cargo vendor-binary-workbench project status \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run \
  --json-report generated/reports/project-status.json
```

The report is deterministic for the same project, local bindings and generated
outputs. It may contain caller-local artifact paths, so it normally belongs in
the ignored `generated/` tree rather than in a public target pack.

## Phases

| Phase | Included readiness |
| --- | --- |
| `configuration` | Backend/ABI, platform pack, compiled harness and MMIO map |
| `inputs` | Caller-owned run spec, artifact existence, container readability and symbols |
| `analysis` | Complete symbol inventory, linked-IR/pseudo-Rust profiles, MMIO facts and interface facts |
| `review` | Register coverage and policy packs, interface anchors/slots, functions and context fields |
| `verification` | Parsed scenario profiles, dispositions and the accepted evidence baseline |
| `publication` | Exact current SVD, Rust PAC and binding index derived from reviewed registers |

The status collectors call the same parsers and validators as the individual
workflows. Publication readiness prepares expected outputs in memory and
compares bytes; it never writes SVD or Rust source.
Verification readiness is intentionally public and structural: it establishes
that the policy packs and baseline parse, not that a protected vendor-artifact
run currently matches them. Use `verify inventory` for that gate and
`verify evidence` to review its JSON report.

## States and exit behavior

Components and phases use four states:

- `ready`: all configured evidence for that component is valid and current;
- `incomplete`: a required input/output is missing, stale, or still unreviewed;
- `not-configured`: the optional component does not belong to this project;
- `invalid`: configured data exists but fails parsing, compatibility, or
  semantic validation.

The overall project is `invalid` if any phase is invalid. Otherwise it is
`incomplete` while any configured phase is incomplete; optional
`not-configured` phases do not prevent readiness.

By default, incomplete is a successful informational result while invalid
returns failure. This lets a newly initialized project produce a useful status
report. Add `--deny-incomplete` for a strict lifecycle gate; the command then
returns the workbench's unsuccessful exit status until the overall state is
`ready`.

## Checking a stored report

Status JSON follows the same write/check convention as other generated
evidence:

```console
cargo vendor-binary-workbench project status \
  --project PATH/vendor-project.toml \
  --run-spec PATH/local.run \
  --json-report PATH/generated/reports/project-status.json \
  --check --deny-incomplete
```

`--check` requires `--json-report` and never creates or updates the file. It
fails when the stored document differs. This detects newly discovered MMIO,
new interface slots, stale IR, review regressions, or changed publication
outputs without invoking `project analyze` or `project publish`.

## Status versus doctor

`project doctor` remains the verbose troubleshooting command. It prints exact
input symbol counts, individual linked-IR diagnostics and workspace-specific
errors useful while fixing a project.

`project status` is the smaller automation contract: stable phase names,
component states, structured counts and one overall result. It does not mutate
analysis facts, reviewed packs or publication outputs. Use `project analyze`
to refresh generated evidence and `project publish` to write derived register
artifacts.
