# Repository tools

This tree is limited to repository-wide generators and policy checks:

| Path | Purpose |
| --- | --- |
| `memory-report/` | Target-neutral ELF memory ownership, placement policy and before/after analysis |
| `register-model/` | Shared register model, clean SVD, PAC/evidence schemas and generic invariants |
| `qualification-check/` | Fail-closed readiness manifest validation |
| `blobray/` | Blobray implementation, reusable backends, models and platform harnesses |
| `audit-driver-safety.sh` | Unsafe-code boundary policy |
| `audit-cargo-metadata.sh` | Locked metadata audit for every tracked Cargo workspace island |
| `audit-source-only.sh` | Production dependency and final-image source-only audit (including all locked Cargo metadata) |

Hardware scenarios and privileged network fixtures belong under `hil/host`,
not in this directory. Chip target packs belong under `verification/` and
readiness claims belong under `qualification/`.
