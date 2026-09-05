# Historical revision snapshots

The snapshots dated 2026-08-29 use schema 4. They preserve historical analysis
and their original checksums in `state.blobray`. Current Blobray uses schema 5:
each function additionally requires its raw identity, exact artifact digest,
locator, typed occurrence identity, and optional semantic identity. Those facts
cannot be recovered faithfully by merely changing the old schema number.

`project doctor` therefore reports this active state as invalid and requires
migration. The source-only register publication project is independent of this
investigation history.

To begin a current investigation, preserve the entire historical `revisions/`
directory in a separate archive first. Keep the snapshot files and their original
state/checksums together. Move the old `state.blobray` out of the active
`revisions/state.blobray` location; do not overwrite or relabel its snapshots.
Bind the intended vendor inputs in a caller-owned run spec and regenerate current
findings through the normal project workflow. Then capture a new, distinctly
named baseline using those authenticated inputs:

```console
cargo blobray project revision snapshot CURRENT --project verification/vendor/targets/esp32s31/vendor-project.toml --run-spec /absolute/path/to/local.toml
```

A new capture is evidence about its actual inputs and current analysis. It does
not retroactively authenticate the historical records, and it does not transfer
reviewed decisions between revisions without review.
