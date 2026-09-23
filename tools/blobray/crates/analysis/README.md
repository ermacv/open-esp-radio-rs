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
without acquiring source-selection authority. Its owned output retains a memory
reservation. Imported expressions retain analysis provenance; transitive memory
records are may-effects, not unconditional traces. Unknown callees retain gaps.
