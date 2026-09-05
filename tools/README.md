# Repository tools

This tree is limited to repository-wide generators and policy checks:

| Path | Purpose |
| --- | --- |
| `memory-report/` | Target-neutral ELF memory ownership, placement policy and before/after analysis |
| `blobray/crates/register-model/` | Shared register model, clean SVD, PAC/evidence schemas and generic invariants |
| `qualification-check/` | Fail-closed readiness manifest validation |
| `blobray/` | Blobray implementation, reusable backends, models and platform harnesses |
| `audit-driver-safety.sh` | Unsafe-code boundary policy |
| `audit-driver-architecture.sh` | Compile supported feature profiles, inspect Cargo ownership boundaries, run composition tests |
| `audit-cargo-metadata.sh` | Locked metadata audit for every tracked Cargo workspace island |
| `check-network-adapter-boundaries.sh` | Compile network variants and validate resolved package/source boundaries; `--check-dependencies-only` runs the graph policy used by source-only |
| `check-esp32s31-examples.sh` | Type-check four independent target examples and the compatibility-network station variant |
| `blobray/scripts/check-standalone` | Manually extract and compile generic Blobray as an independent workspace |
| `blobray/scripts/run-limited` | Launch actual analysis with process-tree memory and runtime limits |
| `audit-source-only.sh` | Production dependency and final-image source-only audit (including locked Cargo metadata); batches compatible all-feature lint policies, overlaps the isolated optimized HIL image build with workspace audits, validates register sources without requiring disposable vendor-analysis reports and checks publication reproducibility when those reports are present |

Hardware scenarios and privileged network fixtures belong under `hil/host`,
not in this directory. Chip target packs belong under `verification/` and
readiness claims belong under `qualification/`.

The [shell-script audit](../docs/SHELL_SCRIPT_AUDIT.md) covers all repository
shell entrypoints, including HIL provisioning and vendor probe builds. Cargo
graph policies establish package boundaries; compiler checks and ownership
tests cover typed behavior. Searching Rust source text for type names is not
an architecture check.

The network checker uses Python 3.11+ (`tomllib`) and Cargo itself to resolve
temporary consumers. It keeps repository lockfiles intact and rejects resolved
versions outside their pinned catalog. Run the tool regression tests with
`python3 -B -m unittest discover -s tools/tests -p 'test_*.py'`.
