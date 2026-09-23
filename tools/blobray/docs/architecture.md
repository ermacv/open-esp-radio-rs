# Architecture (normative)

Blobray records evidence and derives explicitly qualified hypotheses.
It extracts, links, and presents observations from caller-supplied
artifacts. It may attach reviewed labels and executable boundary models, but
it must preserve uncertainty and must not promote a hint to hardware truth.

The authoritative observed source is the authenticated vendor artifact bytes
together with ABI, load mapping, and provenance. Persistent linked IR is one
canonical **derived** representation used by downstream analyses,
pseudo-Rust, and executable references. IR records instruction provenance,
uncertainty, and blockers; it is never hardware truth or an independent source
of observed behavior.

## Responsibility matrix

| Subsystem | Responsibility | Owns | Consumes | Produces | Must not know about | Source of truth |
|---|---|---|---|---|---|---|
| Generic Blobray | Extract, link, compare, and present facts | Analysis schemas and fail-closed algorithms | Artifact bytes, composition, add-ons | Observed facts, derived IR, pseudo-Rust, reports | Chip addresses, product policy, ledger state | Artifact bytes + provenance |
| Architecture backend | Decode/lift one ISA | ISA semantics, calling-convention implementation | Bytes, relocations, ABI | Instructions, CFG, effects, blockers | Vendors, chips, drivers, ledgers | Artifact bytes + target ABI |
| Ecosystem pack | Attach reusable vendor/RTOS vocabulary | Declarative operation names/signatures | Generic semantic vocabulary | Hints and reviewed call annotations | Chip addresses, production code | Reviewed pack |
| Chip pack/knowledge | Declare reusable chip facts | Memory map, SVD inputs, ROM identities, ABI and semantic declarations | Target ABI, ecosystem vocabulary | Reviewed interpretation inputs | Handwritten control flow, production driver, qualification policy | Reviewed chip pack; observations remain external |
| Executable analysis models | Interpret external boundaries or temporarily reconstruct selected bodies | Runtime adapters, guarded summary hooks, implementation provenance | Knowledge/contracts and authenticated applicability context | Modeled effects with explicit uncertainty | Hardware truth or qualification policy | Reviewed model implementation, distinct from observations |
| Project manifest | Compose one investigation | References, local workflow and output selection | Target, ecosystem packs, chip pack, reviewed workspaces | Resolved project | Reusable chip facts duplicated inline | Manifest only for composition |
| Reviewed knowledge | Accept hardware/function meaning after review | Reviewed assertions and links to evidence | Immutable observations and external evidence | Accepted names, fields, enums, contracts | Rewriting underlying observations | Reviewed model |
| PAC generation | Publish reviewed register structure | Generator and generated-file contract | Reviewed register model | Raw PAC, bindings index | Driver/runtime policy | Reviewed register model |
| Restricted PAC/capabilities | Encode approved low-level authority | Register-local operations and capability types | Raw PAC + reviewed API policy | Non-forgeable, bounded register authority | Polling, retries, Wi-Fi roles | Reviewed API pack |
| HAL | Implement hardware operations and lifecycle | Sequences, waits, timeouts, recovery, serialization | Narrow capabilities | Hardware operation outcomes | Wi-Fi/BLE/SoftMAC policy | Reviewed sequences + production implementation |
| Driver | Implement protocol/runtime behavior | Wi-Fi/BLE/SoftMAC state and policy | HAL operations | Production behavior | Raw MMIO/PAC authority | Compiled production Rust |
| Verification models/add-on | Model environment and compare observations | Scenarios, external services, comparison relation | Compiled vendor and Rust artifacts, inputs, declarations | `MATCH`, `DIFF`, or `INCOMPLETE` evidence | Product readiness decisions | Artifacts + recorded observations |
| Dispositions | Declare reviewed mapping and claim ceiling | Vendor-to-production binding declarations | Function identities and reviewer decisions | Allowed comparison claims | Execution truth | Reviewed declaration, not observation |
| Qualification ledger | Decide product trust/readiness | Claims, required evidence, readiness policy | Read-only verification/HIL results | Qualification decision | Blobray analysis internals | Ledger |
| Documentation | Explain current contracts and workflow | Normative architecture and operator guidance | Code/schema contracts | Human guidance | Duplicate historical narratives | This file + code/tests |

## Dependency and knowledge direction

The project analysis application owns a `CapturedSourceSet` for binary stages.
It captures declared run-input paths when the first uncached binary stage
requires them. Symbol inventory, MMIO discovery, interface discovery and all
selected IR profiles borrow this set through their analysis and export APIs.
Standalone commands construct a set for their declared inputs. Those APIs
reject a path absent from the set; they cannot silently open it themselves.

Each capture owns immutable bytes and a content digest. Its lazy backend
catalog shares those bytes without copying the container. Failed reads and
failed binary parses remain associated with the captured path and are not
retried during the run. A binary parse failure does not discard the captured
raw bytes. Rendering uses captured digests and data objects, so it cannot
combine analyzed code with a later version of a file.

A project session also owns the immutable `InterfaceFacts` query capture.
Snapshot and revision queries borrow that capture; reviewed interface bindings
are constructed over the same allocation. A missing or invalid reviewed pack
does not remove raw observations from the workspace snapshot. Loading errors
remain diagnostics. The first observation load, including a missing-file or
parse-error result, is retained for the session lifetime. Explicit application
reload opens a new session; existing snapshots retain their captured facts.
This capture does not atomically freeze the reviewed pack or other workspaces.
`BlobrayApplication` opens a query session without constructing the backend
register catalog. It retains the full memory map, including 64-bit ranges.
Analysis, comparison and project-analysis planning prepare a separate session
from the already resolved declarations and explicit overrides. This preparation
owns the backend catalog and input guards; unsupported addresses fail explicitly
without installing an empty catalog. Successful preparation is retained until
reload. Reload replaces the query session and clears the prepared session and
analysis cache only after resolution succeeds. Model preparation does not freeze
all project inputs into one snapshot.

A session retains its first register inventory capture, including a failed load,
until reload. List, detail, research and revision use that same graph. The public
`RegisterInventorySnapshot` owns the graph and its content ID; retaining its
`Arc` keeps the graph available after reload. Publication inspection captures the
model, discovery facts, review scopes and selected assertions once alongside the
inventory, preserving separate load failures. The inventory ID describes only the
graph, not those publication inputs. Workspace generations distinguish reloads
even when the graph is unchanged. Capture currently reads these inputs in
sequence; it is not an atomic transaction across their files.

Register inventory queries accept the resolved `ProjectSession`, including its
SVD selection and published artifact owner. Selected SVD bytes use the shared
`CapturedSourceSet` during initial inventory capture, including opaque invalid
text and explicit read failures. Discovery and linked-IR records come from the
selected published epoch; deleting their exports does not change the graph.
Missing publication, missing members and malformed records retain explicit gaps.
Malformed MMIO payloads are retained as opaque evidence from the selected epoch.
The inventory builder has no query-store writer and does not publish on a cache
miss. Its derived graph is retained in memory by the session; the former
file-fingerprinted persistent inventory cache is removed. Models, reviewed
inputs and external observations still have separate file capture lifetimes.
Plan/statistics readers retain their separate exclusion lock.
Interface calls, assignments and decode diagnostics retain their physical code
owner through discovery and query. Function review consumes this identity in
both compact and full queries and uses it to associate interface callers.
Relocated roots carry a captured symbol reference or an explicit unknown reason.
Argument roots belong to their physical function owner. Navigation groups captured
symbols by occurrence and preserves every display label; relocated roots use the
same occurrence lookup. Reviewed root selectors and project-call reachability
still use separate association logic. Neither a navigation link nor a physical
caller identity proves linker selection or target behavior.

Publication input guards remain separate: they check whether current files
still belong to the run's generation. Capturing binary bytes does not make
project configuration, reviewed workspaces, replay execution or publication
one atomic snapshot. Those components retain their own guards and lifetimes.
The source set also does not prove linker selection or resolve conflicting
companion layouts. Captured data definitions carry physical symbol identities
through export, indexed queries and correspondence. Global memory-access
expressions preserve physical relocation references alongside display names.
A local target in the same
captured artifact has an exact data-object association; global/weak definitions
and name-only associations remain candidates. Runtime-address enrichment still
uses a data-symbol range match and records an explicit unknown physical binding.

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
                                    qualification evaluator
```

The project composition is deliberately layered:

1. `target.toml` owns only architecture and ABI facts;
2. `ecosystem.toml` owns reusable vendor/RTOS semantic catalogs;
3. `chip.toml` owns reusable memory-map, base register geometry, SVD, ROM, and
   only compiled knowledge that is valid across investigations of that chip;
4. `vendor-project.toml` references these inputs and owns composition plus an
   optional investigation-local `analysis-provider`;
5. sparse reviewed-knowledge packs own investigation-specific assertions,
   vendor bugs and blob-occurrence-to-semantic bindings with stable IDs,
   evidence and bounded applicability;
6. reviewed project workspaces own the remaining investigation-specific
   contracts while their generated candidates remain disposable.

Pack order is not an override mechanism. Conflicting definitions fail closed.
The project manifest separately names one exact `reviewed-knowledge.default-pack`
for new investigation facts; this destination must be a configured pack and
does not change composition precedence.
Two projects may reference the same chip pack without copying its address map.
Likewise, projects for different chips may reference one ecosystem pack. A
generic algorithm or schema belongs in Blobray source; generic is not a dump
for reusable vendor data. ESP-IDF vocabulary and public header-versioned
interface layouts, for example, are data-only ecosystem add-ons, while
ESP32-S31 addresses remain chip knowledge only when they are revision-stable.
Executable summary hooks belong to separately selected model providers even
when reusable across investigations. Exact artifact roots/digests/runtime guards,
body-identity guards, private callback cells, and Wi-Fi/BLE/802.15.4 artifact
profiles remain investigation-local because they describe the supplied blob
lineage. A project
provider may compose with a chip provider only when its compiled descriptor
explicitly extends that exact reusable root, exposes a contract superset and a
distinct precomposed harness/cache domain. Unrelated providers, transitive
extension chains and contract downgrades fail closed.

The ESP32-S31 host composes separate chip/project `knowledge` and `models`
crates. Dependencies point from models to knowledge; knowledge does not install
runtime addons or construct function traces. The host registers each model
provider's identity, revision, kind and applicability separately, and doctor
exposes that selection. Model revisions participate in both stage identities
and function harness cache domains. Descriptive provenance is not a proof of
model equivalence; exact body/context guards remain mandatory where applicable.
See the [provider ownership contract](../../../verification/vendor/projects/esp32s31/blobray-provider/OWNERSHIP.md).

Reviewed [function routes](function-workspace.md) may select unique calls by
identity/operation and derive their instruction sites and exact payload from
current observations. Explicit occurrence selectors resolve ambiguity; a
review file need not reproduce a generated expression to attach its meaning.
Whole upstream-chain discovery and typed guard expressions remain separate
work from these call selectors.

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

Project orchestration uses the `PassSpec` registry in
`application/project_analysis/pass_spec.rs`. It owns pass identities, semantic
revisions, output schema references, configuration fingerprints, cache-domain
requirements and execution dispatch. A resolved `PassGraph` fixes configured
stages, prerequisites and required/optional dependencies once per invocation.
Execution and the plan report consume that same graph; rendering the plan does
not inspect files again to reconstruct dependency policy. A failed optional
predecessor does not block its consumer. Every executed pass, including coverage,
checks the run's input guard before invoking its domain operation.

Cache-backed domain operations resolve one `ResolvedWork` declaration with checked identity,
configuration, required/optional inputs, ordered outputs and execution mode.
Preparation selects planning, cached completion or an owned `ExecutionWork`.
Successful execution consumes that declaration when recording results; completion
does not rebuild configuration or choose another set of files. Linked-IR profile
expansion uses this same path for planning, writing and check mode, while batching
pending profiles for shared analysis. Coverage declares the bundle files and
source artifacts it verifies as inputs, with no generated outputs.

The run also captures an `OutputCatalog` of exact file and IR bundle ownership.
Replay declarations are retained from one reviewed pack read. Output conflicts
and aliases of protected project inputs fail before any pass writes or restores
files. Work preparation verifies the ordered output declaration and its current
path binding. Plan dependency lookup names the producing work item, including
its profile; it does not infer production from a shared path prefix.

Project analysis retains an `OutputSet` with each execution declaration. A slot
can issue one consuming file request; a shared clone retains the same claim and
completion state. Dropping a request or failing emission does not complete it.
Completion of a work item requires all its declared outputs to have succeeded,
including check mode. Domain helpers receive these requests instead of choosing
destinations again from project configuration. Cache hits use separate candidate
sets: verified existing bytes or a successful CAS restore complete their slots;
a cache miss discards that candidate before preparing execution.

Completion retains the byte length and SHA-256 observed during emission,
comparison or verified reuse. Collecting output receipts never replaces those
values with a later read of the destination. Cache recording validates the
receipts before and after storing the stage; a change during publication retires
that stage binding. The coordinator checks receipts for executed and reused
outputs again before activating the analysis epoch.

Successful project analysis retains a schema-2 output manifest in that epoch.
It records the declared destination, length and SHA-256 of each completed output,
including uncached work, and the owning project manifest locator. The locator
uses the resolved parent directory and declared filename; it is not a content
fingerprint of the project configuration. A reader rejects an active epoch owned
by another project manifest sharing the cache directory.
Output payloads are immutable query results backed by
CAS; the manifest depends on those queries, so the existing epoch/retention graph
owns their lifetime. No separate object store or garbage collector is involved.

The public `PublishedAnalysisOutputs` capability opens the current manifest and
pins its SQLite read transaction and pack descriptors. `manifest()` exposes the
epoch and ordered bindings; `read(path)` returns verified bytes from that epoch,
even after generated files change or a later run publishes. An undeclared path
returns `None`; unavailable or corrupt declared content is an error.
`open_output(path)` authenticates the payload in bounded memory and returns a
`PublishedOutputReader` implementing `Read` and `Seek`. Its cursor is relative to
the payload and cannot enter an adjacent CAS frame. The reader owns its file
descriptor independently of the manifest handle and survives pack replacement.
`read(path)` collects that same verified view into memory. Neither API yet
selects snapshots for all existing frontend loaders.

CAS restoration, compaction, query reads and indexed IR share `FileView`, a
bounded file descriptor with independent positional-read cursors. The indexed
IR reader binds all bundle files before parsing its indexes; later function,
object, overview, graph and register reads use those bindings. They do not reopen
generated paths. Indexed ranges are checked against the captured extent before
allocating record data. `LinkedIrReader::from_files` accepts the same bindings
from a published CAS snapshot without requiring an exported directory.

Directory import still captures its members sequentially; it is not an atomic
initial bundle snapshot. Standalone loaders still select directory inputs.
Descriptor retention protects against path replacement, not external in-place
modification. Published output streams verify once at open and rely thereafter
on the CAS writer contract that existing payload bytes are immutable.

`ProjectArtifactStore` selects one published epoch on its first query and retains
that selection, including missing/failed publication, until session reload. It
admits only declared project IR roots and binds every requested bundle member
from that manifest. Missing members are errors; exported files cannot fill them.
The single-reader MRU evicts indexes, not the epoch, so another profile loaded
later cannot select a newer run.

Application function summaries, full function records, their static MMIO
annotations, register discovery/IR evidence, code-boundary facts, interface
observations and research IR graphs
use this owner. Function investigation accepts
an explicit IR reader capability and captured function pack; nested event-handler
queries use the same capability. A changed live binary cannot be combined with
the function's old published evidence. The raw-body/origin/replacement services
are not yet a common immutable input snapshot. Standalone investigation supplies
an explicit directory importer to the same implementation.

`WorkspaceSnapshot.generated_analysis_epoch` identifies the published generation
used by these readers; `FunctionDetailSummary.analysis_epoch` identifies the
detail's owner. Interface review borrows the same captured observations. Missing
publication or a missing declared output is an explicit error, including when an
export exists. Deleting exports does not prevent reading published evidence.
These fields do not claim that reviewed inputs, external observations, the entire
register inventory, project status or other research projections belong to that
epoch. Those domains still have separate captures and loaders.

The linked-IR builder requires the coordinator's profile output sets in both
write and check modes, validating profile identities, ordered members and mode
before analysis. Its bundle adapter accounts for all bundle files and coverage;
it captures their content identities from staging before directory publication.
Stale check results stay incomplete and remain aggregated across profiles.
Standalone IR build explicitly declares its own sets and uses the same builder.
`ProjectIrBuildContext` borrows its immutable analysis inputs separately from
the caller's query-store writer and output sets.

Single generated files use the consuming `GeneratedOutput` request for text,
bytes, streaming JSON and verified CAS frames. It owns one write/check lifecycle:
check compares against the existing file without staging; write creates an owned
sibling temporary file, finishes encoding, validates any expected length/digest,
syncs the file and replaces the destination. Failure or unwind before replacement
keeps the previous file and drops only the owned temporary file. Text publication
therefore replaces a destination entry rather than modifying an existing inode.
The request itself is a file-emission primitive; project work obtains tracked
requests through its admitted output set. Other standalone exporters still
construct requests directly. Linked-IR bundle publication uses a directory
adapter on the same output set, with the existing directory swap primitive.

The work contract does not provide a transaction over those files or restrict
every domain writer to a publication capability. Receipt validation does not pin
paths against replacement after validation. The final input check and cache-epoch activation remain separate
coordinator transitions. Function-body queries and other workflows also retain
their own orchestration contracts.
The output manifest captures emitted content, not the complete source/evidence
graph. Downstream passes still consume generated paths during execution;
run-local staging and a common snapshot across all query families remain absent.

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
| Operation memory slice | Last SRAM definitions reaching one selected publication site, with selector exactness separate from live site authentication | Persisted instruction effects plus an authenticated, query-time function CFG; stored paths/guards may be displayed only as non-authoritative navigation hints | Using stored path strings for reachability or ordering, cross-call alias guesses, path-feasibility claims |

A query key contains the query kind and semantic revision, the digest and
provenance identity of the artifact bytes it owns, target ABI/backend identity,
and the semantic fingerprints required by that query domain. It never contains
output paths, profile names, timestamps, UI state, reviewed ledger state, or
production-driver state. Results are immutable: changed inputs create a new
key instead of mutating the meaning of an old result. Indexed dependency edges
describe stage ownership and its consumed direct-function queries, while the
direct-function keys use the conservative
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
cache database. Reviewed packs and qualification specifications must never be
copied into or modified by the store. In schema 10, every persisted query result
belongs to at least one analysis epoch; an unowned result is invalid cache state
rather than a GC candidate. Query results are never selected by per-result LRU
or filesystem mtime.

Schema 10 is created only from a cold store. Blobray does not import, upgrade or
accept an older cache schema. After preserving reviewed inputs and durable
generated artifacts, recovery requires explicit removal of the entire
`generated/.blobray-cache/` directory and a fresh analysis.

Pack lifetime is reachability-based with explicit retention roots. Analysis
epoch memberships, query results, stage-output bindings and timestamped retired
objects are preserved by ordinary compaction. Explicit
`project cache gc --retired-epochs --retention-days DAYS --apply` can remove
whole successful epochs older than the retirement cutoff while preserving
current, standalone and pinned owners and their shared query/CAS data.
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

The channel transaction uses a narrow borrowed `RadioChannelHal`.
The runtime arena stores only an opaque `RadioRuntimeOwner` and cannot yield a
PAC owner. Cold MAC, channel, DMA, IRQ, TX, AP, and STA paths consume named
HAL operations. Powered PHY code borrows an opaque `PhyHal` with no `Deref`,
generic callback, or owner-recovery operation. PHY has no PAC dependency and
can use the capability only through named HAL operations. Repository contracts
enforce named HAL operations and reject broad borrow or `Deref` escapes.

## Verification and qualification

Device/semantic models may describe the environment or a bounded relation;
they are not production implementations. A verification-relevant comparison ends
at compiled production Rust. Dispositions can declare what is bound and what
claim is allowed, but cannot change recorded behavior.

Blobray owns neither ledger types nor readiness policy and cannot mutate a
ledger. The independent qualification evaluator produces readiness results. Blobray
does not parse its policy or calculate readiness. An implemented function without a qualifying production
trace remains visible research coverage debt; it does not fail `project
status` or `project verify` unless a configured policy, suite, or binding
requirement makes that trace mandatory.

## Documentation ownership

This file describes ownership and dependency boundaries.
[Project workflow](project-workflow.md) describes operator commands, while
[formats](formats.md) indexes current schemas. Other documents cover one
subsystem. Generated CLI help, reports, PAC/SVD output and manpages derive from
their respective code or reviewed source inputs.
