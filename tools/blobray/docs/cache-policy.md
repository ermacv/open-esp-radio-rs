# Project cache storage and retention policy

The project cache is disposable derived state. SQLite stores immutable query
identity, dependencies, analysis-epoch membership and current stage bindings.
The append-only CAS pack stores generated outputs and large query values.
Reviewed TOML, revision snapshots and reproducible linked IR remain outside
this cache.

## Inline versus CAS

Query values of at most 65,536 bytes are stored as SQLite BLOBs. Larger values
are stored once in the SHA-256 CAS pack; each pack record adds a 48-byte header.
Generated stage outputs always use CAS, independent of size.

SQLite owns its WAL and shared-memory sidecars. Blobray checkpoints them only
when SQLite reports an unblocked, complete checkpoint and never unlinks them
manually; an external reader may legitimately keep either file live. New or
renamed CAS packs and the pinned cache directory are synced before SQLite can
publish their locations.

The boundary is deliberately unchanged in schema 9. The production-path
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

A Linux x86-64 debug-profile run on local Btrfs on 2026-08-25 produced:

| Payload | Storage | Write (16 records) | Read (16 records) | SQLite | Pack | Total |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 16 KiB | SQLite inline | 6.652 ms | 5.503 ms | 303,104 B | 0 B | 303,104 B |
| 64 KiB | SQLite inline | 21.657 ms | 15.637 ms | 1,089,536 B | 0 B | 1,089,536 B |
| 64 KiB + 1 | CAS pack | 46.082 ms | 16.089 ms | 32,768 B | 1,049,360 B | 1,082,128 B |
| 256 KiB | CAS pack | 108.507 ms | 65.163 ms | 32,768 B | 4,195,072 B | 4,227,840 B |

These measurements show approximately equal physical/read cost at the existing
boundary, while the CAS pack's separate data sync makes the just-over-boundary
write materially slower on this filesystem. They support keeping small values
inline and do not justify raising or lowering 64 KiB from one host run. Timings
are diagnostics rather than pass/fail thresholds.
Any future threshold change must record repeated release-profile measurements
on representative local storage and explain the resulting WAL, latency and
space trade-off.

## Retention and hard quotas

When the final stage owner of a CAS object disappears, the same SQLite
transaction records `retired_unix_seconds`. The object is then obsolete but is
still protected from ordinary compaction. If the same digest becomes current
again, publication removes its retirement marker. Function facts and current
stage/query references are never age or LRU candidates.

Every schema-9 query-result row belongs to at least one analysis epoch. This is
a store invariant: a result without epoch membership is invalid cache state,
not a garbage-collection candidate. Query results remain durable roots rather
than age or per-result LRU candidates. The hard quota reports live state that
cannot fit without deleting it.

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
`--max-size` is a hard post-prune assessment: when current and younger
protected state cannot fit, the command fails before rewriting the cache and
suggests a shorter retention age, a larger limit, or deleting the whole disposable
cache. It never evicts current results to meet a budget.

Retention mutation and pack compaction are Linux-only and require a filesystem
classified as local. Known network filesystems and FUSE fail closed because
SQLite WAL locking and descriptor-relative destructive cleanup cannot be
guaranteed there. Ordinary reachability compaction may remove unindexed crash
orphans, but it preserves timestamped retired objects until explicit prune.

Schema 9 is created only from a cold store. A cache database at any other schema
fails closed; Blobray has no in-place cache import or upgrade path. Preserve
reviewed TOML, revision snapshots, linked IR and other durable artifacts first,
then explicitly remove the **entire**
`generated/.blobray-cache/` directory and rerun analysis. Removing only the
SQLite database or copying individual rows, loose objects or pack-index entries
into a new store is unsupported.
