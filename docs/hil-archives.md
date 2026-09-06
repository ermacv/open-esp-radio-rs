# Durable HIL evidence

The HIL runner owns evidence export, verification, import and remote transport.
An archive preserves observations, including failures and interrupted runs.
Archiving does not make a run pass, reconstruct missing source content or
turn development measurements into current hardware qualification.

## Storage boundaries

- Git stores scenarios, analysis implementations, data contracts and identities
  of baselines actively consumed by a tool. It does not store a growing history
  of measurement tables, firmware binaries or generated reports.
- A private evidence repository stores selected experiment archives as GitHub
  release assets. Releases are named by archive ID; each ID is published once.
- `target/hil/esp32s31/` is the local working store. It may be reconstructed from
  archived evidence. Export and import never delete original runs.

Ordinary builds and host tests do not access the evidence repository. Only
`archive publish` and `archive fetch` use GitHub. They require the `gh` CLI and
an account with access to the selected private repository. Authentication uses
`gh auth login` / environment tokens, or the existing noninteractive Git
credential helper for `github.com`. Credentials are passed only to the GitHub
child process, never serialized into archive references.

The project's private store is
[`ermacv/open-esp-radio-evidence`](https://github.com/ermacv/open-esp-radio-evidence).
The repository is selected explicitly with `--repo`; forks can use their own
private store without changing the runner.

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

## Publish and retrieve private evidence

```console
cargo hil archive publish <archive.tar.gz> --repo <owner/private-evidence-repository>
cargo hil archive fetch <archive-id> --repo <owner/private-evidence-repository> --sha256 <expected-digest>
```

Publication refuses public repositories. It creates a new draft release with
`evidence.tar.gz` and `reference.json`, downloads the uploaded archive and
verifies it against the local digest before publishing the release. A failed
upload or read-back leaves a draft for inspection; the command never replaces
an existing release. Use a new archive ID for a revised report or selection.

`reference.json` schema 1 records the repository, archive ID, asset name and
SHA-256. Fetch checks the requested release identity, reference, optional
independently pinned digest, complete archive and native HIL seals before
importing. The release reference checks transfer integrity; an independently
pinned digest additionally detects replacement of both remote assets.

To import an already downloaded package without GitHub access:

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

Archive selected control measurements, reproducible regressions and experiments
that justify implementation choices. Preserve failed attempts and selection
rationale alongside successful measurements. Keep routine local iterations
until they are no longer needed; there is no automatic garbage collection.
Before deleting the last local copy, verify a downloaded remote copy. Private
remote archives require access and should have a separate backup when they are
irreplaceable. Public documentation may describe current behavior and link to a
selected reference; access to private evidence is not implied by that link.
