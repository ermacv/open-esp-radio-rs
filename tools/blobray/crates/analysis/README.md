# Bounded function control-flow and value analysis

Owns a bounded, iterative local CFG, typed output and independent coverage
for section-relative objects and image-addressed functions. It
receives borrowed captured code, structural relocation records, an injected ISA
port, working capacity, control and a result sink. It has no store or filesystem
authority. Edges to callees never schedule analysis of another function.

`registers` recognizes bounded saved load/mask/store expression shapes using
the shared borrowed fact index. It reports bit-selection observations with their
original physical load width; application owns address scope and knowledge matching.
It never infers register geometry, hardware field meaning or safe RMW semantics.

The private `values` module owns finite-lattice register states and a bounded
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
symbolic loads, conditions and returns. Record and payload reservations live with the DAG;
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

`pointers` streams explicitly selected captured pointer slots using structural
write bounds and the injected `PointerDecoder`. It performs no name lookup,
relocation application, project I/O or table-sized result allocation. Physical
symbol references, numeric addresses, null and unresolved transformations remain
distinct; the caller owns the prepared bytes and output stream.

`value_sets` interns canonical sets of at most eight exact leaves in admitted
operation-local vectors and a bounded-load lookup index. It does not enlarge each
register state with an inline array. Joins and Cartesian arithmetic widen explicitly
on a ninth result; stored `alternative-limit` gaps distinguish this loss from a
resolved singleton. Public alternatives are nonrecursive and validated on decode.
Callee composition qualifies every retained image-address alternative; it never
silently removes an unknown callee stack possibility or chooses one callback.

`interfaces` follows saved indirect-call expressions iteratively and canonicalizes
physical root/path/slot keys. It retains bounded alternative paths and explicit
unsupported/missing-provenance issues, with admitted per-query indexes. It neither
loads knowledge nor chooses an accepted binding; application owns those decisions.

Saved navigation indexes one flat function record stream and interprets calls and
physical access paths without storage, linking or binding authority. Cycles are
reported as structural edges; unknown addresses remain explicit.

The flow module computes bounded iterative reachability over caller-selected
unambiguous arcs. It returns predecessor indexes and depths; storage and reviewed
path authority stay with application/knowledge.


`memory_slice` borrows saved facts, constructs admitted instruction/access indexes
and iteratively finds last local writes before an exact anchor. SCC membership
bounds scalar identity; per-location backward searches release their scratch
before the next location. It performs no I/O, execution or callee expansion.


`event_routes::Prepared` borrows one authenticated function stream and its existing
navigation index. It interprets required ABI values, local field coordinates and
selector predicates, and shares the memory-slice owner for callback-store checks.
Only owned admitted observations survive that borrowed preparation.
