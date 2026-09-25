# Interfaces, identities and formats

Reference application owners, durable identities, result assessment and native formats.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Owners and interfaces

| Crate | Responsibility | Internal dependencies |
| --- | --- | --- |
| [domain](../../../crates/domain/README.md) | Identities, revision records, budgets and portable control/stream ports | None |
| [artifacts](../../../crates/artifacts/README.md) | ELF/AR inventory over borrowed captured bytes | Domain |
| [store](../../../crates/store/README.md) | Capture, integrity, persisted run records, publication and owned staging | Domain |
| [knowledge](../../../crates/knowledge/README.md) | Claim validation and review transitions | Domain |
| [verification](../../../crates/verification/README.md) | Concrete observation comparison and verdicts | Domain |
| [analysis](../../../crates/analysis/README.md) | Local CFG, values and memory effects | Domain |
| [riscv](../../../crates/riscv/README.md) | RV32 decoding, lifting and relocation interpretation | Domain |
| [application](../../../crates/application/README.md) | Common supervisor, execution protocol, ordering, thin-member resolution and recovery | Domain, artifacts, store, analysis, knowledge, verification |
| [next](../../src/main.rs) | Rendering, signals, Linux process/cgroup adapter and host composition | Domain, application, backend-riscv |

`Application::new` receives an `OperationHost`; libraries do not discover executables,
install signal handlers or change process-global supervision state.
`Application::start_import` and `Application::start_query` return `RunHandle`
with `status`, `events`, `cancel` and repeatable `wait`. `Application::import`
and `Application::query` are blocking adapters over those same jobs. Import
returns a compact `RunRecord`; retrieve full inventory separately. Query output
transfers once through `take_output`, after worker cleanup. Dropping handles
does not detach operations.

`shutdown` closes admission, cancels active jobs and drains workers without
holding the registration mutex; concurrent shutdown callers wait for teardown.
Application Drop uses the same path. Each project admits one writer and concurrent
readers, with `busy` for a competing import. `ApplicationLimits` defaults to 16
operation/result slots and 64 events per job; `with_limits` accepts 1..1024 slots
and 1..4096 events. Full admission returns `busy`, without queuing. Finished
application-owned jobs are reaped on admission or shutdown. Retained client
handles, transferred output and live plans continue to occupy their slot until
dropped. Plan clones share a slot; each concurrent run additionally needs its own.
Progress cursors are monotonic; the terminal record remains available through
wait, and import records also through persistent `runs` queries.

Inventory query admission resolves current once to an explicit revision ID.
`ReadView` exposes selection, inventory and doctor without writer or recovery
methods. `InventoryView` owns a verified manifest lease, and its byte-source
borrow cannot outlive that lease. Neither exposes the underlying store project.

The worker receives a payload-only staging capability. It captures, parses,
validates the manifest/dependency closure and serializes large results inside
resource containment. It cannot publish through that capability. The coordinator
accepts a bounded receipt, verifies and promotes referenced files using streaming
reads, then publishes. Large inventory never becomes a supervisor message or an
import response. The host/worker protocol is trusted composition, not a sandbox
for arbitrary hostile plugins running under the same OS user.

### Relationship to target interfaces

The [target capability boundaries](../../../docs/design/contracts.md#handles-and-capability-boundaries)
are broader than these callable interfaces. Import, inventory, doctor, selection
and inspection planning/execution share
application admission, worker lifecycle, cancellation and teardown. Queries keep
their status and diagnostics in memory; they never acquire a project writer or
create a run journal. Private job messages and host ports belong to application;
store owns persisted run/owner/receipt schemas, and the Linux adapter interprets
process identities. Human/JSON formats belong to the frontend.

The worker writes typed length-delimited records and a bounded summary into a
private result directory; inventory also captures the validated manifest bytes.
This result protocol is internal to the matching application/host build. It does
not add another archive parser or retained research format. The frontend consumes
borrowed records or the captured manifest without loading the complete result.

`InventoryView` is not a transitive retention pin for future pruning/compaction,
which Next does not implement. Inspection `Plan`, synthetic `LinkPlan` and retained
prepared images, bounded review and concrete comparison are implemented. General
pass planning and extended model/review policies remain outside this profile. Metadata listing,
initialization and recovery do not use the supervised worker path.

## Identity and schema 1

[Domain records](../../../crates/domain/src/lib.rs) define revision serialization:

- `ProjectId` is a persistent random 256-bit identity independent of directory.
- `ArtifactId` hashes exact captured bytes with SHA-256.
- `ObjectId` combines the container artifact with standalone or zero-based archive
  payload ordinal. Archive index/name-table metadata does not consume ordinals.
- `SymbolId` includes object, table kind, table section and entry index.
- `RevisionId` hashes the exact stored JSON manifest bytes, not arbitrary JSON
  reserialization. `RunId` identifies an execution attempt, never its result bytes.

Revision schema 1 retains project/parent, target, producer, ordered roles,
lossless absolute origins, expected digests, captures, thin-member bindings and
inventory. Repeated content may share stored bytes while member occurrences
remain separate. Thin container identity alone does not identify external bytes;
use the selected revision's ordinal-to-capture bindings.

Inventory retains sections, static/dynamic symbols including null/local/weak/
common/undefined entries, raw metadata and REL/RELA references. Malformed names,
invalid references, unreadable members and unknown remaining membership are
explicit diagnostics. Packed RELR decoding and nested archive expansion are
unsupported. Inventory never chooses linker definitions or proves equivalence.
The initial interpretation target is `riscv32-ilp32`; structural ELF32/64 reading
is independent of target execution support.

## Job outcomes and publication

The normal path is registered → running → validating → completed. Failure,
cancellation, timeout and resource exhaustion have distinct terminal states;
recovery marks interrupted nonterminal attempts abandoned.

Cancellation admitted before the commit boundary prevents publication. Once the
coordinator enters the final metadata transaction, cancel returns false and the
transaction determines the outcome. A successful commit atomically records the
revision, advances current and marks the run completed. Lost response delivery
or late cancellation cannot relabel that stored success. Cleanup errors remain
diagnostics and do not revoke a committed revision.

The Linux guard is a dedicated child-subreaper process. It observes a coordinator
lifetime pipe, applies TERM then KILL after the grace period, and reaps descendants
before reporting completion. It retains a shared staging lease during cleanup;
recovery requires an exclusive lease. Owner records include PID, process start
time and boot identity. A PID by itself does not establish that an owner survives.

Worker memory is kernel-enforced or sampled as recorded; coordinator validation
uses fixed-size streaming buffers and bounded control messages. The deadline also
covers coordinator verification before commit. Final commit and durable sync are
not interruptible halfway. Kernel uninterruptible I/O can delay teardown; these
host deadlines are not hard real-time guarantees. Read-only inventory/doctor use owned query workers under the same host guard.
They create no project journal row and never acquire a project writer.

## JSON and checks

`import` returns envelope schema 3 with `run`, including its state, budget/mode,
selected base, resulting revision/completeness and diagnostic. Completed imports
exit 0 even for incomplete inventory; all other run outcomes exit nonzero and
write the run envelope to stderr. Inventory coverage is not a verification verdict.

Typed clients decode `--format json` output with the host library's
`blobray_next_host::wire` module. It covers record documents, run documents and
the inventory document. The renderer emits the same types. Their payloads are
the application and domain records, so clients never copy a request or evidence
schema. The ESP32-S31 vendor scenarios are such a client.

### Current formats

Run records use journal schema 38 for every durable and read operation. Storage metadata
uses schema 39; revision manifests use schema 1 and execution manifests use schema
22. These are independent formats. Storage indexes run state, the execution
published by each run, and each revision input's captured payload, so reads and
executions never scan the journal or walk the inventory to find them. Import
also records, from one full validated walk, each revision's header, input
records and the manifest span of every object. A read scoped to one input or
object, such as validating a knowledge occurrence, decodes only those spans and
verifies the selected capture; every decoded object must carry the requested
identity, so a damaged span fails as an integrity error. A revision without that
index is read by the full walk. Earlier journals are rejected by single-run,
list, recovery and restore readers. `assessment` replaces generic run-level
`complete`/`verdict`; its scoped coverage, optional policy check and optional
comparison are independent of `state`. Empty assessment means the operation has
no research coverage or verdict to assert. A non-completed run has no assessment.

The private import/read-query requests use schema 2; image, function,
investigation, knowledge, execution and concrete scenario requests use schema 1.
Worker reports use schema 6 with tagged receipts and fixed progress counters.
These private protocols require the matching worker binary. Request/control
messages have a 64 KiB encoded limit before allocation can expand the message.
Admission requires working capacity and work policy; diagnostics cannot be
invented while reading unsupported old records.

`runs`/`recover` retain envelope schema 2; `init` uses schema 1. Successful
inventory and doctor output use schema 2 and include `assessment`; inventory
also retains `complete` within its inventory-specific contract and `snapshot`.
Record streams use schema 2 with `records`, `summary` and `assessment`.
Command run envelopes (import 3, function/research 4, investigation 5, knowledge 6,
execution 7) wrap the same schema-38 run; an envelope version is not a journal
version. Partial research and valid comparison verdicts exit 0. Failed or
inconclusive policy checks, including doctor/link-plan blockers, exit nonzero.
Request/admission errors use `{schema:1,error:{code,message}}` on stderr; worker
query failures use `{schema:1,query:WorkerReport}`. Codes are machine interfaces;
descriptive prose is not a parsing key.

```console
cargo test --manifest-path tools/blobray/Cargo.toml -p blobray-domain -p blobray-artifacts -p blobray-store \
  -p blobray-application -p blobray-next
cargo clippy --manifest-path tools/blobray/Cargo.toml -p blobray-domain -p blobray-artifacts -p blobray-store \
  -p blobray-application -p blobray-next --all-targets -- -D warnings
cargo xtask check blobray-standalone
```

Tests use synthetic binaries and isolated fixture processes. The cgroup allocation
test requires real memory delegation and is explicitly ignored in ordinary runs.
Build its executable with `cargo test --manifest-path tools/blobray/Cargo.toml -p blobray-next --lib --no-run`, then run
that printed test executable under a delegated systemd user service:

```console
systemd-run --user --pipe --wait --collect \
  --property=Delegate=memory --property=DelegateSubgroup=coordinator \
  /absolute/path/to/test-executable --ignored \
  --exact linux::tests::kernel_cgroup_enforces_real_memory_allocation
```

This needs a systemd version supporting `DelegateSubgroup`. The standalone check
extracts only the shipping core and runs its tests. It excludes the independent
register source-publication tool.
