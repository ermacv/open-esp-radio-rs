# Navigate saved research

Find physical references, structural paths, memory definitions and conditional event routes.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Navigation over saved research

`navigate --request query.json [--output observations.json]` reads an explicit
selection and optionally exports the same JSON to a new file atomically. Example:

```json
{
  "scope": {
    "revision": "<source-revision>",
    "publications": ["<publication-id>"],
    "analyses": [],
    "knowledge": null
  },
  "filter": {"kind": "functions", "function": null}
}
```

Copy a returned `function.location` into the optional `function` focus of
`{"kind":"calls","direction":"callers","function":<location>}` or `callees`.
A null focus returns all retained call/tail-transfer observations. Calls retain
record ordinals, saved resolutions and every selected physical candidate. Multiple
analyses of an entry stay distinct. Unresolved calls remain visible even in a
callers query, with `focus_match: false`; they are not confirmed callers. Finite
alternatives remain ambiguous when only some candidates are selected.

An object filter is `{"kind":"object","occurrence":<KnowledgeOccurrence>,
"selector":<DataSelector>,"access":"writers"}`. Use `readers` or null for both.
Selectors use the same physical section/symbol/image ranges as data reading.
NOBITS ranges are searchable with `file_range: null`; no bytes are fabricated.
Results include partial overlaps and finite-address alternatives. Composed effects
retain their origin analysis. Unqualified callee addresses have `foreign-occurrence`
instead of inheriting the caller's object; explicitly scoped addresses can match.

A context filter is `{"kind":"context","assertion":"<accepted-function-claim>",
"field":{"argument":0,"name":"state"},"access":null,"arguments":[]}`.
Select its knowledge revision explicitly. A null field selects all declared fields.
Known signatures determine physical argument placement; an unknown signature needs
an explicit `arguments` mapping, for example `[{"argument":1,"word":2}]`.
`argument` is a logical ordinal, `word` an incoming ABI word. A contradictory mapping
fails. Reading a pointer from its incoming stack slot is distinct from accessing
its context fields. Reviewed field access roles do not replace observed access kinds.

The supported nonvariadic RV32 scalar mapping counts 64-bit arguments as two words.
A named argument may split a7/stack; a wholly stacked 64-bit scalar aligns to eight
bytes. This follows the [RISC-V integer calling convention](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention);
it performs no execution and verifies no runtime precondition.

The selection allows at most 64 publications and 4096 explicit analysis IDs;
publication members and operation indexes remain memory/work limited. Duplicate
analysis IDs are read once. `functions` decodes manifests without loading fact records (`analyses_read: 0`),
while still verifying their retained stream digests. Call/access queries load one
function's facts at a time. Unavailable entries,
partial analyses, unresolved and ambiguous observations remain explicit. There is
no general `complete`, PASS or proof of absence: neither unselected functions nor
unclassified executable intervals are silently analyzed. Source removal, project
move and backup/restore preserve this read/export scenario.


## Structural flow and effect inventory

`flow --request query.json [--output observations.json]` uses the same explicit
scope and resource options as `navigate`. For a saved root and target:

```json
{
  "scope": {"revision":"<revision>", "publications":["<publication>"],
            "analyses":[], "knowledge":null},
  "root":"<root-analysis>",
  "goal":{"kind":"function", "analysis":"<target-analysis>"},
  "max_depth":8
}
```

Both IDs must belong to the selection. `function` rows provide one shortest
structural predecessor path using exact caller analysis, call record and callee
analysis. Call rows retain the original navigation observation. Unknown/ambiguous
calls do not create traversable edges. Partial analyses, missing selected callees
and the depth boundary remain frontier rows. `target_reached` describes this graph;
false is not a firmware-wide absence proof, and true asserts no path feasibility.
The depth range is 0..4096; zero includes the root and exposes outgoing frontiers.

For effects, use `{"kind":"effects","profile":"memory","address":null}`.
Profiles are `calls`, `memory` and `all`. An optional numeric address filters memory
access spans, including partial overlap; calls-only rejects an address filter.
`address_match: null` means no filter or an unknown/partly matching address, with
the request distinguishing these cases. Unknown accesses remain in the output.
`effect` rows preserve their analysis ID, record ordinal and original fact, including
callee origin. Local and composed instances are separate evidence rows. Counts do
not represent deduplicated hardware events or a runtime sequence.

Graph construction decodes each selected function once. Memory-effect delivery
makes another pass per reached function, releasing each record buffer before the
next. `facts_passes` counts those function decodes. There is no implicit linking,
research or current-head selection. The result and optional atomic JSON export
can be reopened from a moved/restored project without source files.

To review an exact path, use the ordinary `knowledge validate/apply/accept/show/export`
lifecycle with this claim and an `analysis` evidence reference to its root:

```json
{"kind":"path", "path":{
  "hops":[{"caller":"<analysis>", "record":42, "callee":"<analysis>"}],
  "purpose":"Selected captured call path",
  "applicability":"Conditional structural navigation for these saved analyses"
}}
```

The proposal occurrence must match the root. Every hop is rechecked through the
same selected edge resolver during proposal and review. Missing records, different
targets, ambiguous interpretations, disconnected hops and cycles fail before
knowledge publication. The path holds at most 64 hops and retains participant
manifest/fact roots. Acceptance records a reviewed structural relationship;
preconditions, asynchronous delivery and executable equivalence remain unproved.


## Memory definitions at a publication point

`memory-slice --request query.json [--output observations.json]` reads one saved
analysis. Obtain exact record ordinals through `analysis --id <analysis>` first:

```json
{
  "analysis":"<analysis>", "anchor":42, "abi":"riscv-integer",
  "locations":[{"kind":"stack", "offset":-4, "width":4}]
}
```

The anchor is a local call instruction, transfer or write record, and the slice
ends immediately before it. A loop can include a prior iteration of that same
write. `locations: []` discovers preceding local write spans. Up to 256 explicit
selections accept `access` with a saved local memory record ordinal, `stack` with
an entry-SP offset, `address` with an RV32 address, or `argument` with an incoming
ABI `word`, byte `offset` and `width`. Widths are 1, 2, 4 or 8. An explicit argument
requires `abi: "riscv-integer"`; `abi: null` never guesses an argument mapping.
A selected read may expose untouched incoming state. Unknown addresses remain
`unknown-location` rows during discovery; explicit unresolved access selectors fail.

`location` rows report `incoming: possible|overwritten|unknown` and named issues.
`definition` rows retain exact source record, raw fact, instruction offset and a
structural suffix witness to the anchor. `must` requires one last-write site on
all saved paths, closed coverage and no unresolved interference. `alternative`
means multiple possible last definitions or a surviving incoming value. `candidate`
keeps evidence affected by partial coverage, partial overlap, dynamic identity,
unknown writes or calls. `barrier` rows identify that evidence explicitly. A
covering write may overwrite incoming bits without providing an interpreted narrow
value; the result performs no byte assembly or atomic-operation execution.

Fields of one immutable loaded pointer can share a saved expression identity.
Loads repeated within cycles remain dynamic. Different roots may alias, and a
stack argument's initial pointee is not equated to an arbitrary later load from
its mutable cell. An `access` selector follows the exact saved loaded value.
Composed callee effects never silently replace local call clobbers. The
[canonical contract](../../../docs/design/contracts.md#memory-definitions-at-publication)
defines these bounds; none of the classes claims runtime feasibility or hardware
state. API/CLI results and atomic JSON export reopen without original sources.


## Conditional event routes

`event-route --request route.json [--output observations.json]` inspects saved
participants without linking or running analysis. For selector delivery:

```json
{
  "revision":"<revision>",
  "route":{
    "mechanism":"project.events", "execution_context":"interrupt to task",
    "applicability":"These captured participants and the reviewed service semantics",
    "upstream":[], "terminal":[],
    "route":{"kind":"selector-delivery", "route":{
      "dispatch":{"call":{"caller":"<producer>","record":42,"callee":"<send>"},"word":1},
      "selector":25,
      "delivery":{"call":{"caller":"<consumer>","record":80,"callee":"<receive>"},"word":0},
      "selector_load":{"analysis":"<consumer>","record":90},
      "selector_offset":0, "selector_width":4,
      "case":{"condition":{"analysis":"<consumer>","record":95},"taken":true,
        "handler":{"caller":"<consumer>","record":100,"callee":"<handler>"}}
    }}
  }
}
```

Record ordinals come from `analysis --id`; call ordinals are the exact sites used
by `navigate`. Calls name the selected callee analysis, preserving alternate
interpretations. Words 0..7 mean a0..a7 under the explicit RV32 integer route
profile. Higher words are accepted as selectors but currently return unresolved
saved-call evidence. This never substitutes a register for a stack argument.

The [native route types](../../../crates/domain/src/event_route.rs) define three profiles:

| Kind | Required physical relationships |
| --- | --- |
| `selector-delivery` | Dispatch selector, delivery output pointer, exact field load/width/offset, matching branch edge and structural handler suffix |
| `static-callback` | 1..16 dispatch object/queue pairs, registration object/callback pointer, matching receive queue, receive return or output load passed to invoke, exact callback entry |
| `broker-subscription` | Publish object/selector/payload word, attach object/domain selector, subscribed domain, subscriber callback-field store reaching subscription, callback selector case and handler |

Optional upstream and terminal paths have at most 16 contiguous acyclic hops each.
Selector fields use 1/2/4 bytes; callback stores use four. A whole-word mask preserves
an RV32 pointer, while a partial mask remains unresolved. Different stack frames
and captured object namespaces are not merged merely because offsets match.

Output contains original evidence rows, named `established|unresolved|mismatch`
checks and separate conditions. `analyses_read` counts decoded participant streams;
the operation releases each full buffer before reading the next. A selected branch
has a structural suffix consistent with the supplied selector; it does not prove
exclusive dispatch or runtime feasibility. A partial CFG remains unresolved.

To review, place the declaration in a knowledge claim
`{"kind":"event-route","route":<the route object>}` with the dispatch root's
exact occurrence and an analysis evidence reference. Use the ordinary
`knowledge validate/apply/accept/show/export` lifecycle. Both proposal and acceptance
recheck physical bindings; unresolved/mismatched declarations fail before publication.
Accepted route identities and query exports survive source removal and current-format
backup/restore.

Service roles are reviewed interpretations of selected callees. The query does not
infer send/receive/invoke semantics from service bodies: `mechanism-semantics`
remains an obligation. Object lifetime, delivery order, execution context and runtime
guards remain conditions; registration routes also retain registration-before-dispatch.
There is no overall complete/PASS or proof that the event actually arrived. See the
[canonical route contract](../../../docs/design/contracts.md#reviewed-event-routes).
