# Register model library

`open-esp-radio-register-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities;
- generic physical-layout and write-semantics invariants;
- reviewed PAC transaction, binding-index and evidence-catalog schemas.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The vendor validator composes it with observed MMIO
facts, the project memory map and target-owned reviewed packs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented in
[`../vendor-code-validator/docs/register-workspace.md`](../vendor-code-validator/docs/register-workspace.md).
