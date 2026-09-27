# Durable HIL evidence

The HIL runner owns evidence export, verification and import.
An archive preserves observations, including failures and interrupted runs.
Archiving does not make a run pass, reconstruct missing source content or
turn development measurements into current hardware qualification.

## Storage boundaries

- Git stores scenarios, analysis implementations, data contracts and identities
  of baselines actively consumed by a tool. It does not store a growing history
  of measurement tables, firmware binaries or generated reports.
- `target/hil/esp32s31/` is the local working store. It may be reconstructed from
  archived evidence. Export and import never delete original runs.

The runs of every checkout of this user share one store (see
[find and compare runs](../hil/host/README.md#find-and-compare-runs)); an
archive moves selected runs between machines or keeps them beyond pruning.

## Export and verify offline

```console
cargo hil archive export <archive-id> --run <run-id> --run <another-run-id>
cargo hil archive export <archive-id> --run <run-id> --supplement <analysis-directory> --output <archive.tar.gz>
cargo hil archive verify <archive.tar.gz> --sha256 <expected-digest>
```

The default export path is
`target/hil/esp32s31/exports/<archive-id>.tar.gz`. Existing output files are not
replaced. The same input bytes produce the same archive. Export verifies native
run integrity before packaging and validates the encoded archive afterwards.
The output JSON contains its SHA-256 digest.

A package contains:

```text
archive.json             versioned identity, run selection and complete inventory
runs/<run-id>/           original, self-contained HIL bundles
supplement/              explicitly selected analysis inputs and derived reports
```

`archive.json` schema 1 records the archive ID, target, run IDs and every member's
relative path, byte size and SHA-256. Only regular files are accepted: links,
path traversal, duplicates, unlisted files and content mismatches are rejected.
Extraction is bounded to 100,000 payload files, 4 GiB per file and 64 GiB total;
the manifest is bounded to 32 MiB. These are archive format limits, not radio
capabilities. Gzip integrity and every enclosed run's native seal are verified.

The supplement belongs to the experiment's author, not the qualification
reader. It should include the exact scenario definitions, selection/exclusion
rationale, analysis implementation, report and measurement limitations needed
to interpret that experiment. For dirty development builds, preserve available
patches and relevant untracked source files explicitly: the archive cannot
reconstruct source bytes that the original run did not retain.

## Import

To restore a package, for example from another machine:

```console
cargo hil archive import <archive.tar.gz> --sha256 <expected-digest>
cargo hil report verify <run-id>
cargo hil image replay <run-id> <image-class>
```

Import retains the complete package under
`target/hil/esp32s31/archives/<archive-id>/` and restores its runs into the normal
`runs/` directory for verification, reporting and image replay. It checks all
existing identities before publishing any new directory. Matching content is
reused; conflicting content is an error. If disk failure interrupts publication,
a retry resumes without overwriting different data. Immutable file contents can
share hard links locally; copying is the fallback. Report history can be
regenerated with `cargo hil report rebuild`.

Scenario execution still requires its definition in the scenario catalog and
a configured hardware fixture. Import preserves experiment supplements but
does not install arbitrary scenario files or execute archived analysis code.

## Retention

```console
cargo hil archive retention --keep-run <baseline-run-id> --keep-run <selected-experiment-id>
```

This read-only inventory preserves explicit selections, the latest passing run
for each scenario, non-passing or incomplete runs, imported archive members and
sources referenced by firmware replay. Unreadable/unknown run metadata is kept
with its reason; an unknown explicit run ID is an error. It never deletes files,
opens hardware or changes CAS objects.
All CAS objects remain retained. Declared digest references are not hash
verification; logical candidate bytes include hard-linked storage and must not
be interpreted as reclaimable disk space. Concurrent writers can change the
inventory. Review candidates and verify evidence separately before any deletion;
the report is not deletion authorization. Baselines outside imported archives
must be selected explicitly.

The shared run store is pruned automatically by the rule of
`cargo hil runs prune` (see
[find and compare runs](../hil/host/README.md#find-and-compare-runs)). Pin a
run, or export it, to keep it beyond that rule. Archive selected control
measurements, reproducible regressions and experiments that justify
implementation choices, together with failed attempts and selection rationale.
