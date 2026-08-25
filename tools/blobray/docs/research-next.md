# Research-next ranking and budgets

`project research next` turns the current project's review queues, linked IR,
register facts and reviewed interface observations into copyable, project-bound
research actions. It does not infer capability evidence or mutate reviewed
knowledge.

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

The machine report uses schema 4. Each candidate has a compact
`score_explanation`:

- `benefit_points` is the sum of guaranteed, optimistic, marginal, root,
  verification and publication weights;
- `effort_points` is the cost penalty plus the co-blocker penalty and one
  smoothing point;
- `estimated_cost_units` is the bounded unit consumed by `--budget`;
- `score` is `floor(100 * benefit_points / effort_points)`.

The detailed `score_breakdown` remains available for audit. Scores are stable
prioritization signals, not elapsed-time estimates.

## Filters, budget and limit

`--scope ID` selects one exact configured review scope. `--protocol NAME`
selects the leading namespace of configured scope IDs: for example `wifi-rx`
belongs to `wifi`, and `ieee802154-baseband-leaves` belongs to `ieee802154`.
This deliberately uses project configuration rather than guessing a protocol
from symbols or incomplete capability evidence. Unknown names and an empty
scope/protocol intersection fail with the configured alternatives.

`--limit N` bounds the number of actions. `--budget UNITS` bounds their
cumulative `estimated_cost_units`. Selection walks the chosen deterministic
ranking, skips actions that do not fit the remaining budget, and never silently
reorders them. A budget too small for every eligible action produces an empty
report with the minimum required cost in `selection_diagnostic`.

```console
cargo blobray project research next \
  --strategy quick-wins --protocol wifi --budget 20 --limit 8 \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo blobray project research next \
  --strategy frontier --scope ieee802154-coex-client --limit 20 \
  --format json \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Every returned `next_command` includes the resolved `--project` path and remains
directly actionable. `--output PATH --check` retains the normal generated-file
contract for reproducible machine plans.

## ESP32-S31 measurement

Measured on 2026-08-25 against the repository's current ESP32-S31 generated
review inputs. All 29 configured scopes produced 1,765 findings, coalesced into
515 distinct actions:

| Strategy | Eligible actions | Returned at limit 20 | Cost units | Leading action |
| --- | ---: | ---: | ---: | --- |
| `impact` | 515 | 20 | 84 | call boundary, score 1369, benefit/effort 972/71 |
| `quick-wins` | 515 | 20 | 65 | unresolved call, cost 2, no co-blockers |
| `frontier` | 6 | 6 | 22 | same highest-impact call boundary |

The filtered command `--protocol ieee802154 --strategy frontier --budget 10`
analyzed the two configured IEEE 802.15.4 scopes, reduced 38 findings to 26
actions and one non-dominated action, and consumed 4 cost units. Its suggested
command retained the explicit ESP32-S31 project manifest.

Reproduce the measurements with `--format json` and inspect
`total_candidates`, `total_actions`, `strategy_actions`, `consumed_budget` and
the candidate `score_explanation` objects. Results intentionally change when
the project's generated analysis or reviewed facts change.
