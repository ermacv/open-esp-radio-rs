# Register model library

`oer-register-model` owns the portable register publication formats:

- safe loading of a multi-file TOML model;
- native unreviewed initialization from explicit peripheral geometry and import
  of CMSIS-SVD standard declarations; the source tool retains original inputs;
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

The raw PAC is generated output only. The PAC pack has no module declarations
for hand-written code; ownership and validation transactions that need raw
register access live in the closed parent PAC.

The closed-PAC transaction pack is schema 5 only. Its
`w1c-register-snapshots` operation binds one non-array 32-bit W1C register field
to an affine sample token. The token has no public constructor, is not cloneable
and is consumed by the exact same-register acknowledgement; no caller-built
clear image crosses this boundary. Sequencing and hardware qualification stay
above the generated raw helper.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
contents or output paths. The [register tool](../README.md) composes it with
explicitly selected reviewed publication inputs. RTOS, NVS,
logging and delay semantics remain outside this crate.

The format and editing workflow are documented by the
[register tool](../README.md).
An absent physical register is created only by one reviewed
`register-identity = "REGION.NAME"` assertion; the
`register-declaration` and `register-name` kinds are rejected with an error. The
top-level manifest is schema 3 and declares the stable chip ID and address
space. Assertion subjects use only canonical `register:<chip>/<space>/...` or
`register-field:<chip>/<space>/...` semantic IDs; other subject spellings are
rejected. The subject must match this model's chip and address
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

A register or field review also records where its name comes from with
`naming`:

| `naming` | Meaning | Requirement |
| --- | --- | --- |
| `vendor` | The vendor's own name | Cites the header, SVD or manual in `sources` |
| `descriptive` | A name this project assigned from established behavior | Published description starts with "Project-assigned name." |
| `opaque` | The meaning is not established | The name ends in `_OPAQUE`; published description starts with "Opaque: meaning not established." |

Every register and field carries `naming`. An `_OPAQUE` name and
`naming = "opaque"` require each other, and no name spells unknown meaning as
`UNKNOWN` or `UNNAMED`, so an unreviewed name cannot claim or hide it. `naming` applies only
to registers and fields. The SVD description carries the marker, so raw PAC
documentation shows it too.
