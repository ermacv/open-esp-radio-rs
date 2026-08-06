# Repository tools

This tree is limited to repository-wide generators and policy checks:

| Path | Purpose |
| --- | --- |
| `register-model/` | Shared register model, clean SVD, PAC/evidence schemas and generic invariants |
| `qualification-check/` | Fail-closed readiness manifest validation |
| `vendor-binary-workbench/` | Vendor Binary Workbench implementation, reusable backends, models and platform harnesses |
| `audit-driver-safety.sh` | Unsafe-code boundary policy |
| `audit-source-only.sh` | Production dependency and final-image source-only audit |

Hardware scenarios and privileged network fixtures belong under `hil/host`,
not in this directory. Chip target packs belong under `verification/` and
readiness claims belong under `qualification/`.
