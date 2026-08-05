# Repository tools

This tree is limited to repository-wide generators and policy checks:

| Path | Purpose |
| --- | --- |
| `pac-gen/` | Reproducible radio PAC generation from checked SVD sources |
| `qualification-check/` | Fail-closed readiness manifest validation |
| `vendor-code-validator/` | Transitional home of the generic vendor verification engine and CLI |
| `audit-driver-safety.sh` | Unsafe-code boundary policy |
| `audit-source-only.sh` | Production dependency and final-image source-only audit |

Hardware scenarios and privileged network fixtures belong under `hil/host`,
not in this directory. Chip target packs belong under `verification/` and
readiness claims belong under `qualification/`.
