# Preservation

Back up a project, restore it into a new directory and export retained payloads.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Backup and restore

```console
blobray backup --project PROJECT --output project.blobray --limit-mode watchdog
blobray restore --backup project.blobray --project NEW_PROJECT --limit-mode watchdog
blobray export-payload --project NEW_PROJECT --id PAYLOAD_SHA --output retained.bin --limit-mode watchdog
```

Source `RevisionId` and analysis, publication and execution IDs name different
retained results. `status` reads publication metadata rather than rehashing every
child analysis. `doctor` performs closure validation.

Backup takes a SQLite read snapshot and copies immutable CAS payloads. Concurrent
publication cannot change that database snapshot; the bundle may include extra
unreferenced objects added during copying. The version-1 private bundle includes
an entry length and SHA-256 for the database and every payload. Restore verifies
entry identities, rejects duplicate entries, trailing bytes and path substitutions,
and runs doctor before delivery. Restored unfinished runs become `abandoned`;
the original database bytes remain in CAS. Imported revisions and completed
results retain their identities. Restore rejects unsupported metadata and journal
formats without converting them.

Backup and new-project preparation run in the common supervised query lifecycle.
The caller owns one result slot through delivery. Delivery copies to a private
sibling destination under the remaining time/work/disk budget, syncs it, and
exposes `.blobray-next` only after completion. The destination directory must not
exist. Cancellation or a copy failure cannot replace an existing project. A
crash at exposure can leave an empty destination or a complete state directory;
there is no partial project publication. A post-publication directory-sync error
is reported and may leave a complete destination requiring verification.
