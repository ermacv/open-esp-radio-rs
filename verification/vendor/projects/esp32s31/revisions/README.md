# Investigation revision state

This directory contains immutable machine snapshots and the selected revision
state. They bind analysis identities, artifact provenance and reviewer
decisions; they are inputs to revision comparison, not product qualification.

Blobray's current revision schema is 5. A function identity includes the raw
identity, exact artifact digest, locator, typed occurrence identity and optional
semantic identity. A schema-4 record cannot acquire those facts by relabelling
its schema. The checked state uses schema 4, so the current loader rejects it
as an active revision state. Source-only register publication is independent
of revision snapshots.

Use the [revision commands](../../../../../tools/blobray/docs/project-workflow.md#updating-vendor-artifacts-without-losing-review)
with authenticated inputs to capture a distinct current snapshot. Reviewed
decisions transfer only through the explicit revision comparison/review
contract. Existing checksums and immutable snapshots retain their original
meaning; a new capture does not authenticate another snapshot's inputs.
