# Bounded function control-flow and value analysis

Owns a bounded, iterative local CFG, typed output and independent coverage
for section-relative objects and image-addressed functions. It
receives borrowed captured code, structural relocation records, an injected ISA
port, working capacity, control and a result sink. It has no store or filesystem
authority. Edges to callees never schedule analysis of another function.

The private `values` module owns flat-lattice register states and a bounded
fixed-point queue. It emits values and accesses only after convergence, through
the same borrowed sink as the CFG. Input-dependent allocations share the caller's
working capacity, and all repeated visits share its work/deadline budget.
See [value semantics and limits](../../next/README.md#values-and-memory-effects).

An optional borrowed `ImageMemory` port supplies immutable load bytes. Static
ELF permissions qualify these constants; writable/unmapped memory stays unknown.
Image PC-relative values use virtual addresses, and saved transfer records expose
resolved and unresolved calls/outgoing jumps without expanding the local CFG.
No second analyzer or machine executor is selected for linked code.

The same value solver can emit a flat expression DAG, entry-register values,
symbolic loads, conditions and returns. Chunk reservations live with the DAG;
loops use the bounded monotone state queue. Explicit ABI context controls only
integer call preservation. ELF ABI flags never select that assumption.

The `summaries` module composes supplied acyclic callees and substitutes arguments
without acquiring source-selection authority. Its owned output uses a capacity-admitted `RecordBuffer`; mapping/return
workspaces end when composition returns. Imported expressions retain analysis provenance; transitive memory
records are may-effects, not unconditional traces. Unknown callees retain gaps.

The `audit` module scans all supplied executable ranges linearly for direct and
locally resolved transfers. It shares the ISA port and integer folding with value
analysis; it does not obtain filesystem access or claim dynamic-target completeness.

`PreparedReferences` retains one normalized reference table per prepared section. Sorted offset, symbol and physical-pair indexes replace repeated whole-table scans while preserving nonadjacent HI/LO ambiguity and exact physical identities.
