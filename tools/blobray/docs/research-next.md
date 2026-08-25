# Research-next ranking and budgets

`project research next` turns the current project's review queues, linked IR,
register facts and reviewed interface observations into copyable, project-bound
research actions. It does not infer capability evidence or mutate reviewed
knowledge. Existing reusable-capability matches and verification surfaces are
reported as context, but never counted as new evidence or as ranking benefit.

Always select the project explicitly when a command is copied outside its
project directory:

```console
cargo blobray project research next \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

## Ranking strategies

`--strategy impact` is the default and preserves the established descending
impact-per-effort score. `--strategy quick-wins` orders actions by estimated
cost, then by co-blockers and impact. `--strategy frontier` (also accepted as
`pareto`) removes every action for which another action has at least as much
benefit and no more effort, with one strict improvement. Frontier results are
then ordered by the impact score.

The machine report uses schema 10. It separates the complete backlog from the
bounded recommendation:

- `inventory.findings` contains every typed finding exactly once;
- `inventory.actions` contains every coalesced inspection action and refers to
  its findings only through `finding_ids`;
- `inventory.prerequisites` contains every deduplicated destination or anchor
  action without a selection-specific rank;
- `selection.steps` is the only ranked list. Its typed prerequisite/action
  references are bounded by `--limit` and `--budget`.
- `finding_query` is always present. It distinguishes `all`, `open`,
  `condition-satisfied`, `input-not-observed`, `filtered-out`, and
  `not-present`. Correlated register states include typed current observation,
  ownership, scope, model and exact reviewed-assertion evidence. Every state
  has `completion_claim = false` and `historical_finding_claim = false`.

For an exact register lookup, interpret the states in this order:

- `open`: the current observation is relevant to the selected scopes and still
  satisfies the finding predicate;
- `input-not-observed`: the current MMIO facts do not contain the physical
  input, or no configured scope observes it;
- `filtered-out`: configured scopes observe the input, but the selected
  protocol/scope filter excludes all of them;
- `condition-satisfied`: exact retained reviewed evidence makes the current
  producer predicate false. This describes current state, not a historical
  transition and not completion;
- `not-present`: no typed attribution supports a stronger conclusion. A
  base-model identity or an unknown arbitrary ID is not resolution proof.

An action is one copyable `inspect_command` and may coalesce several findings
without duplicating them. A finding retains its typed `subject`, typed
executable `consumers`, exact evidence sites/channels, causal inspection
functions, impacted functions, context links, required knowledge,
finding-level `actionability`, prerequisite IDs and post-review revalidation
commands.

`inventory.sha256` hashes the project ID, analyzed scope IDs and canonical
ID-sorted catalogs. It is independent of strategy, limit and budget, so two
selections over the same backlog share one identity. The digest is currently
invocation-path-bound: action IDs hash copyable commands and findings retain
copyable revalidation commands, both of which include the caller's resolved
project path. Use the same project-path spelling for reproducible `--check`
output. The digest identifies inventory content; it never replaces full report
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

Selection uses explicit lanes: prerequisites first, then ready actions, then
inspection-only actions, with blocked actions last for machine/detail audit.
The requested ranking strategy is applied inside each lane. Prerequisites
aggregate downstream function/root sets without summing duplicate action
scores.

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

`--scope ID` selects one exact configured review scope. `--protocol NAME`
selects scopes by their mandatory, explicit `protocols` membership in
`vendor-project.toml`; scope IDs and symbols are never interpreted as protocol
names. Canonical names are `wifi`, `bluetooth`, `ble`, `ieee802154`, `coex`,
and `shared`. The CLI also accepts `bt` for `bluetooth`, plus `802.15.4` and
`802154` for `ieee802154`, and emits the canonical name in JSON. A shared PHY
or coexistence scope can carry several protocol tags. Unknown names and an
empty exact scope/protocol intersection fail with the configured alternatives.

`--limit N` bounds prerequisites and research actions together. `--budget
UNITS` likewise uses one cumulative cost across both. Selection exhausts the
ranked prerequisite lane before ready and inspection lanes, skips steps that do
not fit the remaining budget, and never silently reorders them. A budget too
small for every eligible step produces an empty report with the minimum
required cost in `selection.diagnostic`. Complete inventory lengths,
strategy-eligible counts and ordered typed step references make the bounded
selection auditable without dropping hidden findings.

```console
cargo blobray project research next \
  --strategy quick-wins --protocol wifi --budget 20 --limit 8 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray project research next \
  --strategy frontier --scope ieee802154-coex-client --limit 20 \
  --format json \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray project research next \
  --finding register-0x20103100-32 --format json \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Exact finding lookup derives the complete selected candidate set and its
co-blockers before retaining the requested ID. With no `--finding`, the full
inventory is unchanged. `not-present` means only that the exact ID is absent
from the current analyzed inputs selected by `--scope`/`--protocol`; it is not
proof that a review was correct or that research is complete.

Every returned `inspect_command` includes the resolved `--project` path and
remains directly actionable. Finding-level `revalidation_commands` describe
what to rerun after human review; they do not assert that the work is done.
`--output PATH --check` retains the normal generated-file contract for
reproducible machine plans. File generation and checking stream serialization
against the destination and do not allocate a second full JSON document.

The human summary keeps the inspect target near the left edge of the ranking
table so it remains readable in an ordinary terminal. The top action then
lists up to eight coalesced findings separately, including kind, summary,
knowledge gap, typed consumer resolution, causal inspection functions, linked
evidence sites/channels and required evidence. The compact view states exactly
how many findings remain; `--details` expands them. When prerequisites consume
the entire shared limit, the default view still discloses complete and eligible
action counts and points to `--strategy frontier` or a larger limit. JSON and
`--output` always retain the complete inventory, even when no action is
selected.

## ESP32-S31 measurement

Measured on 2026-08-25 against the repository's current ESP32-S31 generated
review inputs. All 29 configured scopes produced 1,918 findings, coalesced into
485 distinct inspection actions and 161 deduplicated prerequisite actions:

| Strategy | Eligible prerequisites/actions | Returned prerequisites/actions at limit 20 | Cost units | Leading step |
| --- | ---: | ---: | ---: | --- |
| `impact` | 161 / 485 | 20 / 0 | 60 | create interface anchor, downstream benefit 375 |
| `quick-wins` | 161 / 485 | 20 / 0 | 60 | same cost-3 interface-anchor lane |
| `frontier` | 1 / 13 | 1 / 13 | 60 | interface anchor, then the nondominated action frontier |

The measured all-scope baseline report is 6,213,213 bytes as pretty generated JSON and
4,318,279 bytes in compact form. The compact catalogs account for 3,829,455
bytes of findings, 341,630 bytes of actions and 144,935 bytes of prerequisites.
An impact run with `--limit 20` measured 10.68 seconds and 223,100 KiB peak RSS;
a streaming `--check` measured 11.45 seconds and 223,712 KiB. Measurements
include loading and ranking the real project, not only JSON serialization.

Reproduce the measurements with `--format json` and inspect
`inventory.sha256`, the three inventory catalog lengths,
`selection.eligible_prerequisites`, `selection.eligible_actions`,
`selection.steps`, `selection.consumed_budget`, prerequisite benefit objects
and action `score_explanation` objects. Results intentionally change when the
project's generated analysis or reviewed facts change.
