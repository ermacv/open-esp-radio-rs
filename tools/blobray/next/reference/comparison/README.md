# Comparison relations

Choose physical or reviewed call, memory, branch, layout and effect relations.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Physical call capture and comparison

An invocation may set `observe_calls` to:

```json
{"include_tail":false,"argument_words":8,"overrides":[{"target":8192,"words":10}]}
```

`null` disables capture. Counts are explicit RV32 words (maximum 256); at most
128 unique target overrides are allowed. Eight words select a0–a7; later words
read the current private stack. Unknown and unavailable words stay explicit and
do not change code execution. Zero words selects targets only. `include_tail`
also captures x0 noncanonical jump candidates, including possible intrafunction
jumps; canonical returns and the root sentinel are excluded.

Set `relation.calls` to `true` on a comparison case and use the identical capture
profile on both sides. Physical targets and selected words compare in order with
selected MMIO/fence/delay events. Sites, SP and transfer/target kinds remain
provenance. `call-target` and `call-argument` differences identify the selected
call/effect index and differing values; changed ordering yields `event`.
Unknown selected words prevent MATCH. Captures stay retained when `calls` is
false. This profile does not infer semantic correspondence across different
addresses or interpret pointer arguments.

Capture happens before `observe-call` stops and before captured/model/FIFO
dispatch. Target classification identifies the selected boundary, not execution
of its body. Each `call-transfer` plus its `transfer-argument` rows consumes
`max_events`, with whole-group admission and shared run limits. See
[physical call contracts](../../../docs/design/contracts.md#physical-call-observations)
for lifetime and claim scope. Execute/compare/query/replay all use this same
retained profile.

## Reviewed call correspondence

Use `knowledge propose-call-pair --request pair.json` (or
`Application::start_propose_call_pair`) to propose an explicit correspondence.
The request has `subject`, `correspondence`, `expected_base`, `actor` and `reason`.
Correspondence contains `vendor`/`replacement` endpoints, `arguments`,
`applicability` and `reason`. Each endpoint contains `occurrence` and `boundary`:

- `{"kind":"code","address":4096}` requires the occurrence's exact executable
  symbol, with no name lookup or inferred extent.
- `{"kind":"model","binding":{...},"definition":"..."}` identifies a complete
  explicit call binding and canonical call declaration digest.
- `{"kind":"service","binding":{...},"definition":"...","binding_index":0}`
  identifies a FIFO definition and binding-array ordinal. Modeled/service context
  occurrences have `symbol: null`; they do not claim the source contains the model.

The generic knowledge proposal accepts the same `call-pair` claim. Both paths
validate and preserve both source occurrences; review uses the ordinary accept or
reject action. Select the accepted immutable reference in a comparison relation:

```json
{"calls":false,"reviewed_calls":{"pairs":[{"knowledge":"<revision>","assertion":"<id>"}],"unlisted":"exclude"},"returns":{"low":false,"high":false},"events":{"timeline":{"reads":false,"writes":false,"atomics":false,"branches":false},"mmio_read":true,"mmio_write":true,"fence":true,"delay":true},"memory":[]}
```

Reviewed `arguments` is `{"kind":"exact","words":8}`,
`{"kind":"selected","words":[0,7]}` or `{"kind":"ignore"}`. Exact requires
matching capture counts; selected indices must exist on both sides. No physical
word remapping or pointer/layout interpretation is inferred. Unlisted calls use
explicit `exact` or `exclude`; exact requires identical capture profiles on both
sides. All calls and words remain retained. Listed operations need not all occur,
but observed selected calls compare in order with selected MMIO/fence/delay.
Unknown selected words prevent MATCH.

`manifest.call_pairs` retains the selected accepted contracts and review IDs.
Changed source, model definition, binding or lifetime cannot reuse an inapplicable
pair. At most 128 distinct reviews are selected per execution, within the existing
64 KiB request/manifest bounds. A reviewed relation is a scoped comparison
assumption, not hardware qualification. See
[reviewed call contracts](../../../docs/design/contracts.md#reviewed-call-correspondence).

## Internal physical timeline

Each invocation supplies `observe_timeline`:

```json
{"reads":true,"writes":true,"atomics":true,"branches":true}
```

Use all false to disable guest timeline capture. In the comparison relation,
`events.timeline` independently selects the same four channels; a selected channel
must be captured by both invocations. `max_events` and common run budgets apply.
Capture and comparison use execute/compare/query/replay through the shared API.

Raw `memory` events retain instruction site and typed read/write/LR/SC/RMW data.
Read values distinguish known, unknown and unavailable. Writes retain their exact
width/value; atomics include ordering and old/new or SC outcome. `branch` records
include site, target, fallthrough and taken decision, including compressed branches.
Selected branch sites compare physically. Memory sites/origins remain provenance;
address, width and transaction values/order enter equality. No cross-layout or
pointer correspondence is inferred.

Existing call/service normal-memory effects enter these channels once from their
original records, including dynamic allocation as one `initialize-zeroed` span
for its nonempty accessible prefix. Unused capacity remains provenance; a zero-byte
allocation adds no memory transaction. Bulk initialization is distinct from
individual stores. Fetch, setup initialization, argument inspection and final
snapshots do not invent guest transactions. All selected memory/control observations
remain ordered relative to selected call/MMIO/fence/delay events. Different
intermediate states or read order can DIFF even when final memory matches.
Unknown/inaccessible reads or incomplete execution cannot MATCH. Raw excluded
observations remain available after source removal and project restore. See
[internal timeline contracts](../../../docs/design/contracts.md#internal-timeline).


### Reviewed layout and ABI comparison

`knowledge propose-projection --request projection.json` creates a proposal with
`subject`, `projection`, `expected_base`, `actor` and `reason`. Use normal knowledge
review to accept it, then set the execution case's `relation.projection` to
`{"knowledge":"<accepted-snapshot>","assertion":"<assertion>"}`. The manifest retains
that exact resolved policy; updating knowledge does not update old comparisons.

A projection specifies `vendor` and `replacement` endpoints, `fields`, `branches`,
`applicability` and `reason`. Each endpoint has an `entry` using the exact captured
code endpoint format of call pairs, plus `domains` such as
`[{"address":12288,"length":16}]`. A field can map different offsets:

```json
{"name":"counter","vendor":{"domain":0,"offset":0},
 "replacement":{"domain":0,"offset":8},"width":4,"count":1,
 "final_state":true,"timeline":true}
```

Both domains must contain their aligned field/array. Element widths 1/2/4/8 and a
nonzero count define the exact byte span, with a total 1 MiB projection limit;
aliases, missing capture, duplicate names and unsupported widths fail explicitly.
Capture final ranges in `observe_memory` and enable the selected timeline channels.
All declared final fields participate, including unchanged/unknown bytes. Padding
outside those fields is retained without being included in equality. Branch pairs
contain `vendor`/`replacement` objects with exact `site`, `target`, `fallthrough`;
proposal/review checks the captured instructions. Corresponding taken decisions
must agree. Raw order, memory transaction kind/width/value and atomic outcomes remain
significant. Unmapped selected observations prevent MATCH.

For different call ABI positions, an accepted call correspondence can use
`"arguments":{"kind":"projected","words":[{"vendor":0,"replacement":1},
{"vendor":7,"replacement":8}]}`. Capture every selected word, including stack words.
This compares exact 32-bit values at different positions; it does not normalize
pointer values, truncate integers or infer types. With different capture widths,
unlisted-call scope must explicitly exclude unpaired calls. Raw unselected words
remain available. `CallArgument.word` is the selected pair ordinal for this policy.

All projections are finite reviewed assumptions bound to captured sources and entries.
A successful comparison establishes selected observations under those assumptions;
it does not establish general equivalence. See [projection contracts](../../../docs/design/contracts.md#reviewed-layout-and-abi-projections).

### Reviewed effect comparison

`knowledge propose-effect-contract --request effects.json` submits
`{subject, contract, expected_base, actor, reason}` through the shared application
scenario. Normal knowledge review accepts or rejects it. Set a comparison case's
`relation.effects` to `{knowledge, assertion}` for the exact accepted review;
all four MMIO read/write, fence and delay event channels must be enabled.

A contract contains exact `vendor`/`replacement` code endpoints (the same
occurrence/boundary shape as call correspondence), `rules`, `claim_ceiling`,
`applicability` and `reason`. Each rule contains `name`, per-side patterns,
`disposition`, `min_occurrences`, `max_occurrences` and `reason`. A pattern combines
`selector` with `value: {kind: "any"}` or `{kind: "exact", value: N}`.
Selectors are `mmio-read`/`mmio-write` with physical address and width,
`delay` with `micros` (a value or explicit `null` for all delays), or `fence` with
predecessor/successor masks. Per-side selectors cannot overlap.

Required rules compare identical observations. Omitted rules permit an ordered
subsequence of exact vendor effects; replaced rules require both explicit patterns
in corresponding order. Added rules validate replacement-only effects. Forbidden
rules reject observed occurrences. Missing exercise and unclassified effects are
INCOMPLETE; known differences are DIFF. A zero minimum means required when observed;
an omitted rule always allows zero replacement occurrences. Rules reset per case.

`claim_ceiling: "selected-effect-equality"` permits only required/forbidden rules.
Omitted/replaced/added rules require `"reviewed-effect-refinement"`. Each saved
`CaseComparison` exposes this ceiling as `effect_claim` and its first unclassified
or unexercised obligation as `effect_gap`. A policy violation reports exact side,
raw event ordinal and rule ordinal in `difference`. MATCH with a refinement is
conditional on that reviewed policy; it is not physical equality or general
firmware equivalence. Incomplete execution cannot MATCH.

The manifest retains the resolved accepted contracts alongside all raw evidence,
including omitted and added effects. Comparison composes with reviewed call-word
and layout projections, selected internal timelines, return words and final RAM.
API/CLI query, backup/restore and replay use the same immutable selections. See
[effect contracts](../../../docs/design/contracts.md#reviewed-effect-contracts) for the
precise obligations, lifetime and limits.
