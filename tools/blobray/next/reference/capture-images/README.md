# Capture, selection and linked images

Capture caller-owned artifacts, choose exact code/data scope and prepare linked images.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Use

Run from the repository root with the pinned Rust toolchain:

```console
cargo blobray init --project /path/to/investigation
cargo blobray import --project /path/to/investigation \
  --cgroup-root /sys/fs/cgroup/path/to/delegated-parent \
  --input vendor=/path/to/libvendor.a --input companion=/path/to/companion.o
cargo blobray inventory --project /path/to/investigation \
  --cgroup-root /sys/fs/cgroup/path/to/delegated-parent --format json
cargo blobray runs --project /path/to/investigation
cargo blobray revisions --project /path/to/investigation
```

`import`, `inventory`, `doctor`, `select`, `plan`, `run`, `link-plan`,
`prepare-image`, `images`, `image`, `export-image`, `analyze-function`,
`analyses`, `analysis` and `export-analysis` require Linux containment in the CLI. The default `--limit-mode kernel` requires a writable
cgroup v2 parent delegated to this user, with the memory controller available and
no processes in the parent when enabling that controller. `--cgroup-root` selects
that parent; without it the host tries its current cgroup. The adapter enables
`+memory` in the delegated parent, creates a per-run child and moves the worker
there before exec. It sets `memory.max`, disables swap for the worker group,
and enables group OOM handling. An unavailable backend fails before worker
execution; it never selects watchdog automatically.

Where kernel containment is unavailable, explicitly select
`--limit-mode watchdog`. This samples the worker tree's RSS, including descendants
that enter another session. A short-lived memory spike can exceed the budget
between samples; the run record identifies this mode. Both modes own descendant
cleanup and use the same application cancellation/publication contract.

Defaults are a 4096 MiB process limit, 256 MiB working capacity, 900 seconds,
1,000,000,000 work units, a 100 ms sampling interval and a 10 second termination
grace. Operation commands accept `--memory-mib`, `--working-memory-mib`,
`--timeout-secs`, `--max-work-units`, `--poll-ms` and `--grace-ms` (positive).
The defaults are the `DEFAULT_*` constants of `blobray-domain`. The sampling
interval bounds only resource observation: supervisors wait for process exit
directly, so a finished operation is not delayed by a sampling period.
Ctrl+C or SIGTERM requests cancellation. SIGKILL cannot produce a normal terminal
response; the guard detects the closed coordinator pipe and stops its worker tree.

Repeat `--input ROLE=PATH` in the intended order. Roles are nonempty UTF-8 labels;
duplicate roles, paths and member names are allowed. Native source paths and
archive/symbol names retain their bytes (UTF-16 units for Windows origin records).
Human rendering can be lossy; JSON byte arrays preserve original names exactly.
`--expect INDEX=SHA256` attaches an expected digest to a zero-based occurrence.
Digests contain 64 lowercase hexadecimal digits. Captured mismatches fail import;
unavailable inputs retain their unfulfilled expectations and diagnostics.

`inventory --revision SHA256` selects a committed revision, otherwise current.
Old inventory remains identical after another import, source deletion or moving
the entire project. Reading never imports changed files or reruns the parser.

## Synthetic prepared images

`link-plan` validates a selected revision and creates a schema-2 `LinkRecipe` in a `LinkPlan`.
`prepare-image` materializes its captured inputs, invokes the explicitly selected LLD or GNU BFD ld,
validates the output and atomically publishes an immutable prepared image and
completed run. It never changes the current import revision. A ready plan means
supported inputs and unambiguous roots; it does not promise that all references
will resolve or that the requested layout is large enough. Those checks also run
during preparation. Plans with blockers can be saved and inspected but not run.

The request is JSON with `revision` (ID or null to freeze current at admission),
ordered `inputs` (zero-based imported binding ordinals), `entry`, optional `roots`
and `layout`. An entry/root is `{ "input": 0, "symbol": <SymbolId> }`; obtain the
complete physical `SymbolId` from `select` or inventory. Names are not selectors.
The layout has `code` and `data`, each with numeric `start` and `length`, for example
`{"code":{"start":268435456,"length":65536},"data":{"start":536870912,"length":65536}}`.
Regions must be nonempty, start on 4096-byte boundaries, fit RV32 and not overlap.

```console
cargo blobray link-plan --project /path/to/investigation \
  --request /path/to/link-request.json --linker /usr/bin/ld.lld \
  --output /path/to/link-plan.json --limit-mode watchdog
cargo blobray prepare-image --project /path/to/investigation \
  --plan /path/to/link-plan.json --linker /usr/bin/ld.lld --limit-mode watchdog
cargo blobray images --project /path/to/investigation \
  --limit-mode watchdog --format json
cargo blobray image --project /path/to/investigation \
  --id <prepared-image-id> --limit-mode watchdog --format json
cargo blobray export-image --project /path/to/investigation \
  --id <prepared-image-id> --output /path/to/new-directory --limit-mode watchdog
```

The application equivalents are `start_link_plan`/`link_plan`,
`RunHandle::take_link_plan`, `LinkPlan::write`, `read_link_plan`,
`start_prepare_image`, and `ReadQuery::{Images,Image}`. Plan clones retain one
application slot, immutable captured manifest and temporary workspace. Preparing
uses another slot and revalidates the frozen revision and all selected captures
in the explicitly selected project. Dropping a plan does not cancel an admitted
image job. A saved description needs that project's retained source closure;
it cannot reopen original source paths. The store currently retains all revisions
and images without pruning. Reopening an image verifies retained digests and
needs neither the linker nor original input files. Project relocation preserves
identities. An exported bundle contains `image.elf`, `manifest.json`, `link.map`,
`extraction.raw`, `provenance.jsonl` and `observations.jsonl`. Export requires a nonexistent destination,
writes the manifest last and never overwrites an existing destination. A failed
export can leave an incomplete directory; its presence alone does not mean success.

### Link policy and evidence

Link policy 5 accepts little-endian RV32 ILP32/ILP32F/ILP32D relocatable objects, ordinary
archives and captured thin archives. Companion code and data participate in the
same link. Every selected payload must be supported; malformed, missing, TLS,
RV32E and quad-float inputs block preparation. Linked firmware/ROM ELF bindings as linker inputs,
dynamic linking, arbitrary scripts and caller-supplied linker flags are not
supported. Root symbols must be defined global/weak executable symbols with
unambiguous static-table and section identity. Input names containing CR/LF are
rejected because the linkers' unescaped maps cannot safely represent them.

Each member is materialized once as `i<input>-m<ordinal>.o`; duplicate archive
names and byte-identical imported bindings remain distinct occurrences. Root
objects are forced first in entry/root order, deduplicated by occurrence and
removed from their original lazy groups. Remaining archive members use ordered
`--start-lib`/`--end-lib` groups; standalone objects are direct inputs. Archive
indices are not reused. This explicit synthetic selection policy can differ from
the producer's original link and never establishes original firmware selection.

The generated script defines CODE and DATA regions, RX/RW load segments,
text/rodata/eh-frame, data/sdata and bss/common sections and the RISC-V global
pointer. Both adapters use `elf32lriscv`, section GC, emitted relocations,
no relaxation, no build ID, no demangling and strict undefined-symbol handling.
LLD also explicitly disables ICF and uses one thread. The environment contains
only `LC_ALL=C` and `TZ=UTC`; core dumps are disabled. Other sections follow the
selected tool's placement rules and must pass the same post-link validation.
No PATH/rustc search, archive regrouping or linker substitution occurs.

`ElfLinker` recognizes the family using `--version` and exercises the actual
adapter with two bounded synthetic RV32 links before accepting it. Their ELF,
raw evidence, normalized observations and stderr digests must agree. Each probe
output channel is capped at 64 KiB and observations at 128 records. The probe checks
root extent/layout, garbage collection, retained relocations and normalized
placement/extraction evidence through the production validators. Version numbers
are not an allowlist: missing `elf32lriscv`, `--start-lib` or required evidence
rejects that executable with `Incompatible`. The version's first line and SHA-256
enter identity and are checked again at preparation. Executable hashes are also
checked before and after linking. This does not snapshot dynamically loaded
system libraries.

GNU ld processes archives in order; LLD can satisfy backward references from
previous archives. Both implement `static-analysis-elf-link-v1`, but their
selected members, success/failure and image IDs can differ. Blobray preserves
these differences and never claims original firmware selection.

`LinkPlanId` hashes the schema/policy, project, revision, input order, exact roots,
layout, linker contract and linker identity. Recipe schema 2 / policy 5 fixes accepted ABI families, explicit ROM definitions, executable-section placement, transformation, script generation,
flags and environment. Time, memory and disk budgets and local paths do not enter
this identity. Existing inspection Plan schemas and IDs keep their own contracts.
`PreparedImageId` hashes its manifest, which references the ELF, raw map, extraction
report, normalized observations and provenance by content digest. Image manifest
schema 3 records this closure. The manifest labels the image
synthetic and retains verified roots/segments and bounded successful tool stderr.

Validation requires RV32 ET_EXEC, bounded nonoverlapping PT_LOAD segments inside
the declared regions, no writable executable segment, allocated sections covered
by load segments, file-backed executable roots and no unresolved symbol relocation
in allocated sections, including unresolved weak references. The selected entry
must equal `e_entry`. Empty PT_LOAD records are retained without mapping memory;
nonzero file bytes still require a sufficient memory extent. Root addresses are
proved by normalized input-section placements plus
source symbol offsets, unchanged section sizes and matching output symbols.
Hidden symbols may be localized by LLD. Symbol rows cannot impersonate section
rows. No exact root mapping means no publication. Other map records retain their
reported source occurrence with `exact: false`; they are observations, not a
complete instruction-by-instruction provenance proof. Execution and comparison
are not implemented by image preparation.

### Ownership, limits and persistence

Domain owns recipes/IDs and typed observations; artifacts owns input and output
ELF validation. Application owns selection, materialization, semantic policy,
root proof and orchestration. Linux adapters own CLI/script dialects and parsers
behind `LinkerHost`. The common subprocess helper owns bounded transport and
process lifetime. Store owns payload leases, `PreparedImageReceipt`, `RetainedImage`
and atomic publication; adapters never select project inputs or publish images.

`SectionPlacement` and `ArchiveExtraction` identify the input binding and ObjectId,
including repeated imports, and reference byte spans in retained raw map or
extraction output. Application validates occurrence identity, extraction coverage,
root extent and exit observations. Records use schema 1 and are retained in
extraction/placement/exit order independent of pipe scheduling. `image` queries
emit `link-observation` records; exported `observations.jsonl` contains the same
typed records. LLD extraction evidence comes from why-extract; GNU extraction
comes from the map, so its separate `extraction.raw` is empty. Raw outputs remain
available for provenance without application parsing either tool's text.

The same operation guard contains worker and linker. LLD ELF, map and extraction
flow through concurrently drained pipes into quota-admitted `TemporaryFile`s.
GNU needs a seekable ELF: application reserves its maximum extent before launch,
using the available working memory at image validation after fixed metadata.
This capacity must be available on disk even when the final ELF is smaller.
The adapter enforces the ceiling with hard `RLIMIT_FSIZE`, reaps the writer, and
reconciles actual length before transferring the file. Unused reservation is
released then; failed cleanup retains the charge. No new CLI limit is required.
Map and diagnostics remain bounded streams. Materialized inputs/scripts, raw
outputs and normalized evidence share the operation's temporary budget.

LLD buffers stdout ELF in process memory, covered by the process limit rather
than Blobray `WorkingMemory`. Inspection and validation admit one full object/image
buffer at a time. An 8 MiB metadata reservation covers up to 512 bindings,
4096 members, 64 roots (`MAX_IMAGE_ROOTS`), 32 blockers and the bounded capability probe. Root
names/sections are at most 4096 bytes, map/observation records 64 KiB, saved plan
metadata 60 KiB, image manifest 56 KiB and stderr tail 8192 bytes. Capacity
exhaustion fails explicitly without partial publication.

Durable run records identify the admitted operation and its published result;
query status stays in memory. Payload promotion precedes one transaction for
image plus completed run. Failures may leave unreferenced CAS objects, never a
listed partial image. Cancellation is linearized before commit; a committed
result remains successful even if response delivery is lost. Recovery abandons
interrupted attempts and never converts staged ELF into success. Doctor checks
retained image closures. See [JSON and checks](../interfaces-formats/README.md#json-and-checks) for versions.

Real-link acceptance requires both LLD (reference 22.1.8) and GNU ld (reference
2.47 with RV32), installed on the host. Set `BLOBRAY_TEST_LLD` to override
`/usr/bin/ld.lld`. `BLOBRAY_TEST_GNU_LD` selects the GNU executable; without it,
tests use the first of `riscv-none-elf-ld`, `riscv32-unknown-elf-ld`, `riscv32-esp-elf-ld`,
`riscv64-unknown-elf-ld`, `riscv64-elf-ld` and `riscv64-linux-gnu-ld` on `PATH`.
The adapter always selects `elf32lriscv` and passes archive inputs with
`--start-lib`/`--end-lib`, so any RISC-V cross GNU ld from binutils 2.47 or later
qualifies. Earlier releases, including ESP-IDF's binutils 2.46 toolchain, and a
GNU ld with only x86 emulations fail the capability probe. Missing tools fail
tests. Tests use synthetic RV32 inputs, not hardware qualification.

## Selection and inspection plans

`select` searches exact name bytes and streams all matching candidates. It never
chooses the first match. UTF-8 `--name` and byte-exact `--name-hex` are mutually
exclusive; `--input` restricts the zero-based input binding. A candidate includes
its revision, complete `scope` selector and captured payload digest when known.
Names, roles and source paths are not identities. Empty names are permitted.

```console
cargo blobray select --project /path/to/investigation \
  --kind symbol --name same --limit-mode watchdog --format json
cargo blobray select --project /path/to/investigation \
  --kind symbol --name-hex 6c6f63616cff --input 0 --limit-mode watchdog
cargo blobray plan --project /path/to/investigation \
  --request /path/to/request.json --output /path/to/plan.json --limit-mode watchdog
cargo blobray run --project /path/to/investigation \
  --plan /path/to/plan.json --limit-mode watchdog --format json
```

The request is the same `PlanRequest` accepted by the application API. Example
for inspecting one input binding (set `revision` to a selected digest to avoid
using current at admission):

```json
{
  "revision": null,
  "scope": {"kind": "input", "input": 0},
  "budget": {
    "mode": "watchdog",
    "memory_bytes": 4294967296,
    "working_memory_bytes": 268435456,
    "timeout_ms": 900000,
    "grace_ms": 10000,
    "poll_ms": 100,
    "max_work_units": 1000000000,
    "work_policy": 1
  }
}
```

Other scopes are `{"kind":"revision"}`, object (`input` plus `object: ObjectId`)
and symbol (`input` plus `symbol: SymbolId`). Copy the candidate's `scope` and
`revision` into the request rather than resolving its name again. This preserves
repeated input bindings and static/dynamic symbol tables. Identical thin archive
bytes can select different payloads in different inputs or revisions.

Object inspection returns its input/object headers, sections, symbols,
relocations, thin binding and diagnostics. Symbol inspection returns one exact
symbol with its input/object context, defining section when identified, referring
relocations and applicable diagnostics. Undefined/common/absolute symbols do not
invent a defining section. Input headers and object ELF headers in callbacks have
empty nested arrays; following records supply those values. No linker definition
selection, disassembly or behavioral analysis occurs. `revision_complete` always
describes the full source inventory, even when a selected subset is inspectable.

`Application::start_plan`/`plan` create an immutable `Plan` under a separately
supplied planning budget. `RunHandle::take_plan` transfers it once; wait remains
repeatable. The plan owns a private captured manifest with no writer access.
Clones share that ownership and admission slot. `start_run(&Plan)` acquires its
own ownership until worker teardown and uses the captured manifest, even after
the original project path disappears. It does not reparse binary inputs or
inspect current. Last release removes the private manifest. This metadata lease
is not a transitive payload pin for future garbage collection.

`PlanDescription` contains an ID and recipe: schema/operation/result versions,
project and revision, exact scope, target, inventory producer, selected capture
binding, revision coverage and execution budget. Whole-revision dependencies are
bound by the manifest digest rather than an in-memory list. `PlanId` is SHA-256 of
the compact typed JSON recipe in serializer field order; input key order and
whitespace do not affect it. IDs include execution budgets, but exclude project
paths, time and attempt IDs. They establish content identity, not authenticity.
This is an inspection recipe identity, not a future semantic computation/cache key.

`PlanDescription::read` bounds the complete file to 64 KiB and checks versions
and ID. `start_reopen_plan`/`reopen_plan` additionally verify project membership,
retained dependencies and exact recipe agreement before granting a live Plan.
They never substitute current or replan a changed description. A missing target
returns `not-found`; corruption is an integrity error. Missing/unsupported
inventory remains inspectable with coverage diagnostics, not a fabricated
analysis capability.

`Plan::write` delivers the portable description once under the remaining planning
budget; writing a destination does not modify the project. CLI `--output` stages
and publishes that file without overwriting an existing file. Without `--output`,
`plan --format json` writes the description to stdout; human mode displays a
bounded summary. Saving is explicit and no plan database is created.

Every `start_run` gets a new `RunId` and deadline using the saved execution budget.
CLI `run` limit flags control reopening only; execution cannot silently override
the recipe. Reopening defaults to kernel containment even when the saved execution
budget requests watchdog, so select watchdog explicitly where delegation is absent.
Within each operation, work/deadline accounting continues through output delivery.
Planning, reopening, selection and inspection do not create durable run journals.

`select` and `run` JSON output use `{schema:2,records:[...],summary:{...},assessment:{...}}`. Records
carry `kind` and `value`; candidates carry complete selectors. Both the records
array and human records are streamed. Zero matches is a successful search with
coverage reported; precise planning of an absent occurrence fails. Consumer
failure is terminal and cannot be converted to a coverage gap or retried implicitly.
