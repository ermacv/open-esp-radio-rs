# Knowledge and review

Retain explicit assertions, applicability and review decisions for a captured revision.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Knowledge and preservation

Source `RevisionId`, `KnowledgeRevisionId` and analysis/publication IDs have
different meanings. A knowledge revision is an immutable review event with its
expected parent, project, assertion, actor, reason and retained evidence roots.
The head is the last committed event. Proposal, review and head advancement share
one transaction with the durable run outcome. A stale base returns `conflict`;
failure before commit leaves the previous head readable. Superseded and rejected
assertions, evidence and historical revisions remain retained; there is no GC.

The pure [knowledge crate](../../../crates/knowledge/README.md) owns claim and transition
rules. Application owns occurrence/evidence validation and review orchestration.
Store owns content identities, event history and transactional publication. CLI
only supplies the same typed requests used by API clients. Accepted hypotheses
remain hypotheses; review does not authenticate a proof.

```console
blobray knowledge --project PROJECT --limit-mode watchdog show
blobray knowledge --project PROJECT --limit-mode watchdog accept --base PROPOSAL_REVISION --assertion ASSERTION_ID --actor researcher --reason "Checked exact source evidence"
blobray backup --project PROJECT --output project.blobray --limit-mode watchdog
blobray restore --backup project.blobray --project NEW_PROJECT --limit-mode watchdog
blobray export-payload --project NEW_PROJECT --id PAYLOAD_SHA --output retained.bin --limit-mode watchdog
```

Proposals enter through the typed commands `knowledge propose-effect-contract`,
`propose-projection` and `propose-call-pair`, described with their
[comparison relations](../comparison/README.md). `accept` reviews one proposed
assertion against an explicit `--base`; it cannot silently supersede a conflict.
`show` streams assertion states and freezes the current knowledge head at
admission unless a revision is given.

`Application::start_knowledge` takes the underlying `KnowledgeChange`.
`expected_base` is required, including an explicit `null` for the first
proposal. `actor` and `reason` must be nonempty. The tagged `action` is either
`{"kind":"propose","proposal":...}` or
`{"kind":"review","assertion":"ASSERTION_SHA","decision":"accept","supersedes":null}`.
A proposal contains `subject`, `occurrence`, `claim`, `evidence` and optional
`note`. `occurrence` contains `revision`, `source` (`input` or `image`), `object`
and optional `symbol`. Knowledge event manifests use version 2; earlier derived
events are not converted. Evidence tags are:

| Tag | Fields and validation |
| --- | --- |
| `source` | `payload`, `range`: retained object digest and nonempty object-file byte range; ordinary archive members use their exact ordinal |
| `analysis` | `analysis`, optional `record`: retained analysis of the same occurrence; record is a zero-based JSONL ordinal |
| `publication` | `publication`: retained publication containing the occurrence in the same source revision |
| `document` | `payload`: retained provenance/review document bytes; the document does not assert semantic correctness |

`status` includes the knowledge head;
it reads publication metadata rather than rehashing every child analysis.
`doctor` performs closure validation.

Backup takes a SQLite read snapshot and copies immutable CAS payloads. Concurrent
publication cannot change that database snapshot; the bundle may include extra
unreferenced objects added during copying. The version-1 private bundle includes
an entry length and SHA-256 for the database and every payload. Restore verifies
entry identities, rejects duplicate entries, trailing bytes and path substitutions,
and runs doctor before delivery. Restored unfinished runs become `abandoned`;
the original database bytes remain in CAS. Imported revisions, completed results
and review events retain their identities. Restore rejects unsupported metadata and journal formats without converting them.

Backup and new-project preparation run in the common supervised query lifecycle.
The caller owns one result slot through delivery. Delivery copies to a private
sibling destination under the remaining time/work/disk budget, syncs it, and
exposes `.blobray-next` only after completion. The destination directory must not
exist. Cancellation or a copy failure cannot replace an existing project. A
crash at exposure can leave an empty destination or a complete state directory;
there is no partial project publication. A post-publication directory-sync error
is reported and may leave a complete destination requiring verification.
