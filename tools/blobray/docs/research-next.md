# Research-next ranking and budgets

`project research next` turns the current project's review queues, linked IR,
register facts and reviewed interface observations into copyable, project-bound
research actions. It does not infer capability evidence or mutate reviewed
knowledge. Existing reusable-capability matches and verification surfaces are
reported as context, but never counted as new evidence or as ranking benefit.

Interface observations and capability links come only from the compact
generated file selected by `[interfaces.capability-context].output`. They are
not recomputed from live interface facts or reviewed packs during this command.
The projection is derived, disposable state; its input digest is checked against
the current interface facts, reviewed interface/semantic/capability/template
packs, calling convention and compiled-knowledge identity before use. The
reviewed inputs remain the authority.

Always select the project explicitly when a command is copied outside its
project directory:

```console
cargo blobray project research next \
  --project verification/vendor/projects/esp32s31/vendor-project.toml
```

## Ranking strategies

`--strategy impact` is the default and preserves the established descending
impact-per-effort score. `--strategy quick-wins` orders actions by estimated
cost, then by co-blockers and impact. `--strategy frontier` (also accepted as
`pareto`) removes every action for which another action has at least as much
benefit and no more effort, with one strict improvement. Frontier results are
then ordered by the impact score.

The machine report uses schema 17. It separates the complete backlog from the
bounded recommendation and exposes reviewed labels for referenced functions:

- `inventory.findings` contains every typed finding exactly once;
- `inventory.actions` contains every coalesced next action and refers to
  its findings only through `finding_ids`;
- `inventory.prerequisites` contains every deduplicated destination or anchor
  action without a selection-specific rank;
- `reviewed_functions` maps generated identities to manually reviewed names,
  roles and summaries without replacing generated behavior or completeness;
- `selection.steps` is the only ranked list. Its typed prerequisite/action
  references are bounded by `--limit` and `--budget`.
- `focus` is always explicit. `all` keeps the complete ranking, while
  `hardware-access` makes only typed MMIO/register subjects and explicitly
  reviewed hardware-shared memory accesses eligible. A memory classification
  is bound to an exact source artifact SHA-256 plus function and instruction
  site; unclassified RAM and reviewed software-only state remain inventory.
- `finding_query` is always present. It distinguishes `all`, `open`,
  `condition-satisfied`, `input-not-observed`, `filtered-out`, and
  `not-present`. Correlated register states include typed current observation,
  ownership, scope, model and exact reviewed-assertion evidence. Every state
  has `completion_claim = false` and `historical_finding_claim = false`.

Every linked-IR blocker also carries `blocker_resolution_route`. The route
separates its real owner (`generic-backend`, `analysis-addon`,
`interface-pack`, runtime verification, or an explicitly unsupported class)
from the project files. `destination`, `record_kind`, and `record_action` are
present only when Blobray already consumes that exact record. The
`producer_effect` distinguishes direct closure, delegated child closure,
downstream-only evidence, informational markers, and unsupported causes.
`completion_predicate.root_id` is checked against its authenticated regenerated
producer: linked-IR blockers name the review-scope diagnostic root, while
event-route blockers name the exact route and blocker kind in the current
event-flow report. Editing a project file is never itself a completion claim.

For an exact register lookup, interpret the states in this order:

- `open`: the current observation is relevant to the selected scopes and still
  satisfies the finding predicate;
- `input-not-observed`: the current MMIO facts do not contain the physical
  input, or no configured scope observes it;
- `filtered-out`: configured scopes observe the input, but the selected
  protocol/scope filter excludes all of them;
- `condition-satisfied`: exact retained reviewed evidence makes the current
  producer predicate false. This describes current state, not a historical
  transition and not completion. For a register-model finding this requires
  one retained configured `register-identity = "REGION.NAME"` assertion for
  the exact physical subject; a reusable base-model identity alone is not
  enough;
- `not-present`: no typed attribution supports a stronger conclusion. A
  base-model identity or an unknown arbitrary ID is not resolution proof.

An action is one typed `next_action` plus its typed resolution owner and exact
required model. Findings coalesce only when all three are equal; the same
inspection command remains multiple actions when it exposes work owned by
different components or requiring different models. This keeps action identity
and ranking from hiding distinct causal work. A finding retains its typed `subject`, typed
executable `consumers`, exact evidence sites/channels, causal inspection
functions, impacted functions, context links, required knowledge,
finding-level `actionability`, prerequisite IDs, an exact `requery_action` and
post-review `revalidation_actions`.

`inventory.sha256` hashes the project ID, analyzed scope IDs and canonical
ID-sorted catalogs. It is independent of strategy, limit and budget, so two
selections over the same backlog share one identity. The digest is
invocation-context-bound: action IDs hash a canonical execution key containing
exact argv boundaries, absolute working directory, context level and resolved
project overrides, followed by the typed resolution owner and exact required
model. Use the same invocation directory and project-path spelling for
reproducible `--check` output. Human output renders argv only at the
presentation boundary. The digest identifies inventory content; it never replaces full report
validation or byte comparison.

Reviewed-knowledge consumers are `ready` only through the explicit
`[reviewed-knowledge].default-pack`; the number of configured packs is never a
routing rule. Interface consumers distinguish `needs-destination` from
`needs-anchor`, and become `ready` only for a project-local, non-templated
anchor that can safely accept the observed slot. Findings without a supported
durable consumer remain honestly `inspection-only`. `coverage-blocked` is
reserved for a typed producer cause and is never inferred from diagnostic
text. The report never turns a suggested consumer or a revalidation command
into a completion claim; `completion_claim` is always `false`.

Incomplete typed event routes are a dedicated inspection-only case. Their
subject carries `route_id` and `blocker_kind`, and their executable action is
exactly `inspect flow --event-route ID` in the resolved project context. Scope
membership and `inspection_function_ids` provide navigation, but
`affected_scope_roots`, direct functions, and guaranteed/optimistic/marginal
unlock sets stay empty until the producer emits separate typed impact evidence;
publication scopes are also empty rather than inferred from scope membership.
Consequently event-route findings receive no root, publication, or
function-unlock weight.
Their completion predicate is satisfied only when that exact blocker kind is
absent from the current authenticated report for that exact route; Blobray does
not parse blocker messages to derive identity, ownership, completion or impact.

If the configured interface capability context is missing, malformed, belongs
to another project or has a stale input digest, the report fails closed only
for that context. It contains no interface-observation actions and no
capability links derived from those inputs, sets `capability_diagnostic`, and
the human view prints `PARTIAL PRIORITIZATION`. Register, replacement and other
available findings remain usable. There is deliberately no live evaluation
fallback: rerun `project analyze` to regenerate the context before comparing a
complete cross-domain ranking.

Selection uses explicit prerequisite and action lanes. Each lane retains
its requested ranking; the bounded result interleaves them as
`prerequisite, action` so setup work cannot hide concrete inspection targets.
Ready actions lead inspection-only actions, with blocked actions last for
machine/detail audit. Missing required public symbol families lead the
prerequisite lane as typed coverage-blocked findings. Prerequisites aggregate
downstream function/root sets without summing duplicate action scores.

Each action has a compact
`score_explanation`:

- `benefit_points` is the sum of guaranteed, optimistic, marginal, root,
  and publication weights;
- `effort_points` is the cost penalty plus the co-blocker penalty and one
  smoothing point;
- `estimated_cost_units` is the bounded unit consumed by `--budget`;
- `score` is `floor(100 * benefit_points / effort_points)`.

The detailed `score_breakdown` remains available for audit. Capability and
verification context weights are deliberately zero, including closed
verification surfaces: correlation cannot masquerade as newly unlocked work.
Scores are stable prioritization signals, not elapsed-time estimates.

## Filters, budget and limit

`--focus hardware-access` narrows only selection eligibility. It does not
remove interface, call-result, control-flow, or other findings from the
inventory, and it does not change exact-finding resolution. Generic call and
control-flow blockers are not admitted merely because they transitively reach
hardware work. A prerequisite is eligible only when it directly satisfies an
eligible finding; an unrelated interface anchor therefore cannot displace an
SRAM or MMIO action. The default is `--focus all`.

Reviewed memory classification is ranking evidence only. It does not resolve
the underlying analysis blocker or claim that the object's address, lifetime,
layout, ownership, or hardware behavior has been modeled.

`--scope ID` selects one exact configured review scope. `--protocol NAME`
selects scopes by their mandatory, explicit `protocols` membership in
`vendor-project.toml`; scope IDs and symbols are never interpreted as protocol
names. Canonical names are `wifi`, `bluetooth`, `ble`, `ieee802154`, `coex`,
and `shared`. `bluetooth` denotes BR/EDR or an explicitly shared BTDM scope;
BLE-only evidence uses `ble` and is never promoted to Classic coverage. The
CLI also accepts `bt` for `bluetooth`, plus `802.15.4` and
`802154` for `ieee802154`, and emits the canonical name in JSON. A shared PHY
or coexistence scope can carry several protocol tags. Unknown names and an
empty exact scope/protocol intersection fail with the configured alternatives.

`--limit N` bounds prerequisites and research actions together. `--budget
UNITS` likewise uses one cumulative cost across both. Selection alternates the
ranked prerequisite and action lanes, skips steps that do not fit the
remaining budget, and never reorders either lane internally. `--limit 1`
retains the highest-priority prerequisite; a limit of at least two exposes an
next action whenever both lanes are non-empty and budget permits. A
budget too small for every eligible step produces an empty report with the
minimum required cost in `selection.diagnostic`. Complete inventory lengths,
strategy-eligible counts and ordered typed step references make the bounded
selection auditable without dropping hidden findings.

```console
cargo blobray project research next \
  --focus hardware-access --protocol ble --limit 20 \
  --project verification/vendor/projects/esp32s31/vendor-project.toml

cargo blobray project research next \
  --strategy quick-wins --protocol wifi --budget 20 --limit 8 \
  --project verification/vendor/projects/esp32s31/vendor-project.toml

cargo blobray project research next \
  --strategy frontier --scope ieee802154-coex-client --limit 20 \
  --format json \
  --project verification/vendor/projects/esp32s31/vendor-project.toml

cargo blobray project research next \
  --finding register-0x20103100-32 --format json \
  --project verification/vendor/projects/esp32s31/vendor-project.toml
```

Exact finding lookup derives the complete selected candidate set and its
co-blockers before retaining the requested ID. With no `--finding`, the full
inventory is unchanged. `not-present` means only that the exact ID is absent
from the current analyzed inputs selected by `--scope`/`--protocol`; it is not
proof that a review was correct or that research is complete.

Every returned `next_action` includes the resolved `--project` path, exact
argument vector, absolute working directory and required project context.
Finding-level `revalidation_actions` describe what to rerun after human review;
`requery_action` addresses the exact finding without parsing another command.
These actions do not assert that the work is done.
`--output PATH --check` retains the normal generated-file contract for
reproducible machine plans. File generation and checking stream serialization
against the destination and do not allocate a second full JSON document.

The human summary keeps the inspect target near the left edge of the ranking
table so it remains readable in an ordinary terminal. The top action then
lists up to eight coalesced findings separately, including kind, summary,
knowledge gap, typed consumer resolution, causal inspection functions, linked
evidence sites/channels and required evidence. The compact view states exactly
how many findings remain; `--details` expands them. When one lane has no
budget-fitting step, the default view still discloses complete and eligible
counts for both lanes. JSON and `--output` always retain the complete inventory,
even when no action is selected. Required surface findings remain addressable
through `--finding`, including their current source/profile state and exact
re-query action.

## ESP32-S31 measurement

Measured on 2026-08-25 against the repository's current ESP32-S31 generated
review inputs. All 32 configured scopes produced 2,143 findings, coalesced into
559 distinct next actions and 196 deduplicated prerequisite actions. The
authenticated BR/EDR controller is analyzed; IEEE 802.15.4 is the one required
missing public controller surface.

| Strategy | Eligible prerequisites/actions | Returned prerequisites/actions at limit 20 | Cost units | Leading step |
| --- | ---: | ---: | ---: | --- |
| `impact` | 196 / 559 | 10 / 10 | 68 | acquire required IEEE 802.15.4 surface |
| `quick-wins` | 196 / 559 | 10 / 10 | 68 | acquire required IEEE 802.15.4 surface |
| `frontier` | 2 / 16 | 2 / 3 | 16 | acquire required IEEE 802.15.4 surface |

The measured all-scope baseline report is 8,779,755 bytes as pretty JSON and
5,837,609 bytes in compact form. The compact catalogs account for 5,183,032
bytes of findings, 480,258 bytes of actions and 171,810 bytes of prerequisites.

Reproduce the measurements with `--format json` and inspect
`inventory.sha256`, the three inventory catalog lengths,
`selection.eligible_prerequisites`, `selection.eligible_actions`,
`selection.steps`, `selection.consumed_budget`, prerequisite benefit objects
and action `score_explanation` objects. Results intentionally change when the
project's generated analysis or reviewed facts change.

These measurements predate the generated capability-context projection. The
projection removes repeated full interface-workspace loading and capability
evaluation from the interactive `research next` path; remeasure the current
project rather than using the historical timings as a performance target.
