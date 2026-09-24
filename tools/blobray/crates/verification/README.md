# Blobray verification

`blobray-verification` compares concrete observations through domain values. It
has no filesystem, store, executor, selection or publication authority. Application
supplies observations, the exact case relation and shared operation control.

`selected-events-returns-memory/model-5` compares explicitly selected ordered
MMIO read/write, fence and modeled delay channels; selected low/high return words;
and exact paired final normal-memory ranges. Physical range lengths must agree;
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
Unknown and unavailable bytes are distinct retained facts. Call boundaries,
ordinary access/branch timelines, elapsed time and abstract service internals are
outside this implemented relation. Results concern explicit cases and the declared
compiled binding; they do not establish whole-domain equivalence or qualification.
See [selected comparison contracts](../../docs/design/contracts.md#selected-final-memory-and-comparison-relations).
