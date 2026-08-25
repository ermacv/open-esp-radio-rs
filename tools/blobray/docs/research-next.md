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

The machine report uses schema 7. It separates three levels:

- a `prerequisite` is a deduplicated destination or anchor action that must be
  completed before its blocked findings can land;

- an `action` is one copyable `inspect_command` and may coalesce several
  findings without losing any finding fields; its `actionability` groups keep
  counts and exact finding IDs for mixed actions;
- a `finding` retains its typed `subject`, typed executable `consumers`, exact
  evidence sites/channels, causal inspection functions, impacted functions,
  context links, required knowledge, finding-level `actionability`, prerequisite
  IDs and post-review revalidation commands.

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
required cost in `selection_diagnostic`. Separate total/strategy/returned
prerequisite counts make the bounded selection auditable.

```console
cargo blobray project research next \
  --strategy quick-wins --protocol wifi --budget 20 --limit 8 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray project research next \
  --strategy frontier --scope ieee802154-coex-client --limit 20 \
  --format json \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Every returned `inspect_command` includes the resolved `--project` path and
remains directly actionable. Finding-level `revalidation_commands` describe
what to rerun after human review; they do not assert that the work is done.
`--output PATH --check` retains the normal generated-file contract for
reproducible machine plans.

The human summary keeps the inspect target near the left edge of the ranking
table so it remains readable in an ordinary terminal. The top action then
lists up to eight coalesced findings separately, including kind, summary,
knowledge gap, typed consumer resolution, causal inspection functions, linked
evidence sites/channels and required evidence. The compact view states exactly
how many findings remain; `--details` expands them. JSON output is unchanged
and always retains every finding and evidence field.

## ESP32-S31 measurement

Measured on 2026-08-25 against the repository's current ESP32-S31 generated
review inputs. All 29 configured scopes produced 1,918 findings, coalesced into
476 distinct inspection actions and 161 deduplicated prerequisite actions:

| Strategy | Eligible prerequisites/actions | Returned prerequisites/actions at limit 20 | Cost units | Leading step |
| --- | ---: | ---: | ---: | --- |
| `impact` | 161 / 476 | 20 / 0 | 60 | create interface anchor, downstream benefit 375 |
| `quick-wins` | 161 / 476 | 20 / 0 | 60 | same cost-3 interface-anchor lane |
| `frontier` | 1 / 12 | 1 / 12 | 57 | interface anchor, then ready register-model action |

With `--limit 200`, the explicit protocol filters measured 136 prerequisites
and 327 actions for Wi-Fi, 37 and 206 for BLE, and 16 and 104 for IEEE 802.15.4.
Shared/coexistence scopes intentionally participate in every protocol listed
by their manifest `protocols` membership, so these sets overlap rather than
partitioning the all-scope total.

Reproduce the measurements with `--format json` and inspect
`total_findings`, `total_prerequisites`, `total_actions`, the corresponding
`strategy_*` and `returned_*` counts, `consumed_budget`, prerequisite benefit
objects and action `score_explanation` objects. Results intentionally change
when the project's generated analysis or reviewed facts change.
