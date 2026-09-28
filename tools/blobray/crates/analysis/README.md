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
See [value semantics and limits](../../next/reference/analysis/README.md#values-and-memory-effects).

An optional borrowed `ImageMemory` port supplies immutable load bytes. Static
ELF permissions qualify these constants; writable/unmapped memory stays unknown.
Image PC-relative values use virtual addresses, and saved transfer records expose
resolved and unresolved calls/outgoing jumps without expanding the local CFG.
No second analyzer or machine executor is selected for linked code.

The same value solver can emit a flat expression DAG, entry-register values,
symbolic loads, conditions and returns. Record and payload reservations live with the DAG;
loops use the bounded monotone state queue. Explicit ABI context controls only
integer call preservation. ELF ABI flags never select that assumption.
An operation-owned, capacity-admitted hash index interns full `(site, expression)`
keys with stable record IDs and provenance. Hash collisions use full equality;
lookup, replacement-index construction and payload ownership share the analysis
budget. Failed admission or cancellation leaves retained IDs usable.

The `audit` module scans all supplied executable ranges linearly for direct and
locally resolved transfers. It shares the ISA port and integer folding with value
analysis; it does not obtain filesystem access or claim dynamic-target completeness.

`closure::code_closure` walks the code statically reachable from root entries
by recursive descent: direct branches, jumps and calls, `auipc`/`lui` plus
`jalr` pairs with a known target, and the caller-supplied observed targets of
executed indirect transfers. A jump to another function's start is a tail
transfer; declared boundaries stop the walk. Each function reports its blocks,
branch directions, callees, modeled boundaries and unresolved or
observation-followed transfers, which application's code coverage compares
with executed instructions. Decoding shares the working capacity and run
control; the function count is bounded.

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

Saved navigation indexes one flat function record stream and interprets calls and
physical access paths without storage, linking or binding authority. Cycles are
reported as structural edges; unknown addresses remain explicit.

The flow module computes bounded iterative reachability over caller-selected
unambiguous arcs. It returns predecessor indexes and depths; storage and reviewed
path authority stay with application/knowledge.

`trace` consumes supplied original local IR and physical call links. Borrowed
per-function indexes are reused across invocations; admitted expression memoization,
path visitation and call frames are iterative. Canonical symbolic equality can prove
a selected observable relation; unsupported paths and undecided inequality remain
incomplete unless an observed prefix already proves a difference. A length difference
is proven only when the shorter side is exact. It neither schedules analysis nor models RAM/peripheral state. Typed
fences come from the semantic producer, not parsed disassembly text.

Trace policy 3 distinguishes pre-transfer `CallInputs` from callee entry, applying
the saved typed `Value` link effect first. Physical return addresses become exact
values; section-relative links stay unresolved without an address mapping.
