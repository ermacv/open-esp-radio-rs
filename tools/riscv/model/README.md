# RV32 program model

`oer-riscv-model` owns the identities, errors, run control, working memory
and the ISA-neutral function decoding and lifting contracts that the
[analysis](../analysis/README.md), the [RV32 lifting](../lift/README.md) and
Blobray share. It has no internal dependency, no filesystem access and no ISA.

[Identity types](src/lib.rs) retain content (`ArtifactId`), object and symbol
identities, and the shared `Error`, `ErrorCode` and `Result`.

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

[Function values](src/function.rs) define physical symbol selectors, extents,
address spaces, per-obligation coverage and the `FunctionDecoder` /
`FunctionSemantics` ports. Typed operations and abstract values describe local
effects without granting access to machine state. `ImageMemory` is a borrowed
immutable-byte port, not a loader or mutable execution bus. Dynamic execution,
inferred function extents and cross-archive definition selection are not
implicit in these values. Calls and possible continuations remain distinct
graph observations.
