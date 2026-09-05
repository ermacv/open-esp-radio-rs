# Versioned scenario catalog

Folders identify the workload domain: system, IEEE 802.15.4, and IEEE 802.11
station/access-point/roles/monitor. Scenario IDs are stable across folder moves.
Tags select overlapping diagnostic, characterization and qualification uses;
they do not assign ownership or make a hardware-readiness claim.

Both the runner and the independent qualification evaluator discover regular
TOML files recursively. The filename stem must equal the document ID. IDs are
unique throughout the catalog. README.md is the only ignored documentation
filename. Symlinks (including a symlink catalog root), special files, other
file extensions and an empty catalog are rejected. The readers never follow
directory links outside the catalog. Each independently checks its required
schema and repetition bounds; only the runner interprets executable workload
and acceptance fields.

The runner sorts by scenario ID.
`run-all` first traverses `ImageClass::ALL`, then the selected
scenarios of each image in catalog order. Folder traversal order cannot change
physical execution order. Each TOML document owns its image features,
workload criteria and repetition count.

Synthetic serialized compatibility inputs live in `hil/tests/fixtures/catalog`.
They are used by both independent readers and are not part of this catalog.
