# Project workflow

The Blobray is project-oriented. A `vendor-project.toml` names reviewed
configuration and generated outputs; private binary paths belong in the
ignored local run specification, never in the reviewed manifest.

## The normal loop

For an existing project:

```console
cargo blobray project doctor --project path/to/vendor-project.toml
cargo blobray project files --project path/to/vendor-project.toml
tools/blobray/scripts/run-limited \
  project analyze --project path/to/vendor-project.toml --jobs 1
cargo blobray project status --project path/to/vendor-project.toml
cargo blobray project research next --project path/to/vendor-project.toml
```

- `doctor` validates configuration, local inputs and reviewed workspaces.
- `files` explains ownership and whether each input or generated output exists.
- `analyze` refreshes symbol, MMIO, interface, linked-IR and navigation facts.
- `status` reads the current results without regenerating the whole project.
- `research next` ranks concrete human-review actions by downstream impact.

`status` is deliberately shallow and reports generated freshness as unknown;
it does not deserialize every large evidence record. `doctor` is deliberately
deep but still does not claim reproducible freshness. Its JSON schema 3 report
includes per-section timings so expensive symbol, register, interface or input
validation is attributable. Use `project check` when current bytes must be
reproduced, rather than interpreting either inspection command as a freshness
proof.

`research next` combines root-cause blockers, the reverse call graph,
unreviewed MMIO and interface observations, sparse unknown hardware semantics,
publication scopes and verification surfaces. Its `G/O/M` metrics mean
guaranteed unlock, optimistic reverse-reachable impact, and marginal benefit
after other blockers are resolved. Co-blockers and an estimated research cost
are explicit score penalties; the JSON report exposes every weighted term so
the ranking is auditable rather than an opaque recommendation. Restrict the
result to one radio area when needed:

```console
cargo blobray project research next --scope ieee802154-baseband-leaves \
  --project path/to/vendor-project.toml --limit 10
```

The command is read-only unless `--output` is supplied. Each candidate names
the missing knowledge, confidence, affected scopes, expected impact and a
copyable next inspection command. Low confidence on write semantics is
intentional: W1C, self-clear and hardware-owned behavior require reviewed HIL
or authoritative documentation and are not inferred from vendor writes.

Preview the exact stage order and cache work before a large analysis:

```console
cargo blobray project analyze --plan --project path/to/vendor-project.toml
```

The plan is read-only. It reports configured dependencies and distinguishes
current outputs, CAS restoration, cold computation, check-mode verification,
deferred decisions, blocked work, failures, and unconfigured stages. If an
earlier stage must materialize an input, a dependent cache decision is reported
as `deferred`; Blobray does not guess the digest of an output that has not been
produced yet. JSON output includes the exact stage signatures that were safe to
evaluate. Add `--details` to the human view to see every profile/work item,
output and cause without grouping.

Before publishing or merging a replacement, run:

```console
cargo blobray project publish --project path/to/vendor-project.toml
cargo blobray project verify --project path/to/vendor-project.toml
cargo blobray project check --project path/to/vendor-project.toml
```

`project check` is the fail-closed reproducibility gate. It verifies generated
outputs and the configured verification policy. It does not decide whether the
product is ready to ship; the repository qualification ledger is the sole
readiness authority. Unqualified implementations remain reported as coverage
debt in review-scope details but do not make `project status` incomplete and
are not silently promoted into mandatory verification claims. Only an explicit
verification-policy requirement makes a production trace a Blobray gate.

## Focused and full analysis

Focused inspection, a selected IR profile and `project analyze` use the same
recovery engine. Full project analysis enumerates all configured roots; it
does not switch to a second bulk algorithm. The persistent store reuses a
complete immutable profile/stage projection, including explicit
incomplete/blocker results, and restores its generated files from CAS. It also
stores direct-function facts, so partially overlapping root sets can reuse the
functions they share. All stale IR profiles selected by one `project analyze`
invocation enter the builder together, so shared catalogs and reviewed
interface knowledge are loaded once. Cache-current profiles are not rebuilt.

Each direct-function fact is bound to its exact owner identity and body and to
conservative resolver, MMIO and harness-semantic fingerprints. Other function
bodies are excluded from that resolver fingerprint: changing one body can
reuse unchanged facts for the other functions. Resolver layout, MMIO or
harness changes may conservatively invalidate a wider set; the cache does not
claim an exact per-function dependency DAG. A provider with summary hooks must
publish a stable, versioned semantic cache domain. Missing domain identity
fails closed to cold analysis instead of reusing a potentially stale fact.

The local persistent store lives below `generated/.blobray-cache/`. Removing
that directory only causes a cold recomputation. It never removes reviewed
knowledge or generated publication artifacts. Profile IDs and output paths
are bindings, so renaming an otherwise identical investigation does not change
its analysis query key. Changing artifact bytes, provenance, ABI/backend
revision or a semantic input covered by the current query domain creates a new
key. Direct-function reuse remains intentionally conservative across resolver,
MMIO and harness-semantic changes.

## Updating vendor artifacts without losing review

Capture an immutable snapshot before replacing caller-owned artifacts, run the
normal analysis on the new revision, then capture and compare it:

```console
cargo blobray project revision snapshot vendor-2026-05 --project path/to/vendor-project.toml
# update local artifact bindings, then run project analyze
cargo blobray project revision snapshot vendor-2026-08 --project path/to/vendor-project.toml
cargo blobray project revision diff vendor-2026-05 vendor-2026-08 \
  --project path/to/vendor-project.toml --details
cargo blobray project revision rebase vendor-2026-05 vendor-2026-08 \
  --project path/to/vendor-project.toml \
  --output generated/revisions/vendor-2026-08.rebase.json
```

Snapshots default to `generated/revisions/NAME.json`. They contain artifact
digests, address-independent function feature fingerprints, MMIO/interface
observations and complete serialized reviewed records, but no vendor bytes or
disassembly. The portable fingerprint is a cross-version correlator and never
replaces the address-bound evidence/cache identity.

Diff classifies stable identities, unique moves, modifications, additions,
removals, split/merge candidates and ambiguity. Only an unchanged stable
subject or a one-to-one exact normalized-feature move is automatically
carryable. Split, merge, ambiguity, removal, modified semantics and stale
artifact applicability remain `review-required`. A rebase plan retains every
old assertion/vendor-bug record with its provenance even when it cannot be
carried, so an upgrade cannot silently discard research progress.

Inspect the store before deciding whether cache growth needs investigation:

```console
cargo blobray project cache stats --project path/to/vendor-project.toml
```

The command reports cache-file, database and pack sizes, query kinds, dependency
and object counts, live records, and currently reclaimable pack bytes. It opens
an existing cache read-only. If the store has not been created, that state is
reported without creating `generated/.blobray-cache/` or any SQLite sidecar.
`cache stats` does not perform garbage collection, compaction, pruning, schema
migration, or quota enforcement; it only describes the state already on disk.

For performance measurements, set `BLOBRAY_REPORT_USAGE=1` when
calling `scripts/run-limited`. This selects its process-session watchdog and
prints elapsed time plus peak RSS for the complete Blobray process tree.
External `/usr/bin/time` otherwise measures only the `systemd-run` wrapper on
hosts where the systemd limiter is available.

## Creating a project

```console
cargo blobray project init \
  --directory radio-project \
  --id radio \
  --source vendor \
  --mmio radio=0x60000000..0x60010000

cargo blobray project inputs init \
  --project radio-project/vendor-project.toml \
  --bind source-artifact:vendor=/opt/vendor/libvendor.a
```

Generated files are disposable analysis results. Reviewed packs, policies,
register models and dispositions are source inputs. `_oracles/`, vendor
binaries, disassembly dumps and credentials remain private.

## Investigating one function

```console
cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml

cargo blobray inspect function archive:phy_chip_set_chan \
  --project path/to/vendor-project.toml --replacement
```

The ordinary view is for understanding vendor behavior. The replacement view
adds reviewed ownership, production binding, proof strength and verification
status. These are deliberately separate questions.

Use `project browse` for navigation and `advanced ...` only for backend
debugging or a focused low-level experiment.

## Resource limits

Real binaries must be analyzed through `scripts/run-limited`. The wrapper
limits aggregate memory and runtime; a large analysis should fail visibly
rather than make the development machine unusable. Build the optimized host
once when iterating repeatedly:

```console
CARGO_BUILD_JOBS=2 cargo build --profile blobray \
  -p blobray-esp32s31 --bin blobray
```
