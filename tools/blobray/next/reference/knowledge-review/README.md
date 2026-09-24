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
remain hypotheses; review does not authenticate a legacy proof.

```console
blobray knowledge --project PROJECT --limit-mode watchdog validate --change change.json
blobray knowledge --project PROJECT --limit-mode watchdog apply --change change.json
blobray knowledge --project PROJECT --limit-mode watchdog show
blobray knowledge --project PROJECT --limit-mode watchdog history --revision KNOWLEDGE_SHA
blobray knowledge --project PROJECT --limit-mode watchdog export --output knowledge.json
blobray backup --project PROJECT --output project.blobray --limit-mode watchdog
blobray restore --backup project.blobray --project NEW_PROJECT --limit-mode watchdog
blobray import-legacy --request legacy.json --project NEW_PROJECT --limit-mode watchdog
blobray legacy --project NEW_PROJECT --format json --limit-mode watchdog
blobray export-payload --project NEW_PROJECT --id PAYLOAD_SHA --output retained.bin --limit-mode watchdog
```

`Application::start_knowledge` takes a `KnowledgeChange`. `expected_base` is
required, including an explicit `null` for the first proposal. `actor` and
`reason` must be nonempty. The tagged `action` is either
`{"kind":"propose","proposal":...}` or
`{"kind":"review","assertion":"ASSERTION_SHA","decision":"accept","supersedes":null}`.
`decision` also accepts `reject`. An explicit `supersedes` assertion replaces a
conflicting accepted assertion. `validate` runs the same checks in a read query;
it publishes nothing and does not reserve the base against a subsequent writer.
`show` streams assertion states; `history` streams original review events.
Both freeze the current knowledge head at admission unless a revision is given.
`export` writes the history and evidence references as JSON without binary bytes.
It is not a complete private backup. All export destinations must be new.

A proposal contains `subject`, `occurrence`, `claim`, `evidence` and optional
`note`. `occurrence` contains `revision`, `source` (`input` or `image`), `object` and optional `symbol`. Knowledge event manifests use version 2; earlier derived events are not converted.
The claim tags are `name` (a `name` string), `binding`, `function-extent` (an
`extent` range), `hypothesis` (a `text` string), `mmio-region` and `mmio-register`. Evidence tags are:

| Tag | Fields and validation |
| --- | --- |
| `source` | `payload`, `range`: retained object digest and nonempty object-file byte range; ordinary archive members use their exact ordinal |
| `analysis` | `analysis`, optional `record`: retained analysis of the same occurrence; record is a zero-based JSONL ordinal |
| `publication` | `publication`: retained publication containing the occurrence in the same source revision |
| `document` | `payload`: retained provenance/review document bytes; the document does not assert semantic correctness |

Function extents additionally pass the artifact parser's executable section and
range checks. A plan can select accepted boundaries explicitly using
`InvestigationRequest.reviewed_extents`, a list of `{revision, assertion}` pairs.
Each reference must denote an accepted extent at that exact knowledge revision
and the plan's source revision. Explicit and reviewed extents cannot repeat a
physical selector. Plans retain the review references; function computation still
uses the exact source and range. Name changes do not change a function analysis
identity or silently change a saved plan. `status` includes the knowledge head;
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

`LegacyRequest` has `manifest` (the original project TOML), optional `run_spec`,
`roots`, `inputs` and `target` (`riscv32-ilp32`). Paths use the lossless
`OriginPath` encoding. Additional `inputs` use `ImportBinding`: role, origin and
optional expected digest. The schema-1 run spec contributes its ordered exact
role/path bindings. No binary paths are inferred from symbol names. All inputs
must be stable while capturing; the importer never modifies its source project.

The adapter captures the entire manifest directory, including hidden caches,
revision snapshots, reviewed packs, comparison outputs and compiled binding
files present there. It follows the supported TOML file-reference fields and
captures additional caller-selected private roots. Symlink records preserve the
link target and separately capture the target; cycles are deduplicated. Every
file and top-level TOML table/value or array-table item has a catalog outcome:
`converted`, `preserved-unresolved`, `unsupported` or `missing-payload`. The
original file remains available by content digest, including all nested fields,
provenance, schema inputs and recovered hardware/calibration data.

The active conversion is schema-1 code boundaries with exact source digest,
source role, member, section and one matching function entry. It validates the
extent and records the old decision with actor `legacy-import`. Legacy names
remain in the proposal note; they are not automatically accepted as new name
claims. Ambiguous occurrences and conflicting decisions remain unresolved.
Unsupported ABI/interface/register representations and opaque files (including
compressed snapshots) remain verbatim and are not activated. References embedded
inside opaque formats are not interpreted: supply their external payload roots
explicitly. Catalog status reports this limitation; a completed capture does not
mean full semantic conversion or legacy feature parity.

Discovery is iterative and bounded: 16,384 paths, 4 MiB aggregate path bytes,
4,096 bytes per path, TOML nesting 64 and an 8 MiB per-document parser ceiling.
Private project builds admit at most 4 MiB of metadata before another bounded
event, with 16 MiB of reserved SQLite growth/journal headroom.
Exceeding a bound fails the operation without exposing the new project. Working
capacity also admits the path catalog and each parsed document. Temporary
capacity covers retained bytes and destination copying; insufficient capacity
fails rather than discarding evidence. Runtime result inspection permits six
directory levels and up to one million entries. Kernel/watchdog process limits
remain necessary for allocation in third-party libraries; these APIs do not
claim a single mmap arena or a process-wide no-allocation guarantee.

## Reviewed interface declarations

`knowledge validate/apply/accept/show/export` also accepts the native `interface`
claim. It declares a conditional table contract used by `interfaces` queries.
It supplies no runtime model. Existing evidence, expected-base review and physical
occurrence checks apply. Example `claim` within a `KnowledgeChange` proposal:

```json
{
  "kind": "interface",
  "contract": {
    "root": {"kind": "address", "address": 4096},
    "path": [{"kind": "load-pointer", "offset": 0}],
    "layout_version": "reviewed-layout/1",
    "layout_bytes": 16,
    "pointer_bytes": 4,
    "abi": "riscv-integer",
    "index_domains": [],
    "guards": [{"kind": "runtime-value", "offset": 0, "width": 1,
                "mask": 255, "value": 1, "purpose": "required runtime layout tag"}],
    "slots": [{
      "offset": 4,
      "name": "callback",
      "semantic": "project.callback",
      "signature": {
        "arguments": [{"role": "project.argument", "value_type": {"kind": "integer", "bits": 32, "signed": false}}],
        "result": {"kind": "integer", "bits": 32, "signed": false},
        "variadic": false
      }
    }],
    "purpose": "Explicit conditional callback interface",
    "applicability": "Selected captured occurrence with the declared runtime tag"
  }
}
```

Roots can instead be `symbol` with a physical `SymbolId` and signed `addend`, or
`entry-word` with an exact `FunctionSelector` and zero-based physical ABI `word`,
or `section` with a physical section index and byte offset.
Symbol-less function ranges are valid argument contexts. An address root is a
literal RV32 address scoped by the occurrence, not an offset into the ELF file.
Paths contain explicit `offset`, `load-pointer` and `index` steps. Each index
names an incoming ABI word and byte stride and requires a unique inclusive `min`/`max`
domain with a reason. Domains are declared caller preconditions, not inferred
values. Word positions 0..63 can be declared: 0..7 are a0..a7, and 8 starts at entry SP.
They are not logical signature argument ordinals. This does not assert execution
support for stack arguments or signatures.

The declaration profile has four-byte pointers, aligned nonoverlapping slots,
RV32 integer ABI, nonvariadic signatures, 8/16/32/64-bit integers, pointers and a
void result. A slot may have `signature: null` when its call signature is unknown;
this preserves a structural observation without inventing argument or return types.
At most 64 slots, 32 arguments per signature, 16 path steps, 16 guards
and eight index domains fit within the existing 64 KiB knowledge-event limit.
A `captured-payload` guard must match the exact object digest (including detached
thin-member identity). Runtime byte/halfword/word guards are bounds/mask checked;
contradictory overlapping bits fail validation. Their acceptance never means the
runtime condition was observed or satisfied. Semantic keys are reviewed labels;
they select neither machine-code callees nor external execution models.

Changed declarations for the same subject/path or provably overlapping static
root ranges conflict during acceptance. Different dynamic dereference paths are
distinct declared identities; static validation does not prove runtime non-aliasing.
Export retains review events and evidence references, not a transitive binary
backup. Use project backup/restore to preserve the captured research.


## Interface discovery and selected bindings

`interfaces --request query.json [--output observations.json]` is one supervised
read query. The optional output is an atomic JSON export to a new file. Without
it, normal human/JSON output selection applies. Example request over saved facts:

```json
{
  "input": {"kind": "analysis", "analysis": "<analysis-id>", "abi": "riscv-integer"},
  "knowledge": null
}
```

Alternatively `input` is `{"kind":"data","occurrence":<KnowledgeOccurrence>,
"selector":<DataSelector>,"layout":{"count":4,"stride":4}}`. This reads exact
captured pointer bytes through the same occurrence/relocation owner as `data`.
`captured_span` preserves section, digest and writable-initialization classification;
pointer values distinguish null, physical symbols, numeric addresses, external
symbols and unresolved transformations. Slot offsets are relative to the selected
root. Captured pointer contents do not assert function boundaries or live values.

Analysis discovery reads retained instructions, call inputs and expression facts,
including calls in ET_REL objects. It never schedules analysis or linking. Paths
retain literal/section/symbol/entry-word roots, dereferences, byte offsets,
bounded scaled entry-register indices and the final pointer slot. The profile
requires an explicit integer ABI for a0..a7 and four-byte incoming stack-word loads.
Missing facts, unsupported expressions, call results, non-pointer loads and path
bounds produce explicit issues. A known or finite call destination can lack a
retained pointer-load expression: `no-pointer-path` retains that target and does
not guess its originating table. A path is a structural may-observation, not proof
of an executable route or of runtime index/guard satisfaction.

`knowledge: null` means no declarations. A revision ID selects exactly that saved
snapshot, never the current head implicitly. The query indexes physical occurrence,
root/path and slot once. Bindings retain assertion IDs and proposed/rejected/accepted
states; only accepted matches contribute to `matched_accepted`. Multiple accepted
candidates remain visible and increment `ambiguous_bindings`. Runtime guards/index
domains yield `runtime-conditions-unverified`; neither acceptance nor matching runs
a model, resolves a callee or satisfies those conditions. Unknown signatures and
semantic keys remain null. Summary counts have no general `complete` or PASS claim.

To review a discovered path, propose an `interface` claim using the summary's exact
occurrence and payload, the observation's path/slot and its source analysis record
as evidence. Use the existing validate/apply/accept lifecycle, then repeat the query
with that explicit knowledge revision. Export preserves these identities and review
states; retain the project for transitive evidence and source-free reopening.


## Reviewed function and context contracts

The `function` knowledge claim uses the same `knowledge validate/apply/accept/show/export`
lifecycle as interface declarations. Its `selector` is an exact symbol or executable
range in the proposal's captured occurrence. Example claim (substitute the physical
selector obtained from inventory or a saved function recipe):

```json
{
  "kind": "function",
  "contract": {
    "selector": {"kind": "symbol", "symbol": "<physical SymbolId object>"},
    "abi": "riscv-integer",
    "signature": {
      "arguments": [{"role": "project.context", "value_type": {"kind": "pointer", "nullable": false}}],
      "result": {"kind": "void"}, "variadic": false
    },
    "name": "copy_field", "role": "project.copy", "return_role": null,
    "summary": "Conditional context-field interpretation",
    "contexts": [{
      "argument": 0, "name": "state", "start": 0, "length": 12,
      "fields": [
        {"offset": 4, "width": 4, "name": "input", "value_type": {"kind": "integer", "bits": 32, "signed": false}, "access": "read", "role": null},
        {"offset": 8, "width": 4, "name": "output", "value_type": null, "access": "write", "role": null}
      ]
    }],
    "preconditions": [{"kind": "argument-bits", "argument": 0, "mask": 3, "value": 0, "reason": "Caller supplies aligned storage"}],
    "applicability": "Selected captured function and declared caller obligations"
  }
}
```

Wrap the claim in a `KnowledgeChange` proposal with exact occurrence and retained
source/analysis evidence, then validate and review it. Data symbols, stale physical
selectors, invalid fields, contradictory predicates and incompatible accepted
contracts fail before publication. Explicit review supersession is required to
replace a conflicting interpretation; there is no implicit merge of field layouts.

`signature: null` leaves the call signature unknown. The shared signature profile
supports up to 32 integer/pointer arguments; declaration does not assert execution
support for every ABI placement. Function/argument/return roles are reviewed subject
keys. A return role needs an explicit non-void return. Context arguments are zero
based; a known signature must identify them as pointers. Signed context starts
allow explicit prefixes before the argument pointer. Fields must fit their context,
have width 1/2/4/8, unique names and nonoverlapping ranges. Known field types must
match width; null preserves an unknown type. `read`, `write` and `read-write` are
asserted roles, not access permissions or observations.

Preconditions are `argument-range` (inclusive `min`/`max`), `argument-bits`
(`mask`/`value`), `context-bits` (`argument`, signed `offset`, `width`, `mask`, `value`)
or `assumption` (`id`, `statement`). Each has a required `reason`. Argument predicates
require a known signature and operate on unsigned raw bit patterns, including for
signed ABI types. Non-null pointers exclude zero. Context bits use little-endian
byte interpretation; overlapping contradictory requirements are rejected. Named
assumptions are uninterpreted, not evaluated expressions. Limits are 16 contexts,
128 fields total and 64 predicates, within the common 64 KiB admission message.

Acceptance preserves a conditional interpretation and its evidence. It neither
changes saved analysis facts nor marks preconditions as observed. Explicit-revision
knowledge queries/exports work after removal of source archives and image inputs;
project backup is still required for the complete retained evidence closure.
