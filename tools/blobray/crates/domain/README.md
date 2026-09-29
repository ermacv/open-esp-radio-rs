# Blobray domain

`blobray-domain` owns shared identities, records, resource limits, outcomes and
portable byte/record/control ports. It has no internal crate, filesystem,
database, backend or frontend dependencies.

[Identity types](src/lib.rs) retain content (`ArtifactId`), object and symbol
identities, lossless names, and the `ArtifactInventory` of one executable with
its container kind, member completeness, objects and diagnostics. Recorded
malformed ELF references remain coverage diagnostics.

[Resource ports](src/resources.rs) define `RunControl` with its run position
and phase, work policy 1 and the default limits (`DEFAULT_WORK_UNITS`,
`DEFAULT_TIMEOUT_MS`). The application supplies the control; domain values
enforce nothing.

[Memory contracts](src/memory.rs) provide `WorkingMemory`, RAII reservations,
fallible `ScratchBytes` and `AdmittedVec`, which admits overlapping buffers
before growth. Borrowed scratch cannot outlive its authority, and live
reservations cannot exceed its capacity. These values account requested
capacity, not resident process pages. `MemoryFailure` carries a rejected
reservation with the phase, input and member of the run position, without
requiring memory from the exhausted pool. [`RecordBuffer`](src/record_memory.rs)
owns function records and their admitted variable capacities.

[Streaming ports](src/stream.rs) separate stable positional byte reads
(`ByteSource`) from ELF consumers (`ElfSink`). Callbacks borrow records for one
call; retaining copies requires consumer-owned capacity.

[Image values](src/image.rs) own the `LinkRequest` with its exact root and
companion selections, bounded RV32 regions, linker identity, the
`static-analysis-elf-link-v1` contract, placement/extraction/exit observations
with raw evidence spans and the `ImageManifest`. `RiscvAbi` records the ELF
calling convention separately from the RV32 integer-analysis target.

[Function values](src/function.rs) define physical symbol selectors, extents,
address spaces, per-obligation coverage and the `FunctionDecoder` /
`FunctionSemantics` ports. Typed operations and abstract values describe local
effects without granting access to machine state. `ImageMemory` is a borrowed
immutable-byte port, not a loader or mutable execution bus. Dynamic execution,
inferred function extents and cross-archive definition selection are not
implicit in these values. Calls and possible continuations remain distinct
graph observations.

[Data values](src/data.rs) identify exact captured ranges and their spans; file
offsets and executable VMAs are distinct values. [Register accesses](src/registers.rs)
are observations of analyzed functions, blocked functions and gaps.
[Audit values](src/audit.rs) define forbidden target ranges, findings, the
summary and `CheckVerdict`.

Concrete execution values include exact targets, explicit scenarios, producer
identities, observations and comparison evidence. `Executor` and `ExecutionMemory`
are injected ports; domain selects neither an ISA nor an environment. Request
validation bounds control cardinalities. See the [execution contract](../../cli/reference/execution/README.md#concrete-execution-and-comparison).

Concrete execution uses bounded optional RV32 ABI words: a0–a7 followed
by ascending stack slots. `Invocation::entry_stack` validates placement within the
declared stack; `register_arguments` preserves missing values as unknown. These
are physical words, with type/variadic lowering owned by the caller.

The atomic memory port owns reservation/access/update indivisibility; backend
callbacks supply pure word arithmetic. Ordering bits are explicit inputs. Missing
access is distinct from a failed SC reservation.

Execution cases declare cold/warm resets and per-invocation entry PCs. RAM mappings
declare phase/session lifetimes; the first case must be cold. Targets own captured
address spaces and stack geometry, not one fixed entry.

Invocation goals distinguish return, reach-symbol and observe-call. `ExecutionStart`
lends resolved boundaries to the executor. Completed goal outcomes, premature return
and ordinary gaps remain distinct; `ExecutionStop::completed` means the declared
phase goal was reached. Non-return goals cannot compare return registers.


Device declarations bind caller applicability, phase/session lifetime and every ordered configuration field to a stable content identity. Model observations separate code goals from participation and closure obligations; successful return alone cannot complete an unconsumed transcript.


`CallDeclaration` and `CallObservation` retain exact modeled ABI boundaries, response/effect identities and phase/session obligations. The required call-dispatch port distinguishes captured code from explicit modeled returns and issues; argument words and output ownership are never inferred.

`comparison` defines explicit per-case return/event/memory selections and bounded
final-memory chunks with separate availability/knownness masks. Pair indices bind
same-sized physical selections, not inferred layouts. Execution coverage remains
code-goal/environment completeness; selected unknown outputs prevent comparison
MATCH without converting a successful code phase into a failed session transition.

`LayoutProjection` declares finite source-bound code entries, explicit memory
domains, same-width byte fields and conditional branch correspondence. Selection
names the projection by the digest of its canonical encoding; unknown/unmapped
observations never become equal by omission. These are reviewed comparison
assumptions, without implicit pointer/type normalization.
