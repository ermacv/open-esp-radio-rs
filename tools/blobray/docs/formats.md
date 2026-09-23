# Formats and schemas

Reviewed configuration is TOML; large generated analysis artifacts are JSON
or JSONL. Persistent machine formats have explicit schema numbers and reject
unknown fields.

## Schema-4 project and schema-3 reusable composition inputs

- `target.toml`: architecture, calling convention, endianness, pointer width,
  and Rust target only;
- `ecosystem.toml`: reusable ordered semantic catalogs, capability-rule packs,
  and public interface-template packs for a vendor/RTOS ecosystem, with no
  chip addresses or executable provider; `[applicability].ecosystems` declares
  the stable reviewed-fact identities contributed by this layer;
- `chip.toml`: reusable memory map, base register model, SVD inputs, chip
  semantic catalogs, and an optional compiled knowledge-provider ID;
  `[applicability]` declares its `chips` and `chip-revisions` identities;
- `vendor-project.toml`: composition, reviewed workspaces, and generated
  output selection; its optional `analysis-provider` selects compiled logic
  that is valid only for this investigation, never reusable chip facts;
  schema 4 `[applicability].artifact-lineages` declares the selected blob
  lineage. Exact `{ source, sha256 }` identities are derived from the bytes
  bound by the active local run specification; declaring `artifacts` in the
  project manifest is invalid;
- `[interfaces.capability-context].output`: the required generated destination
  whenever `[interfaces].pack` selects reviewed interface knowledge. Declaring
  either side without the other is invalid; no legacy implicit destination or
  live-evaluation fallback exists;
- `[reviewed-knowledge].packs`: sparse accepted assertions and vendor-bug
  records with stable IDs, evidence, provenance and applicability;
  `default-pack` is mandatory for a non-empty list and exactly selects the one
  configured project pack that receives newly reviewed facts. It is forbidden
  when no packs are configured; pack order is never a destination fallback;
- `[[analysis.public-symbol-families]]`: explicit required or intentionally
  excluded public entry-point families, each with canonical protocol tags,
  source and symbol prefix. Required families name their expected profile;
  exclusions require a reason and must match current symbol-inventory
  identities. Missing artifacts/profiles, analyzed profiles and exclusions
  remain distinct states; exclusions never count as analyzed coverage;
- local run specification: ignored bindings to caller-owned private artifacts.

`project files` schema 4 exposes the resolved portability layer separately
from edit ownership, so a reviewed file can still be identified as reusable
ecosystem/chip knowledge or as investigation-local review.

The schema-4 project and schema-3 target, ecosystem, and chip formats are a
clean break. Schema-3 project manifests are rejected without a compatibility
reader.
Old inline `memory-map`, `svd`, `platform-pack`, `harness`, and
`semantic-catalogs` keys fail closed; no compatibility shim reinterprets them.
A project-local `analysis-provider` and chip-pack `knowledge-provider` compose
only when the installed analysis descriptor explicitly extends that exact chip
provider. Registry validation requires a reusable root, a complete contract
superset and a distinct precomposed harness/cache domain; there is still one
effective provider after resolution. Missing or unrelated descriptors fail
closed instead of using manifest order as executable precedence.

## Other reviewed inputs

- `verification-addon.toml` (`schema = 3`): suites, compiled-artifact
  comparison inputs, declarations, and report paths; it has no executable
  verdict provider and grants no analysis knowledge;
- register/interface/function packs: reviewed assertions and evidence links;
- sparse reviewed-knowledge packs (`schema = 2`): canonically typed semantic
  subjects, consumer-owned kind/value facts, entity bindings and vendor bugs.
  Empty evidence, hints, duplicate IDs, invalid artifact hashes and overlapping
  assertions fail closed; pack and record applicability are intersected rather
  than overridden. Blob-local occurrences can bind to stable semantic entities
  only with exact artifact applicability and occurrence-linked evidence. The
  same occurrence cannot bind to two semantics in overlapping applicability,
  while one semantic may intentionally collect multiple blob occurrences. The
  effective project composition selects facts before use. A missing context
  dimension or two same-subject, same-kind facts selected by an ambiguous
  context is an error. Schema 1 is rejected without a compatibility reader;
- disposition manifests: reviewed vendor-to-production binding and claim
  declarations, never execution truth;
- verification policy: required comparisons and bounded properties;
- evidence catalogs: provenance links for reviewed claims.

### Reusable capability packs

A schema-1 capability pack contains `[[rules]]` with a stable dotted `id`, a
`protocol`, a classification `scope`, a human summary, optional `depends`, and
nested `[[rules.requirements]]`. Requirement `kind` is `operation`, `effect`,
or `call`; `value` is reviewed semantic vocabulary and `min-matches` defaults
to one. Rule IDs may be shared across ecosystem and chip packs only when their
complete definitions are identical. Missing dependencies, cycles, duplicate
matchers, zero match counts and conflicting definitions fail during loading.

`protocol` and `scope` are report labels, not evidence filters or coverage
claims. A rule searches all validated interface bindings. `operation` matches
a reviewed semantic binding, `effect` matches one of that binding's reviewed
effects, and `call` additionally requires at least one concrete resolved call
site. Machine reports contain the exact binding/call evidence and sort packs,
rules and matches deterministically.

A `matched` result means only that the declared rule matched current reviewed
interface evidence. It does not establish hardware support, runtime ordering,
semantic completeness, or qualification. Known vocabulary without enough
current evidence is `incomplete`; vocabulary absent from the configured
semantic catalogs is `unknown`. Either state propagates through dependent
rules and never becomes a positive capability claim.

### Reusable interface templates

A schema-1 interface-template pack contains public, versioned callback-table
layouts. Each template owns only its stable ID, public header provenance
(`repository`, exact 40-hex `revision`, and relative `path`), layout version,
pointer width, size, stride, and slot offsets/ABI/semantic IDs. It cannot name
an artifact source, symbol/address root, container path, digest/runtime guard,
execution contract, or compiled execution model. Duplicate pack IDs and
same-ID template conflicts fail closed; identical template definitions from
distinct pack IDs may be deduplicated.

An interface pack schema-3 anchor opts in with `template = "..."`. That
project anchor still owns the exact source/root/container binding, exactly one
artifact SHA-256 guard, any runtime guards, and its execution contract.
`[[anchors.overrides]]` entries are keyed by a template slot `offset`, require
a one-line `reason`, and may explicitly change provenance, ABI/semantic fields,
or attach a project provider execution model. Template slots start as reviewed
public-header assertions: only an explicit `origin = "observed"` override may
classify matching generated artifact evidence. Unknown/duplicate offsets,
unexplained overrides, local layout duplication, and missing digest bindings
fail closed. Validation JSON preserves sorted template pack IDs, source
provenance, and every overridden offset/reason/field; reasons are diagnostic
provenance, never executable semantics.

## Durable revision state

- immutable schema-6 revision snapshots (`revisions/snapshots/NAME.json.gz`)
  contain typed vendor artifact/inventory/companion digests, normalized function
  features and the common register inventory graph with evidence digests.
  Evidence payloads and captured input bytes are not copied into snapshots.
  Function artifact scope excludes local Rust verification ELF identities;
  register observations retain their individual source provenance. Snapshots
  also bind every reviewed record to the
  full authenticated ecosystem/chip/revision/lineage/artifact context;
- `revisions/state.blobray` is the tracked custom revision-state DSL. Its first
  line is `blobray-revision-state 1`; the remaining typed directives store only
  project/revision names, relative snapshot locations, SHA-256 identities,
  `baseline`/`current` pointers and an optional update-preflight marker.
  `snapshot-sha256` identifies normalized logical snapshot content, not the
  encoded `.json.gz` bytes; gzip is a replaceable storage codec.

The register graph uses the same physical subject IDs, unknown/conflicted
properties, alternative fields, access widths, bit/address coverage, source
states and gaps as register queries. Access width is not part of subject
identity. Each evidence reference stores its ID, kind, source identities and
SHA-256 of the complete query record. Full payloads remain available through
the inventory query and its original inputs; a revision file alone cannot
recover their bytes from those hashes. Snapshot validation checks graph
references and recomputes comparison fingerprints from the stored records.

Register diffs include payload-hash changes as well as changes to subjects and
fields. The `register-coverage` domain reports source, region, indexed-domain,
opaque-evidence and gap changes even when no concrete register exists. Rebase
maps reviewed register/field anchors to physical locations and requires an
unchanged graph plus known matching physical width; an unknown or conflicted
width cannot authorize an automatic carry. SVD selection uses the resolved
query context, including explicit overrides.

Snapshots are tool-written correspondence maps rather than manually reviewed
facts. Unlike ordinary generated output, they and their state must survive a
vendor update; commit them or place them in equivalent durable,
access-controlled storage. Snapshot names are immutable.

Linked-IR schema 73 records the primary artifacts, symbol inventories and
companions that affected each generated bundle. Revision capture compares all
three dependency classes with the current typed run-spec and rejects stale
generated evidence. Function records retain artifact-bound occurrence and
reviewed semantic identities; a symbol name alone is not revision identity.
Each function also carries `code_identity`, including the artifact digest,
object ordinal and symbol-table index (or explicit reviewed section range).
Function locators and revision occurrences derive from that physical identity.
Readers validate it in full records, random-access records and overview streams.
Root blockers, including rejected semantic transfers and ambiguous origins,
remain in the bundle manifest even when no body was analyzed. Function
investigation schema 20 includes the same code identity in full and compact
body views; symbol correspondence schema 11 uses physical function and data
locators and exposes each data candidate's `data_identity`.
Data objects and their bundle index carry `data_identity`: artifact digest,
object ordinal, symbol table and index. Co-located anchors and static/dynamic
symbol occurrences remain distinct; names and inferred extents are metadata.
Full and indexed readers reject inconsistent data identities. Global memory
expressions retain a mandatory `reference`: captured symbol location and
binding, or an explicit `unknown` reason. Data-object cross-references use
`physical-local-definition` for a local target in the same captured artifact,
`physical-definition-candidate` for a non-local definition, and
`member-name-and-symbol-candidate` for name associations. A captured global or
weak definition does not prove linker selection. Function-body relocation
records expose the same reference, including the target of normalized HI/LO
pairs. Unnamed relocation targets remain physically identifiable.
Memory accesses and instruction effects also retain mandatory `data_address`
evidence: an explicit unknown reason, one range candidate, or all ambiguous
candidates with physical data identities. Numeric range matching never rewrites
the original memory object or offset and does not prefer smaller or exported
symbols. Its basis distinguishes a complete access address, an indexed base and
an argument-offset hint. Read-location hints without an access width expose
`width: null`; they do not claim that a complete load fits the candidate.
Pseudocode retains the original address expression and displays candidate hints.
Data-object cross-references include `address-range-candidate` associations and
separate `reference-trace` and `instruction` evidence channels; counts from
these channels must not be added as distinct dynamic accesses. Companion data
objects are exported under their actual artifact digest and source context.
`inspect object` schema 3 preserves this association on access evidence and
accepts `SOURCE:occurrence:memory-object:sha256:DIGEST` for exact selection,
including unnamed objects. A symbol selector returns all matching occurrences.
Access evidence includes the original object, `observed_offset`, all address
candidates, and the candidate-relative `offset` used by the offset filter.
Pointer-storage and pointee accesses remain distinct.
Reviewed names on overview memory effects require an exact local reference;
name collisions cannot copy one data occurrence's reviewed binding to another.
Each primary artifact lists the ordered companions used in its own analysis
context. The bundle-wide companion inventory does not grant another source
access to those definitions.

Only schema-6 snapshots and revision-state DSL version 1 are accepted. TOML,
older state and migration maps are not parsed or upgraded. Preserve durable
snapshots and reviewed bindings when a schema is rejected; cache invalidation
is not permission to discard that evidence.

## Generated outputs

- `inspect analyze` schema 4 captures primary and companion bytes before
  constructing the resolver. Direct and reference analysis use that resolver's
  context and exact physical definitions. Every function row carries
  `code_identity`; blocker impacts, callee hotspots and unmapped-MMIO users
  refer to these identities rather than display names, so repeated archive
  member and symbol names remain distinct. Artifact hashes describe the captured
  bytes, including when paths change before rendering. `--details` text output
  includes the physical function identities. A target without a configured
  knowledge provider uses the neutral RV32 context, as linked-IR analysis does;
- symbol inventory schema 7 records object ordinals, symbol-table indices and
  section indices. Names and addresses are metadata; repeated archive members
  remain distinct candidates. Unnamed symbol entries are retained. Artifact
  digests come from the captured bytes and are not recomputed during rendering;
- interface observations schema 11 retains each numeric base, original load/store
  offsets and mandatory `data_address` evidence. All overlapping, alias and
  static/dynamic data definitions remain candidates; missing ranges are explicit
  unknowns. Numeric store assignments are possible pointer evidence and may also
  represent scalars. Review backlog association requires a unique range candidate.
  Revision keeps the address subject stable while merging all range evidence into
  its fingerprint. Navigation schema 7 retains the shared typed interface observations, including
  unresolved calls, assignments, gaps, decode blockers and analysis failures,
  independently of symbol matches. Its reader validates these observations
  against the authenticated interface input. Root links also retain the same
  address resolution. Arguments use a mandatory typed `value` (including alternatives,
  unknown, selectors, indexed pointers and GOT addresses). Calls retain every
  load site, link register and target post-offset; assignments retain target
  loads and post-offsets. Table aggregates compare layout independently of
  instruction sites. `limits` records propagation budgets; `gaps` retains the
  physical code owner, instruction site, reason and all 32 input register
  values. Interface revision includes call, assignment and gap evidence even
  without a table candidate. Old argument and bounded-data-address records
  are rejected;
- `interfaces validate` schema 4 exposes the shared `observations` query result
  independently of reviewed bindings. Resolved calls and assignments wrap their
  full observation; slot selection and reviewed target association are separate
  fields. Argument kinds and text are derived from typed values at rendering
  time. Validation does not claim complete analysis. Function review includes
  the same typed call observation alongside its human-readable table; distinct
  load sites remain distinct even when rendered argument expressions coincide.
  Calls, assignments, decode blockers and analysis failures require a physical
  `owner` code identity; the reader checks its artifact digest. Same-name members
  and equal instruction addresses do not merge observations. Instruction-level
  revision subjects use owner and site; display-name changes do not replace them.
  Full and compact function queries retain `code_identity`, and reviewed interface
  caller joins use that identity. Relocated roots require a physical symbol
  reference or an explicit unknown reason. Function-argument roots include their
  owning code identity; calls, assignments and gap arguments reject a different
  owner. Linkage candidates retain their physical symbol locations.
  Navigation groups symbols by captured artifact and physical occurrence using
  `physical-occurrence-v2`; names, member paths and addresses are retained as
  labels without changing identity. Companion functions retain their own artifact
  digest. Captured relocated roots associate only with their physical occurrence;
  an unknown reference does not fall back to its display name. Numeric ranges
  retain all candidate associations. Reviewed root selectors and project-call
  reachability remain separate metadata associations. These records do not prove
  linker selection or body equivalence;

- application workspace snapshots expose the same typed interface query as
  `interfaces.observations`, independently of reviewed slots. The tagged
  `observation_state` distinguishes `not-configured`, `missing`, `available` and
  `failed` (with a reason). Review diagnostics leave available observations
  intact. The TUI Interfaces section lists raw table candidates, calls,
  assignments, propagation gaps, decode blockers and analysis failures alongside
  reviewed slot projections. Search includes typed evidence; raw details retain
  argument alternatives, load sites and gap register values. Raw observations
  alone leave behavior unknown. Reviewed slot navigation remains separate from
  these raw evidence rows;
- MMIO schema 6 preserves instruction-local
  value provenance and unresolved/indexed address observations as well as
  aggregate discovery statistics;
- register inventory schema 1 joins declarations, observations and hypotheses
  with source digests and evidence IDs. Physical subjects include chip,
  address space, route, bank and address; load/store width is independent.
  Queries retain unknown and conflicting properties, full input records,
  conditional address domains and explicit coverage gaps. Application register
  reports carry `inventory.state`: `available` contains the snapshot's `id` and
  full `inventory` graph; `failed` contains a `reason`. Sources, domains and gaps
  remain available even with no concrete registers. Rows are queried from this
  graph, with no second register projection; field/region totals describe this
  inventory. List/coverage JSON schema 2 includes its content ID as `snapshot_id`.
  The optional `publication` summary retains the separate configured
  model/discovery review counts, with null for unavailable review;
- `inspect register` schema 10 accepts a `register-location/...` subject ID or
  a 64-bit address. `register.selection` preserves that choice; an address
  query includes every containing physical subject across domains.
  `register.inventory_snapshot` identifies the captured inventory used for detail.
  Detail
  `fields` retain the full inventory field under `field` and its parent ID
  under `subject`, including fields without a representable 32-bit mask.
  `regions` contains all matching MMIO ranges. Evidence includes references
  owned by property claims and fields as well as direct register references.
  Name provenance is retained in each subject's property claims and referenced
  evidence; there is no single guessed `name_source` for mixed inputs. An
  ambiguous selection has no single width or reviewed recording subject;
- replay evidence schema 4 includes ordered MMIO observations per concrete
  phase. Their PC is explicitly unknown when the execution event does not
  carry one. Replay coverage applies to the recorded scenario only;
- interface capability context (`schema_version = 2`, command `interfaces
  capability-context`): a compact, deterministically sorted projection of
  unresolved interface observations and existing capability links for
  `research next`. Its `input_digest` covers project identity, calling
  convention, compiled-knowledge identity, generated interface facts, the
  reviewed interface pack, and configured semantic, capability and interface
  template packs. `project analyze` writes it after interface validation and
  `project analyze --check` verifies it. It is disposable derived state, never
  reviewed authority; a missing, malformed, wrong-project or stale document is
  omitted with an explicit partial-prioritization diagnostic rather than being
  reconstructed from live inputs;
- canonical derived linked-IR bundles and indexes, including structural loop
  regions, explicitly non-proving counted-loop candidates, and raw-bit
  floating value-flow nodes whose operation and rounding mode remain explicit.
  Schema 71 records width-alternative bindings, full-word field hypotheses,
  call-argument bit provenance, typed guarded-return frontiers and full call-result
  producer identities. `structurally_complete` means only that every terminal
  was enumerated within bounded traversal; it does not assert expression
  exactness, path feasibility, event delivery, or mutable-object lifetime;
- linked-IR source coverage schema 2 binds every symbol root to `code_identity`
  and uses the analysis source set for its inventory and input hashes. Equal
  member names, symbol names and addresses cannot account for an omitted
  physical occurrence. Coverage validation separately checks current inputs
  and the sealed bundle products;
- navigation and review-scope indexes; project-wide call associations retain
  source-qualified candidates and a unique/ambiguous/unresolved status without
  claiming linker resolution. Review-scope schema 12 persists the mandatory
  many-to-many `protocols` membership from project configuration; protocol
  membership is never reconstructed from scope IDs;
- pseudo-Rust and executable reference artifacts;
- verification reports and evidence index;
- SVD, raw PAC, bindings index, and restricted API output;
- revision diff and rebase plans. Diff reports use their own schema 2,
  independently of schema-6 stored snapshots, and include a typed function
  delta (`changed`, `added`, `removed`, `{ before, after }` remaps and uncertain
  identities) plus research invalidation areas with affected subjects and
  reviewed-record IDs. `@live` is a read-only operand for validating and
  comparing current analyzed bindings before publishing a new immutable
  snapshot;
- research-next reports (`schema_version = 19`) retain source diagnostics and
  independent register-property questions even when discovery or review scopes
  are unavailable. They contain one deterministic,
  SHA-256-identified full inventory of findings, actions and prerequisites.
  Actions refer to the single typed finding catalog by ID and prerequisites
  carry no rank; the bounded `selection.steps` list contains only ordered typed
  IDs. Exact artifact-authenticated reviewed memory classifications distinguish
  hardware-shared SRAM from software-only or unclassified RAM for selection;
  they are ranking evidence and never resolve the analysis finding. The
  explicit `focus` changes selection eligibility, while strategy,
  focus, limit and budget do not change the inventory digest.
  The always-present exact-finding query distinguishes `all`, `open`,
  `condition-satisfied`, `input-not-observed`, `filtered-out`, and
  `not-present`; typed register resolution evidence never claims completion
  or historical occurrence. Findings retain subjects,
  executable-consumer resolution, actionability,
  evidence, impact sets, a typed exact-finding requery action and typed
  revalidation actions. Required public symbol-family gaps distinguish manual
  `coverage-blocked` states with one prerequisite from automatic `ready`
  states with no duplicate prerequisite. Every catalog entry exposes one
  state-consistent `next_action`. Every executable action stores exact argument
  boundaries, the absolute invocation working directory and its project-context
  level; rendered shell text is never part of the machine schema. Capability matches and
  verification surfaces are context-only links with zero ranking weight, and
  the report makes no completion claim. Typed incomplete event-route blockers
  retain their route ID, blocker kind, exact `inspect flow --event-route`
  action and current-report absence predicate. Until the producer publishes
  typed impact evidence, they retain scope and function-inspection navigation
  but expose no affected roots, publication scopes, or function-unlock weight.
- project-status reports (`schema = 11`) keep shallow artifact readiness
  separate from generated freshness, open research debt and verification
  readiness; review-scope details expose their explicit protocol memberships,
  while `radio_surfaces` reports analyzed profiles, missing vendor
  artifacts/profiles, and audited public-family exclusions. `ready` never means
  that a review scope has no remaining work.

Ordinary generated analysis outputs under `generated/` are disposable and
reproducible. They must preserve source artifact identity/provenance and must
not contain proprietary payloads or full disassembly dumps. A generated file
cannot replace its reviewed input.

Two machine-written records are intentionally durable rather than disposable:
the immutable revision snapshots described above, and the incremental vendor
evidence index selected by a verification add-on. The evidence index is a
compact qualification publication whose source hashes are rechecked by the
qualification evaluator; preserve both record classes in version control or
equivalent controlled storage. They remain tool-owned records, not manually
reviewed fact packs.

Vendor evidence index schema 2 records `suite_states` (`complete` or
`incomplete`). Each completed suite replaces only its own current entries.
Starting a rerun removes that suite's previous current entries before execution;
an interruption cannot leave its old PASS current. Content-addressed sibling
`*.history/` files retain prior indexes, including failed comparisons. The
`complete_project_run` flag remains descriptive; per-suite completion governs
schema-2 consumption. Legacy schema-1 indexes still require a complete run.

Each new entry also records `comparison.project_manifest` and
`comparison.sha256`. The identity covers that selected suite and function's
production binding, effect/semantic disposition, profiles, accepted baseline,
applicable policy surfaces and artifact identities. Canonical TOML values omit
comments and descriptive `description`, `title`, `notes`, `reason` and `rationale`
fields. Shared profile/disposition files are filtered to the selected function;
changes in another suite do not invalidate the entry. The producer also checks
that profile, disposition and baseline bytes still match the execution report
before publishing a release-eligible entry.

Hardware artifact selection must have public SHA-256 bindings. Existing
`suites.vendor` fields `artifact-sha256` and `companion-sha256` bind selected
vendor sources. A suite may additionally declare:

```toml
[suites.artifact-bindings]
"auxiliary:linked-image" = "LOWERCASE_SHA256"
"source:vendor:inventory" = "LOWERCASE_SHA256"
```

Replace placeholders with the reviewed digests of the selected inputs. This
covers auxiliary images without publishing private file paths or binaries.
Every recorded `source:*` artifact/companion/inventory and `auxiliary:*` image
must match a public binding. Repeated inventories use the reported
`source:ID:inventory:N` role. Missing or
conflicting bindings cannot establish current release evidence. A baseline
does not substitute for artifact selection. Legacy entries without comparison
identity remain historical; only the selected comparison needs a new publication.

## Internal persistent query store

`project cache gc` retention reports use `schema_version = 2`, with an explicit
`retired-objects` or `retired-epochs` scope and eligible epoch/query/object
counts. `--retired-epochs` requires `--retention-days`; the ordinary GC scope is
unchanged. See [retention policy](cache-policy.md) for protected roots and the
atomic publication contract. This report change does not change cache schema 11.

`generated/.blobray-cache/queries.sqlite3` and `objects-*.pack` are
disposable local implementation state, not project formats and not evidence.
SQLite owns query keys, dependency edges, project-local output bindings, and
the active immutable pack generation. Small query values remain inline in
SQLite. Pack generations own larger immutable query values and every
restorable generated output, addressed by SHA-256. Reachability compaction
atomically switches generations and removes unreferenced objects. A
store-schema change recreates the database and packs. No reader for an
obsolete cache schema is maintained.

Do not commit, publish or hand-edit the store. Generated linked-IR bundles are
the portable/public artifacts; reviewed TOML remains the accepted knowledge.
The SQLite WAL requires a local filesystem and is not supported on a network
share. Linux writer and manual-compaction paths enforce this against known
network filesystem types and conservatively reject FUSE because its backend
locality is not visible. Manual compaction also reserves space for a complete
new live pack, the SQLite working set and a fixed safety margin before writing.
Other platforms keep destructive compaction disabled. JSONL and ZIP are
intentionally not cache backends: they do not provide
the indexed point lookup, multi-table atomic binding update and incremental
append/recovery contract required here. Use JSON/JSONL for portable reports and
bundle files, and use revision snapshots plus reviewed TOML to preserve research
across vendor releases.

Human output is not an automation API. Scripts use `--format json` and check
the reported schema.
