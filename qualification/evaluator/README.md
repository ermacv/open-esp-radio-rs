# Qualification evaluator

This Cargo package independently evaluates the capability programs
in [qualification](../README.md). Its package name is
`open-esp-radio-qualification-check`; the repository entrypoint is
`cargo qualification`.

| Operation | Result |
| --- | --- |
| `validate --manifest PATH` | Reject malformed or inconsistent inputs; a valid incomplete program may pass |
| `evaluate --manifest PATH` | Derive readiness axes and optionally write `--json-report PATH` |
| `gate --manifest PATH` | Fail unless every required capability and dependency is ready |
| `catalog check --manifest PATH` | Resolve selected canonical declarations and run the same fail-closed validation/evaluation |
| `catalog render --manifest PATH --out DIRECTORY` | Write a deterministic ignored Markdown inventory from the resolved evaluator result |

Schema-4 programs declare implementation, host and async status. Vendor/HIL
status is derived from independently checked external evidence. Declarations
must agree with blockers; a declared host status does not attest a test run.

The evaluator reads minimal serialized projections of HIL catalogs and sealed
bundles, then checks their identity, integrity, repetitions and provenance.
It does not import the HIL runner or Blobray verdict implementation. Shared
synthetic [catalog fixtures](../../hil/tests/fixtures/catalog/README.md) describe
interoperability; readers retain separate validation logic.

Unit tests live beside their private modules. No source-name regex is used to
turn Rust symbol spelling into an ownership or execution proof.

Capability catalog schema 1 stores schema-4 capability records without a
second status model. The resolver rejects unsafe catalog paths, symlink path
components, unsupported schemas, duplicate declarations and unknown selected
IDs. Structured chip, role, PHY, security, composition, capability level,
activation and limitation scope is required for each catalog record.
Catalog-owned dependencies are included transitively before the existing
exact-set and dependency-cycle checks run.

HIL qualification requires build provenance for every firmware artifact. The
primary source must match the current clean repository, and the recorded
workspace lockfile must match its pinned composition. Local source overrides
qualify only when clean, reconstructable and at the locked revisions of every
package they replace. Missing provenance, dirty or unpinned overrides, and
firmware replay remain diagnostic evidence without establishing qualification.
