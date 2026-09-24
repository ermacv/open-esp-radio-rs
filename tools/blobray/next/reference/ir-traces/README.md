# Semantic IR and static traces

Package saved facts and extract or compare selected static observable paths.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Saved semantic IR profiles

`ir build` packages an explicitly selected saved research scope. `FunctionRecord`
and `FunctionManifest` remain the semantic representation. Building profiles does
not reread live binaries or schedule analysis; prepare an image and analyze/research
it first when linked addresses and callees are required.

A request contains `scope` (frozen `revision`, `publications`, `analyses`, optional
`knowledge`) and 1–32 uniquely named `profiles`. Each profile supplies `name`,
`include_reachable`, and `roots`: `{"kind":"all"}`, `{"kind":"name-prefix",
"prefix":[112,104,121,95]}` (raw name bytes), or `{"kind":"analyses",
"analyses":["<analysis-id>"]}`. Prefix names come from selected publication
membership; a named analysis without that metadata fails prefix selection. Explicit
analysis IDs and all-roots work without names. Symbol-less ranges have no inferred
name. Empty root matches and duplicate profile names fail.

```sh
blobray ir --project research --limit-mode watchdog build --request ir-build.json
blobray ir --project research --limit-mode watchdog show <semantic-ir-id>
blobray ir --project research --limit-mode watchdog show <semantic-ir-id> --output ir.json
```

`Application::start_build_ir` owns one durable run and publishes `run.semantic_ir`.
`ReadQuery::SemanticIr` / `QuerySink::semantic_ir` provide the same result as CLI
show/export. JSON export is an atomic single-file query export. The saved index
references immutable original streams; read/export expands every original fact
with its analysis ID and record ordinal. It includes original coverage, physical
links and ambiguity, profile memberships/roots, and the explicitly selected frozen
knowledge entries for the source revision, retaining their review state.

Only an unambiguous selected physical target extends call closure. A profile with
`include_reachable:false` keeps its root selection. Dependencies of composed
expressions/effects and selected knowledge analysis evidence are retained transitively
as `provenance_only` functions, without adding them to a profile. Cycles use bounded
iterative worklists. No unresolved call is replaced by another engine or a guess.

Manifest schema 1 / policy 1 separately counts roots, selected functions, partial
functions, unresolved links and unavailable scope entries. Packaging grants no
aggregate coverage, execution verdict, hardware claim or proof that every executable
byte is classified. Composed may-effects retain that meaning. IR exports contain
semantic facts, not captured ELF payloads or every evidence document; a project
backup remains the preservation unit. Source-free reading and backup/restore use
the [current native formats](../interfaces-formats/README.md#current-formats), without converters for previous formats.

## Static observable traces

`trace --project research --request trace.json [--output trace-export.json]`
extracts a static trace, or compares two when `right` is supplied. It uses
`ReadQuery::Trace` / `QuerySink::trace` under the same supervisor and atomic JSON
export contract. Reads never schedule binary analysis or machine execution.

The request has `left`, optional `right`, and `observation`. Each target selects
`ir`, `profile`, exact `entry` analysis, `abi: "riscv-integer"`, and `registers`
(`[{"register":10,"value":0}]` supplies a u32 entry input). Unspecified nonzero
registers are symbolic inputs; x0 cannot be overridden. Observation contains sorted,
disjoint physical `ranges` (`start`, `length`) and a `fences` boolean. At least one
range or fences must be selected. Range/address overflow and duplicate register
inputs fail. A profile member is required; provenance-only functions do not become
implicit entry points or callees.

Policy 3 compares observed prefixes before considering path completeness. A proven
event difference yields DIFF even when a path is incomplete; blockers and exactness
flags remain in the result. An extra event proves DIFF only when the shorter side
is exact. Undecidable symbolic inequality does not hide a later proven difference.

It traces original local function streams. It follows a path only when its
branch predicates are decidable from saved values and supplied inputs. Resolved
calls and tail transfers use the saved physical link index, per-invocation argument
substitution and return values. Saved `CallInputs` describe registers before
the transfer; trace applies its typed `Value` link write before entering the
callee. x1/x5 receive the saved return address, x0 tail transfers preserve links.
Section-relative return addresses stay unresolved in the physical trace profile;
using one in a selected value or address blocks exactness. Missing effects block
tracing and contradictory effects fail integrity validation. A structural unknown
indirect edge can be closed by its unique saved physical
callee in the selected IR profile. This does not repair decoding/reference gaps,
conflicting boundaries, missing flow or unsupported semantics; those still block
exactness. Original function coverage is not upgraded.

Function/call loops use admitted iterative worklists
and leave the extracted path incomplete. Every event carries its original analysis/record/site and
invocation; invocations identify parent call sites. Fences are typed saved facts
in function schema 7 / policy 8 (`values-6`); their display text is never reparsed.

This is a conditional static relation: `abi` explicitly assumes ordinary integer
ABI call/return behavior, including the saved x1/x5 return patterns. It does not
prove that arbitrary code obeys that ABI or terminates on hardware. It also retains
the selected analysis's immutable-image-load assumptions. It is not a peripheral
model or a concrete machine run. Final RAM, return registers, timing and call traces
are excluded from the comparison; invocation/return rows are evidence only.

Selected physical reads/writes and fences remain ordered. Unknown addresses,
accesses crossing a selected range boundary, unknown branch/value facts, unsupported
effects/fences, unresolved/out-of-profile calls and loops retain an explicit blocker
and cannot yield MATCH. There is no RAM state model: an unobserved mutable load
cannot silently supply a value used by the trace. Supplied SP can resolve saved
entry-stack addresses; lost saved values after calls remain unknown. A successful
query may therefore carry an incomplete trace. Query exit status describes delivery;
clients must inspect `left/right.exact` and `verdict`.

Composed research facts remain may-effects. A target whose recipe requested
composition returns `composed-interpretation`; select its original local publication
in an IR build to trace the saved call graph. No implicit reinterpretation or second
analysis engine runs. Exactness concerns the chosen path and observation scope,
not whole-image semantic coverage.

MATCH requires two exact paths and equal ordered observable expressions. A known
address/width/order/fence/constant-value difference yields DIFF. Nonidentical symbolic
value expressions whose inequality is not established yield INCOMPLETE; different
expression IDs alone are not proof of different behavior. Canonical expressions
retain entry-register and observed-read identities. Traces/exports are reproducible
after source removal and project restore; preserving the whole project retains their
IR and original facts. This comparison grants no hardware qualification.
