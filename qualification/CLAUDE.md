# qualification/

The readiness authority: reviewed capability declarations plus independent
evidence decide whether a selected program is ready. Producers (Blobray, HIL)
never decide it. Reference: [README](README.md).

| Path | Owns |
| --- | --- |
| `catalog/<chip>/*.toml` | Canonical capability declarations, source facts and inventories |
| `targets/<chip>/*.toml` | Programs: selected capabilities, HIL requirements, evidence directories |
| `evaluator/` | `oer-qualification`, the evaluator behind `cargo qualification` |

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
- Requirements name HIL scenarios and named checks, never duplicate
  thresholds ([HIL requirements](README.md#hil-requirements-and-scenario-roles)).
- Catalogs are large: grep for the capability ID, then read that entry.

## Commands (in the background)

- `cargo qualification status --manifest qualification/targets/esp32s31/wifi-sta.toml --capability <id>`
- `cargo qualification validate --manifest qualification/targets/esp32s31/wifi-sta.toml`
- `cargo xtask check capabilities` and `cargo xtask check docs` after catalog changes.

Workflow: the `qualification-entry` skill.
