# Architecture (normative)

Blobray is a generic, deliberately non-inferential evidence
tool. It extracts, links, and presents observations from caller-supplied
artifacts. It may attach reviewed labels and executable boundary models, but
it must preserve uncertainty and must not promote a hint to hardware truth.

The authoritative observed source is the authenticated vendor artifact bytes
together with ABI, load mapping, and provenance. Persistent linked IR is one
canonical **derived** representation used by downstream analyses,
pseudo-Rust, and executable references. IR records instruction provenance,
uncertainty, and blockers; it is never hardware truth or an independent source
of observed behavior.

## Responsibility matrix

| Subsystem | Responsibility | Owns | Consumes | Produces | Must not know about | Source of truth | Decision |
|---|---|---|---|---|---|---|---|
| Generic Blobray | Extract, link, compare, and present facts | Analysis schemas and fail-closed algorithms | Artifact bytes, composition, add-ons | Observed facts, derived IR, pseudo-Rust, reports | Chip addresses, product policy, ledger state | Artifact bytes + provenance | Keep, simplify |
| Architecture backend | Decode/lift one ISA | ISA semantics, calling-convention implementation | Bytes, relocations, ABI | Instructions, CFG, effects, blockers | Vendors, chips, drivers, ledgers | Artifact bytes + target ABI | Keep |
| Ecosystem pack | Attach reusable vendor/RTOS vocabulary | Declarative operation names/signatures | Generic semantic vocabulary | Hints and reviewed call annotations | Chip addresses, production code | Reviewed pack | Keep, data-only |
| Chip pack/provider | Attach reusable chip facts and lifting hooks | Memory map, SVD inputs, ROM identities, chip summaries | Target ABI, ecosystem vocabulary | Chip-enriched derived analysis | Production driver, qualification policy | Reviewed chip pack; observations remain external | Keep, split |
| Project manifest | Compose one investigation | References, local workflow and output selection | Target, ecosystem packs, chip pack, reviewed workspaces | Resolved project | Reusable chip facts duplicated inline | Manifest only for composition | Simplify |
| Reviewed knowledge | Accept hardware/function meaning after review | Reviewed assertions and links to evidence | Immutable observations and external evidence | Accepted names, fields, enums, contracts | Rewriting underlying observations | Reviewed model | Keep |
| PAC generation | Publish reviewed register structure | Generator and generated-file contract | Reviewed register model | Raw PAC, bindings index | Driver/runtime policy | Reviewed register model | Keep |
| Restricted PAC/capabilities | Encode approved low-level authority | Register-local operations and capability types | Raw PAC + reviewed API policy | Non-forgeable, bounded register authority | Polling, retries, Wi-Fi roles | Reviewed API pack | Strengthen |
| HAL | Implement hardware operations and lifecycle | Sequences, waits, timeouts, recovery, serialization | Narrow capabilities | Hardware operation outcomes | Wi-Fi/BLE/SoftMAC policy | Reviewed sequences + production implementation | Strengthen incrementally |
| Driver | Implement protocol/runtime behavior | Wi-Fi/BLE/SoftMAC state and policy | HAL operations | Production behavior | Raw MMIO/PAC authority | Compiled production Rust | Keep |
| Verification models/add-on | Model environment and compare observations | Scenarios, external services, comparison relation | Compiled vendor and Rust artifacts, inputs, declarations | `MATCH`, `DIFF`, or `INCOMPLETE` evidence | Product readiness decisions | Artifacts + recorded observations | Keep, separate from knowledge |
| Dispositions | Declare reviewed mapping and claim ceiling | Vendor-to-production binding declarations | Function identities and reviewer decisions | Allowed comparison claims | Execution truth | Reviewed declaration, not observation | Keep |
| Qualification ledger | Decide product trust/readiness | Claims, required evidence, readiness policy | Read-only verification/HIL results | Qualification decision | Blobray analysis internals | Ledger | External; never mutated by Blobray |
| Documentation | Explain current contracts and workflow | Normative architecture and operator guidance | Code/schema contracts | Human guidance | Duplicate historical narratives | This file + code/tests | Consolidate |

## Dependency and knowledge direction

```text
artifact bytes + provenance
          |
          v
canonical derived IR ----> facts / pseudo-Rust / executable reference
          |
          v
reviewed assertions (refer to evidence; do not own it)
          |
          v
raw PAC -> restricted capabilities -> HAL -> driver
                                          |
compiled vendor + production Rust --------+-> verification result
                                               |
                                      qualification ledger
```

The schema-3 composition is deliberately layered:

1. `target.toml` owns only architecture and ABI facts;
2. `ecosystem.toml` owns reusable vendor/RTOS semantic catalogs;
3. `chip.toml` owns reusable memory-map, base register geometry, SVD, ROM, and
   only compiled knowledge that is valid across investigations of that chip;
4. `vendor-project.toml` references these inputs and owns composition plus an
   optional investigation-local `analysis-provider`;
5. sparse reviewed-knowledge packs own investigation-specific assertions and
   vendor bugs with stable IDs, evidence and bounded applicability;
6. reviewed project workspaces own the remaining investigation-specific
   contracts while their generated candidates remain disposable.

Pack order is not an override mechanism. Conflicting definitions fail closed.
Two projects may reference the same chip pack without copying its address map.
Likewise, projects for different chips may reference one ecosystem pack. A
generic algorithm or schema belongs in Blobray source; generic is not a dump
for reusable vendor data. ESP-IDF vocabulary and public header-versioned
interface layouts, for example, are data-only ecosystem add-ons, while
ESP32-S31 addresses and compiled summary hooks remain chip knowledge only when
they are revision-stable. Exact artifact roots/digests/runtime guards,
body-identity guards, private callback cells, and Wi-Fi/BLE/802.15.4 artifact
profiles remain investigation-local because they describe the supplied blob
lineage. A project
provider may compose with a chip provider only when its compiled descriptor
explicitly extends that exact reusable root, exposes a contract superset and a
distinct precomposed harness/cache domain. Unrelated providers, transitive
extension chains and contract downgrades fail closed.

## Demand-driven analysis and persistent ownership

There is one analysis engine. Focused inspection and artifact-wide analysis
must never call different recovery algorithms:

```text
request/profile/full scheduler
             |
             v
       immutable query API
             |
     +-------+--------+
     | memory memo    | current process
     | SQLite index   | identity, dependency edges, result location
     | CAS packs      | large immutable serialized values
     +-------+--------+
             |
             v
       fact projections -> generated bundles
```

`full` means enumerating every function identity and requesting the same
queries that focused inspection requests. It is scheduling policy, not a
second analysis implementation. A profile is likewise a set of roots and
projection options; profile names are never cache inputs. Persistence has two
complementary levels. An immutable profile/stage projection and its generated
outputs can be restored from CAS for an identical request. Linked-IR analysis
also persists direct-function facts, so different root sets can reuse facts for
the functions they share instead of repeating every cold function analysis.

A direct-function key binds the exact owner identity and body, relocations and
memory layout. It also includes conservative fingerprints of the resolver
identity/layout, MMIO map and harness semantics used by structural tracing.
Bodies of other functions are not part of that resolver fingerprint, so
changing one function does not invalidate otherwise unchanged owner facts.
This is deliberately conservative rather than an exact per-function dependency
DAG: a resolver, MMIO or harness change may invalidate more facts than the
changed input ultimately affects. Summary-hook providers must supply a stable,
versioned semantic cache domain; a hook context without one is not persistently
cacheable and fails closed to cold analysis.

Query ownership is layered:

| Query unit | Owns | May consume | Must not consume |
|---|---|---|---|
| Artifact catalog | Sections, symbols, relocations, data objects and provenance | Artifact bytes and load mapping | Profiles, reviewed meaning, ledger |
| Function body | Decode, instruction index, CFG and structural blockers | Artifact catalog, target ABI/backend revision | Vendor/chip semantics, profile identity |
| Function facts | Direct calls, memory/MMIO effects, guards and unresolved facts | Function body, exact origin relocations, conservative resolver/MMIO/harness projection | Driver, qualification policy |
| Semantic projection | Reviewed names/signatures and opaque boundary contracts | Function facts plus versioned add-on inputs | Inventing effects not authorized by a contract |
| Link projection | Cross-function targets and transitive summaries | Function facts, artifact catalog, semantic projection | Profile presentation choices |
| Profile projection | Root/reachability selection and generated documents | Cached query results | Re-running backend recovery |

A query key contains the query kind and semantic revision, the digest and
provenance identity of the artifact bytes it owns, target ABI/backend identity,
and the semantic fingerprints required by that query domain. It never contains
output paths, profile names, timestamps, UI state, reviewed ledger state, or
production-driver state. Results are immutable: changed inputs create a new
key instead of mutating the meaning of an old result. Indexed dependency edges
describe stage ownership, while direct-function facts use the conservative
projection above rather than claiming exact dependency-level invalidation.
Incomplete results remain cacheable; a blocker is a valid fact, not a cache
failure.

The persistent store is disposable Blobray state under
`generated/.blobray-cache/`. SQLite in WAL mode owns the indexed query DAG,
atomic bindings and content locations. Values up to 64 KiB are inline; larger
query values are deduplicated into append-only SHA-256 CAS pack generations. The
measured boundary and retention policy are documented in
[`cache-policy.md`](cache-policy.md). Generated
output files are always streamed into the same CAS and may be atomically
restored; generated IR bundles remain publication artifacts and are not the
cache database. Reviewed packs and the qualification ledger must never be
copied into or modified by the store. In schema 9, every persisted query result
belongs to at least one analysis epoch; an unowned result is invalid cache state
rather than a GC candidate. Query results are never selected by per-result LRU
or filesystem mtime.

Schema 9 is created only from a cold store. Blobray does not import, upgrade or
accept an older cache schema. After preserving reviewed inputs and durable
generated artifacts, recovery requires explicit removal of the entire
`generated/.blobray-cache/` directory and a fresh analysis.

Pack lifetime is reachability-based with explicit retention roots. Analysis
epoch memberships, query results, stage-output bindings and timestamped retired
objects are preserved.
When unprotected unreachable records exceed the bounded compaction
threshold, the store streams all live objects into a new immutable generation,
fsyncs it, atomically redirects SQLite, and only then removes old generations.
A crash can therefore leave an unreferenced old or new pack, but never an
index that points at partially written bytes; unreferenced generations are
removed when the store reopens.

Mutable symbolic continuations are intentionally not persistent query values.
They are expensive to clone, couple the store to backend internals and do not
form stable facts. The CFG explorer caches completed immutable results and
replays bounded decision maps; MMIO discovery and linked-IR construction use
the same explorer and the same typed limit reasons.

Reference-call composition has a separate bounded worker-local memo. Its key
is the exact linked target plus the complete RV32 symbolic argument array.
Both eligible and completed ineligible traces are immutable reusable results;
an ineligible cache hit must return the same fail-closed blocker. A result
whose cause contains recursion is visiting-stack-dependent and is never
memoized. The memo has a fixed entry ceiling and is discarded with its worker;
it is an execution optimization, not evidence and not a replacement for the
persistent direct-function facts described above.

## Rust ownership and capability rules

Ownership transfer, borrowing, and capability passing express hardware
authority. `split()`, `join()`, and `free()` are API choices, not mandatory
patterns. Capability boundaries follow actual exclusive/shared access needs,
not the physical SVD block layout.

- HAL must not publicly re-export a PAC owner or unrestricted register type.
- Code above HAL must not obtain an equivalent owner, including through an
  arena, facade, `Deref`, callback, or generic `with_mut` escape hatch.
- A shared mutable hardware capability names its serialization owner. A
  cloneable handle never implies unsynchronized MMIO authority.
- Multi-register sequences, polling, delays, retry limits, and recovery belong
  to HAL. Register-local fields, masks, and enum encodings belong below it.
- Protocol roles and runtime policy belong to the driver and never flow back
  into register knowledge.

Migration is vertical. The channel transaction uses a narrow borrowed
`RadioChannelHal`; this is a current slice API, not a universal split pattern.
The runtime arena stores only an opaque `RadioRuntimeOwner` and cannot yield a
PAC owner. Cold MAC, channel, DMA, IRQ, TX, AP, and STA paths now consume named
HAL operations. Powered PHY code borrows an opaque `PhyHal` with no `Deref`,
generic callback, or owner-recovery operation. PHY has no PAC dependency and
can use the capability only through named HAL operations. Repository contracts
reject the removed broad borrow APIs and any future `Deref` escape.

## Verification and qualification

Device/semantic models may describe the environment or a bounded relation;
they are not production implementations. A verification-relevant comparison ends
at compiled production Rust. Dispositions can declare what is bound and what
claim is allowed, but cannot change recorded behavior.

Blobray owns neither ledger types nor readiness policy and cannot mutate a
ledger. A future UI may display a read-only result produced by the independent
`qualification-check` tool; Blobray must not parse policy or calculate
readiness itself. An implemented function without a qualifying production
trace remains visible research coverage debt; it does not fail `project
status` or `project verify` unless a configured policy, suite, or binding
requirement makes that trace mandatory.

## Documentation policy

This file is normative for ownership and dependency boundaries.
`project-workflow.md` is normative operator workflow. `formats.md` is the
schema index. Other retained files explain one subsystem. Generated CLI help,
reports, PAC/SVD output, and manpages are generated documentation. Git history,
not checked-in migration narratives, is the historical archive.

The completed binding cutover has no compatibility manifest: current
dispositions, profiles, suites, and repository contract tests are the only
maintained representation.
