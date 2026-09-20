# Qualification evaluator

This Cargo package independently evaluates the capability programs
in [qualification](../README.md). Its package name is
`open-esp-radio-qualification-check`; the repository entrypoint is
`cargo qualification`.

| Operation | Result |
| --- | --- |
| `status (--catalog PATH ... \| --manifest PATH) [--capability ID] [--json-report PATH]` | Show declarations or saved observations, owners, knowledge links, limits and next work without an aggregate gate |
| `next (--catalog PATH ... \| --manifest PATH) [--capability ID] [--json-report PATH]` | Explain work candidates in the same map; never execute checks or infer cross-image evidence transfer |
| `validate --manifest PATH` | Reject malformed or inconsistent inputs; a valid incomplete program may pass |
| `evaluate --manifest PATH` | Derive readiness axes and optionally write `--json-report PATH` |
| `gate --manifest PATH` | Fail unless every required capability and dependency is ready |
| `catalog check --catalog PATH` | Statically validate every catalog declaration without evidence outputs or HIL runs |
| `catalog check --manifest PATH` | Statically validate catalogs, program selection, dependency closure and the declared required-set policy without evidence outputs |
| `catalog render --catalog PATH --out DIRECTORY` | Write deterministic static domain and declaration views |
| `catalog render --manifest PATH --out DIRECTORY` | Write static views plus a separate evaluator-derived program view |

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

Capability catalog schema 2 stores schema-4 qualification declarations and a
wider source-owned domain inventory. Static validation rejects unsafe/symlink
paths, unsupported schemas, duplicate declarations, invalid unselected
records, missing dependencies, cycles, inconsistent axes, invalid source
contracts and unknown disposition/HIL references. Structured chip, role, PHY,
security, composition, capability level, activation and limitation scope is
required for each qualification record. Program resolution includes selected
catalog dependencies transitively before checking the required set. The explicit
`required-capabilities-from = "catalog-closure"` policy derives that set from
catalog roots and rejects mixed explicit IDs or inline declarations. Programs
without this policy still require the exact `required-capabilities` list.

Catalog source facts provide a narrow reuse mechanism for exact matching source
scopes. A fact owns one status, level and source contract; inventory projections
and catalog capability `source-fact-refs` cannot override it. Related or broader
scopes remain independent declarations, and an implemented fact does not
promote a parent capability or any readiness axis. Source-only domain catalogs
may own facts without adding qualification capabilities; consumers must load
those catalogs through explicit root selections or repository-relative `imports`.
Imports resolve transitively with cycle detection and shared-input deduplication;
all imported sources retain hashes. Imported capabilities are validated even if
unselected. Both required-set policies use the same capability and evidence
validation; closure mode never drops a dependency to make a product ready.

By default, HIL qualification requires build provenance for every firmware artifact. The
primary source must match the current clean repository, and the recorded
workspace lockfile must match its pinned composition. Local source overrides
qualify only when clean, reconstructable and at the locked revisions of every
package they replace. Missing provenance, dirty or unpinned overrides, and
firmware replay remain diagnostic evidence without establishing qualification.

Explicit [property-scoped applicability reviews](../evidence-reviews.md) can admit
previously excluded observations for a selected destination build after checking
owner inputs and build/property identities. They also encode individual failure
dispositions. This opt-in path is shared by qualification and the engineering
map; it preserves original observations and never reruns hardware automatically.
