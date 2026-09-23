# Project cache storage and retention policy

This document describes unported facade responsibilities. Its command examples are
not supported by the primary `cargo blobray` command. Use the
[current operator reference](../next/README.md) for implemented operations and
[register publication](../../registers/README.md) for SVD/PAC generation.


The project cache is disposable derived state. SQLite stores immutable query
identity, dependencies, analysis-epoch membership and current stage bindings.
The append-only CAS pack stores generated outputs and large query values.
Reviewed TOML, revision snapshots and reproducible linked IR remain outside
this cache.

Project-stage signatures use the same `PassSpec` registry as the analysis
coordinator and planner. Each descriptor supplies its semantic revision,
artifact schema, analysis domain and compiled-knowledge dependency. Checked
work-item keys distinguish whole stages, linked-IR profiles and validation
policy. Unknown owners, unsupported suffixes and missing validation policy are
errors. Equivalent linked-IR profile bindings share a semantic cache name;
validation with and without `deny-unreviewed` has distinct identity. A linked-IR
provider without a nonempty semantic domain cannot use persistent stage cache.

The coordinator retains one resolved work declaration through planning, lookup
and completion. The input requirement is explicit; absent optional paths remain
in the cache fingerprint. Completion uses the retained configuration and ordered
outputs, and consumes its execution handle once. Existing input mutation guards
still prevent recording a result after its inputs change.

Project analysis validates output ownership before cache lookup or restoration.
Overlapping outputs, protected input aliases and changed output bindings cannot
be admitted as restoration targets. The same catalog determines exact producer
work identities for deferred plan inputs. This is a preflight boundary; it does
not pin filesystem destinations against subsequent concurrent replacement or
make publication of several output files transactional.

CAS restoration uses the same `GeneratedOutput` streaming writer as generated
reports. It copies one framed payload and verifies its exact length and SHA-256
before replacing the destination; truncated or corrupt payloads preserve the
previous file. The common writer owns cleanup of its sibling staging file and
syncs completed content before replacement. Store-level authority and epoch
publication remain separate from this single-file operation.

The cache accepts an explicit `OutputSet` rather than arbitrary restore paths.
An existing file completes its slot only after its actual SHA-256 matches the
cached digest. Otherwise restoration obtains a consuming request for that slot;
successful verified emission completes it. A current hit must complete all its
slots. If lookup detects changed inputs and requests recomputation, its candidate
set is discarded; execution receives a fresh set from the retained declaration.
Domain completion cannot cache a work item whose output was omitted, whose
request was dropped, or whose emission failed.

Each completed slot retains its emitted or verified content length and SHA-256.
Stage recording uses that retained digest and checks the destination before and
after cache publication. A changed output retires the new stage binding; a
changed restored output also retires its consumed binding. Before epoch
activation, the coordinator validates every completed output, including cache
hits. A mismatch leaves the previous epoch active. These checks detect changes
at the validation boundaries; they do not make exported files an immutable,
transactional snapshot or prevent replacement after the final check.

At successful project completion, the writer publishes an
`analysis-output-manifest` immutable query and `analysis-output` payload queries
for every completed output, including those of uncached work. Manifest entries
carry logical paths, lengths and SHA-256; its query dependencies retain those
payloads through the existing epoch and CAS retention rules. The manifest becomes
visible when the epoch activates. An unsuccessful run cannot replace the active
manifest, even if its generated files have already been written.

`PublishedAnalysisOutputs::open` uses the read-only WAL snapshot capability.
Reads access verified CAS content, never current generated paths. Missing cache
or no published project epoch returns `None`. A published epoch with no manifest
requires a fresh analysis; no compatibility reader reconstructs it from files.
Older valid stage results can still be consumed by a new analysis and included
in its newly published manifest. Output payload queries use version-1 keys;
output manifests use version-2 keys and schema 2. They record the owning project
manifest locator (resolved parent directory plus declared filename). Readers
reject an epoch belonging to another manifest in the same directory. Schema-1
output manifests have no reader; rerun project analysis to publish a current
manifest from valid stage results. The SQLite table schema remains unchanged.

Project-session IR queries retain their first published reader, including a
failed or absent capture. Profile reader eviction does not select a new epoch.
Function summaries, details and their generated MMIO annotations read the same
manifest; an exported file cannot replace a missing or corrupt declared result.
Reloading the session allows it to observe a newer publication.

`open_output(path)` provides a bounded `Read + Seek` payload reader. Opening
checks query membership, kind, content identity, pack framing, byte length and
SHA-256 without materializing the payload. Subsequent reads use a private cursor
and positional file I/O; cloned descriptors cannot move another reader's cursor.
The reader retains the descriptor after its parent manifest handle is dropped
and after compaction removes the old pack pathname. It cannot seek into another
object or read bytes appended beyond its captured extent. Integrity is checked
at open; external in-place changes afterward are outside the CAS writer contract.

A successfully consumed function fact belongs to the publishing analysis epoch,
including when it was loaded from an earlier completed epoch. Each linked-IR
stage records its consumed function queries as dependencies. Restoring a whole
stage atomically carries that dependency closure into the new epoch; every
dependency must be visible in the caller's snapshot. Merely finding a result in
an unfinished epoch does not authorize publishing it. Batched profiles keep
separate dependency scopes, including shared functions used by several profiles.
Existing identical function values acquire membership without another CAS write.

Published analysis queries use a separate `QueryReader` capability. It opens
an existing database read-only, holds one WAL read transaction and retains file
descriptors for that transaction's CAS packs. It acquires no analysis writer
lock and cannot publish, restore files or run maintenance. An absent cache
remains absent. SQLite may create or update its WAL coordination sidecars;
read-only access does not mean an immutable filesystem image.

Register inventory is derived in memory from session inputs and retained until
reload. Its builder does not open a writer or populate a persistent query cache;
generated discovery and IR inputs remain durable in the published CAS manifest.

The reader owns its connection, published epoch and pinned files directly. It
contains no writer, publication state or maintenance capability. Readers,
writers and locked plan inspection share one borrowed lookup layer for epoch
visibility, query digests and CAS validation. That layer does not open stores,
restore generated files or acquire query ownership. Snapshot pack reads use
their retained descriptors; a missing pin never falls back to a current pack
pathname.

On Linux, snapshot opening briefly holds a shared directory lock while pinning
pack descriptors. Pack cleanup defers unlinking if that guard is held. Once
opened, readers can outlive compaction and continue reading unlinked packs from
their descriptors. Those disk blocks remain allocated until the last reader
closes, even though directory-size accounting no longer includes their names.
Writer shutdown attempts a nonblocking checkpoint and leaves pinned WAL frames
to SQLite. A reader sees its original published epoch and standalone facts as
of its transaction; it never sees another writer's unpublished epoch.

Inventory queries retain derived graphs in the session without persistent cache writes.
Plan, statistics and maintenance previews still use their stricter shared-lock
inspection path, which excludes a writer and rejects a nonempty WAL. These paths
do not yet share the concurrent query-reader lifecycle.

## Inline versus CAS

Query values of at most 65,536 bytes are stored as SQLite BLOBs. Larger values
are stored once in the SHA-256 CAS pack; each pack record adds a 48-byte header.
Generated stage outputs always use CAS, independent of size.

SQLite owns its WAL and shared-memory sidecars. Blobray checkpoints them only
when SQLite reports an unblocked, complete checkpoint and never unlinks them
manually; an external reader may legitimately keep either file live. New or
renamed CAS packs and the pinned cache directory are synced before SQLite can
publish their locations.

Schema 10 uses this boundary. The production-path
diagnostic uses 16 deterministic, unique values at 16 KiB, 64 KiB, 64 KiB + 1
and 256 KiB. It measures fresh-cache writes (including the normal SQLite
transaction/fsync path), verified reads and physical database/pack sizes:

```console
BLOBRAY_CACHE_BENCH_ROOT=target \
  BLOBRAY_CACHE_BENCH_OUTPUT=target/cache-policy-measurement.json \
  cargo test -p blobray cache_storage_policy_measurement --lib -- \
  --ignored --test-threads=1
```

The ignored diagnostic writes its stable JSON document only to the explicitly
named output file; normal tests do not emit or persist benchmark results.

## Retention and hard quotas

When the final stage owner of a CAS object disappears, the same SQLite
transaction records `retired_unix_seconds`. The object is then obsolete but is
still protected from ordinary compaction. If the same digest becomes current
again, publication removes its retirement marker. Current, standalone and
pinned epoch ownership is never an age or LRU candidate.

Every schema-10 query-result row belongs to at least one analysis epoch. This is
a store invariant: a result without epoch membership is invalid cache state,
not a garbage-collection candidate. Ordinary compaction and the default
retention scope preserve every query-result row and successful historical
epoch. The hard quota reports protected state that cannot fit without deleting
it; it does not select additional eviction candidates.

Preview a retention prune before applying it:

```console
cargo blobray project cache gc --dry-run --retention-days 30 \
  --max-size 4294967296 --project path/to/vendor-project.toml
cargo blobray project cache gc --apply --retention-days 30 \
  --max-size 4294967296 --project path/to/vendor-project.toml
```

The preview is read-only. Apply acquires the exclusive cache lock, rechecks the
same cutoff and filesystem/quota preflight, copies every current or
retention-protected digest to a new pack, verifies it, and switches the SQLite
index atomically. Only persisted retired objects at or before the cutoff are
age-pruned; unreachable indexed crash garbage may be removed by the same
reachability rewrite and contributes only to the reported reclaimed bytes.

To include completed historical analysis epochs, explicitly add
`--retired-epochs` to both preview and apply:

```console
cargo blobray project cache gc --dry-run --retention-days 30 --retired-epochs \
  --project path/to/vendor-project.toml --format json
cargo blobray project cache gc --apply --retention-days 30 --retired-epochs \
  --project path/to/vendor-project.toml --format json
```

This scope removes whole successful epochs retired at or before the cutoff.
Age starts when a later successful analysis retires the epoch, **not** when
the original analysis was created or completed. The current published epoch,
the standalone focused-analysis epoch, every epoch with any persisted pin
(`manual`, `revision-baseline` or `revision-current`), and unfinished epochs are
excluded. Normal analysis and ordinary compaction do not expire successful
history. Abandoned unfinished epochs retain their separate cleanup policy at
the next successful analysis publication.

The plan reports eligible epoch, query and object counts. Queries disappear
only when all their epoch owners are removed; shared function facts and CAS
objects remain available to their surviving owners. An object's final epoch
owner already satisfied the retirement age, so its exclusive CAS data is
removed in the same prune, without starting another retention interval.
The command verifies the preserved CAS pack before atomically deleting epoch
and query metadata and publishing the new pack index. A corrupt preserved
object aborts before epoch deletion. Pruning does not change visibility: pins
keep historical data without making it a current analysis result. If a retained
query lacks ownership of a dependency that would otherwise lose its final
epoch, GC fails closed and asks for the retained query to be recomputed with
complete dependency ownership.

Reviewed TOML, revision snapshots, evidence files and generated outputs outside
`generated/.blobray-cache/` are unaffected. SQLite pages freed by metadata
deletion can be reused but are not shrunk with `VACUUM`; the quota projection
conservatively keeps the current physical database size. Metadata-only pruning
still reserves working space for the complete pack and SQLite rewrite.

`--max-size` is a hard post-prune assessment: when current and younger
protected state cannot fit, the command fails before rewriting the cache and
suggests a shorter retention age, a larger limit, or deleting the whole disposable
cache. It never evicts current results to meet a budget.

Retention mutation and pack compaction are Linux-only and require a filesystem
classified as local. Known network filesystems and FUSE fail closed because
SQLite WAL locking and descriptor-relative destructive cleanup cannot be
guaranteed there. Ordinary reachability compaction may remove unindexed crash
orphans, but it preserves timestamped retired objects until explicit prune.

Schema 10 is created only from a cold store. A cache database at any other schema
fails closed; Blobray has no in-place cache import or upgrade path. Preserve
reviewed TOML, revision snapshots, linked IR and other durable artifacts first,
then explicitly remove the **entire**
`generated/.blobray-cache/` directory and rerun analysis. Removing only the
SQLite database or copying individual rows, loose objects or pack-index entries
into a new store is unsupported.
