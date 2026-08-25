# Register model library

`open-esp-radio-register-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities;
- deterministic sparse reviewed-assertion overlays keyed by address and bit
  range;
- explicit sparse `register-declaration` materialization into an existing
  non-array peripheral, with retained evidence and applicability;
- generic physical-layout and write-semantics invariants;
- reviewed PAC transaction, binding-index and evidence-catalog schemas.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The vendor validator composes it with observed MMIO
facts, the project memory map and target-owned reviewed packs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented in
[`../../docs/registers-and-pac.md`](../../docs/registers-and-pac.md).
The read-only split planner and unchanged-effective-output migration process
are documented in
[`../../docs/register-migration.md`](../../docs/register-migration.md).

An absent physical register is created only by a reviewed
`register-declaration = "PERIPHERAL"` plus a `register-name` assertion for the
same MMIO subject and effective applicability. The subject must match this
model's address space, have a supported aligned width, fit the named concrete
region (and its register address blocks when present), and not alias or overlap
existing geometry. Declaration application is atomic. Generated observations
are never consulted for access or modified-write semantics; those require
their own explicit reviewed assertions.
