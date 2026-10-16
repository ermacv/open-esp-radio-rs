# qualification/

The readiness authority: reviewed capability declarations plus independent
evidence decide whether a selected program is ready. Producers (Blobray, HIL)
never decide it. Reference: [README](README.md).

| Path | Owns |
| --- | --- |
| `catalog/<chip>/*.toml` | Canonical capability declarations, source facts and inventories |
| `targets/<chip>/*.toml` | Programs: selected capabilities, HIL requirements |
| `evaluator/` | `oer-qualification`, the evaluator behind `cargo qualification` |
| `evidence-shard/` | Root-workspace vendor evidence shard format, validation, currency and reader/writer ([README](evidence-shard/README.md)) |
| `evidence/archive.toml` | The pin of the HIL evidence archive version qualification reads (`cargo hil archive publish` writes it; `cargo qualification check references` checks it offline) |

## Rules

- A program declares no capability itself; it selects catalog IDs
  ([imports and required sets](README.md#imports-and-required-sets)).
- An entry that claims support (`implemented`, `partial`, `fail-closed`, or
  `implementation = "complete"`) has a `// CAPABILITY: <id>` anchor on its
  owning production item; keep the entry's status and the code in step
  ([code anchors](README.md#code-anchors)).
- `implementation`, `host` and `async` are reviewed declarations; `vendor` and
  `hil` are derived from evidence. Never write a readiness claim in prose or
  a dated `PASS` table.
- Requirements name HIL scenarios and named checks, never duplicate a
  scenario's thresholds ([HIL requirements](README.md#hil-requirements-and-scenario-roles));
  acceptance bounds on observed measurements live in the requirement
  ([measurement bounds](README.md#measurement-bounds)).
- Catalogs are large: grep for the capability ID, then read that entry.

## Commands (in the background)

- `rm -rf target/hil/evidence-archive && cargo hil archive fetch --out target/hil/evidence-archive` (network; `fetch` never writes over a copy), then `cargo qualification status --manifest qualification/targets/<chip>/<program>.toml --archive target/hil/evidence-archive --capability <id>`; without `--archive` no HIL run is read (`INPUT hil-archive=none`) and no HIL obligation is satisfied
- `cargo qualification check references`: the pin and the cited runs, offline
- `cargo qualification validate --manifest qualification/targets/<chip>/<program>.toml`
- `cargo xtask check capabilities` and `cargo xtask check docs` after catalog changes.

Workflow: the `qualification-entry` skill.
