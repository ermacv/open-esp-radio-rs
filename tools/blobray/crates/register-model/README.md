# Register model library

`open-esp-radio-register-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities;
- deterministic sparse reviewed-assertion overlays keyed by canonical typed
  chip/address-space/register and register-field identities;
- one explicit sparse `register-identity = "REGION.NAME"` fact for renaming,
  no-op confirmation or materialization in a concrete singular peripheral,
  with retained evidence and applicability;
- retained effective classification, applicability and evidence for every
  applied register, field, access, description and write-semantics claim;
- generic physical-layout and write-semantics invariants;
- reviewed PAC transaction, binding-index and evidence-catalog schemas.

The closed-PAC transaction pack is schema 4 only. Its
`selected-register-writes` operation is an evidence-backed escape hatch for one
exact `u32` volatile image to one non-array 32-bit register which the SVD keeps
read-only. It never rewrites SVD access or accepts a dynamic image; sequencing
and hardware qualification stay above the generated raw helper.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The vendor validator composes it with observed MMIO
facts, the project memory map and target-owned reviewed packs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented in
[`../../docs/registers-and-pac.md`](../../docs/registers-and-pac.md).
An absent physical register is created only by one reviewed
`register-identity = "REGION.NAME"` assertion. The removed
`register-declaration` and `register-name` kinds are explicit errors. The
top-level manifest is schema 3 and declares the stable chip ID and address
space. Assertion subjects use only canonical `register:<chip>/<space>/...` or
`register-field:<chip>/<space>/...` semantic IDs; legacy MMIO strings have no
compatibility parser. The subject must match this model's chip and address
space, have a supported aligned width, fit the named concrete region (and its
register address blocks when present), and not alias or overlap existing
geometry. Identity application is atomic and rejects arrays, clusters,
derived regions, placeholders and noncanonical SVD identifiers. Generated
observations are never consulted for access or modified-write semantics; those
require their own explicit reviewed assertions.

Reusable `[[review]]` metadata is accepted as reviewed coverage only with a
non-empty source list, all three classification fields, and non-`hint`
provenance. Incomplete metadata remains a navigation hint and cannot close a
publication gate. Array coverage is resolved from the structural SVD template,
not by wildcard-matching expanded names.
