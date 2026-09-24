# Blobray verification

`blobray-verification` compares concrete observations through domain values. It
has no filesystem, store, executor, selection or publication authority. Application
supplies observations, the exact case relation and shared operation control.

`selected-projected-timeline-calls-returns-memory/model-9` compares explicitly selected ordered
MMIO read/write, fence and modeled delay channels; selected low/high return words;
exact paired final normal-memory ranges; and opt-in ordered physical call targets/words. Physical range lengths must agree;
addresses may differ only because the caller selected that pair. No pointer, ABI
or layout equivalence is inferred. Unselected evidence remains with the caller.

Known event/return differences yield DIFF even with other unmet obligations. A
code-goal-completed shorter event stream contradicts an observed extra event;
an unfinished shorter prefix cannot establish that length difference. Selected
unknown returns or memory bytes cannot establish equality. A known differing final
byte proves DIFF when both code goals completed; snapshots at unfinished stops are
intermediate evidence and cannot prove different completed final states. Equal
selected data can MATCH only with both code goals and due model obligations met.

`CaseComparison.difference` identifies the selected event index, return word, or
memory pair/byte offset and values. Missing execution/model obligations remain in
adjacent observations. Every compared item consumes shared work; exhaustion is an
error, never a verdict. The application aggregates DIFF before INCOMPLETE and
publishes through the ordinary durable lifecycle.

Memory lookups use the ordered selection/chunk index without cloning snapshots.
Unknown and unavailable bytes are distinct retained facts. Elapsed time and
abstract service internals are outside this implemented relation. Results concern explicit cases and the declared
compiled binding; they do not establish whole-domain equivalence or qualification.
See [selected comparison contracts](../../docs/design/contracts.md#selected-final-memory-and-comparison-relations).

An explicit `reviewed_calls` selection supplies immutable accepted call pairs to
this pure verifier. The shared bounded domain index maps each side's physical
boundary to a selected pair and applies exact/selected/ignored physical words.
Unlisted calls are explicitly exact or excluded, with raw evidence retained.
Pairs can relate distinct targets; they do not infer ABI/layout or pointer
normalization. Store independently checks selected reviews and uses the same
index for knownness admission. No knowledge lookup occurs inside verification.

`events.timeline` selects normal reads/writes, atomics and conditional branches.
Guest memory records and declared call/service effects share typed normal-memory
comparison without duplicate bookkeeping events. Dynamic allocation compares one
zeroed requested span, excluding inaccessible capacity; empty spans add no memory
effect. Bulk initialization is distinct from individual stores. Memory sites/origins remain
provenance; branches compare exact physical control coordinates. Unknown reads
cannot establish equality, and identical final RAM does not erase a timeline
DIFF. All selected channels remain interleaved in one ordered comparison stream.

Reviewed layout projections map exact byte fields/offsets and conditional branch
coordinates while retaining order, unknowns and atomic semantics. Unmapped selected
effects prevent equality. Final fields compare complete selected known bytes; unknown
padding is retained outside the explicit selection. Projected call words compare
exact 32-bit values at reviewed positions. No pointer-value or type conversion is
inferred. The verifier borrows contracts and observations; store owns review lookup.
