# Manage issues and dependencies

An issue names one result, the owner that accepts it and the evidence needed
to close it. Keep the current scope and accepted decisions in the body. Keep
experiments and superseded proposals in comments, with a link from the current
summary when they explain a decision. Task tracking stays in GitHub; owner
documentation describes the implemented contract.

## Classify the result

At creation, every issue needs exactly one `kind:*` and at least one `area:*`.
Every executable issue also needs exactly one `priority:*`. Only
`kind:tracking` may omit priority because its children have different urgency;
it still cannot have conflicting priorities. Triage reviews the supplied
classification. Add another area only when it helps an actual consumer find
the task. Add `target:*` only for chip-specific scope; record revision, board,
profile and features in the body. A native Issue Type does not replace a kind
label: a Task can deliver a feature, refactor, decision or validation.

The [label catalog](../.github/labels.json) defines managed names, descriptions
and colors. The [issue-label workflow](../.github/workflows/issue-labels.yml)
creates missing definitions and repairs description/color drift. Change the
catalog through a PR; manual description/color edits are reverted. Renaming a
managed label also requires migrating its issues in the same task. Unmanaged
labels are preserved. Unknown names in a managed namespace and
conflicting kinds or priorities violate the invariant.

| Kind | Result accepted at closure |
| --- | --- |
| `kind:bug` | Expected behavior is restored; the cause or remaining uncertainty and the regression check are stated |
| `kind:research` | A scoped question is answered with applicable evidence, including negative, partial or inconclusive outcomes |
| `kind:decision` | The owner selects a contract or policy and records its rationale and consequences for callers |
| `kind:feature` | A named API or scenario works in the stated scope |
| `kind:refactor` | A named ownership or structural invariant holds, consumers are migrated and behavior is checked |
| `kind:validation` | A method and predeclared acceptance criteria produce evidence for the named subject; qualification evaluates readiness |
| `kind:docs` | The canonical owner description is accurate and its checks pass |
| `kind:tracking` | Required child results and integration criteria are met or explicitly revised by the owner |

Areas identify responsibility: Wi-Fi, Bluetooth, IEEE 802.15.4, shared radio,
register publication, platform, host tooling, HIL and qualification. In the
body name the defining paths and their existing
[`package.metadata.open-radio.layer`](architecture.md#package-classification).
The repository model's [Layer](../tools/repo/src/classification.rs) is the
canonical vocabulary; do not create a separate issue-layer taxonomy. For work
across packages, select the primary accepting package's layer and name the
other owners in the body. For non-package data or documentation, identify the
accepting code owner and its layer. A register investigation or HIL experiment
is a work stage, not a new architecture layer.

## Creation and automatic control

Agents and maintainers using a plugin, API or CLI supply the full validated
label set in the first creation request, then check the returned issue.
The [shared validator](../.github/scripts/issue_labels.py) refuses an incomplete
or conflicting set before its creation helper makes any API request:

```sh
python3 .github/scripts/issue_labels.py validate \
  --label kind:refactor --label area:tooling --label priority:P2
python3 .github/scripts/issue_labels.py create \
  --title 'process: establish one descendant cleanup owner' --body-file /tmp/issue.md \
  --label kind:refactor --label area:tooling --label priority:P2
```

The creation helper uses `GH_TOKEN` or `GITHUB_TOKEN` with the necessary
repository access; GitHub can silently drop requested labels without it, so
the helper reads the issue back and reports failure if the invariant is lost.
For another creation tool, run `validate` on exactly the labels sent to it and
verify its result. Do not create an unlabeled draft and patch labels later.

Forms require an explicit primary area and, for executable results, priority.
The change form also requires its feature/refactor/docs kind. When an issue
opens, the workflow applies only these explicit canonical selections and
validates the live labels. It never guesses classification from prose, Issue
Type or a default priority. Form answers are initial input; later triage edits
the labels, and old answers are never reapplied on edits or label removal.

GitHub has no repository setting that rejects every unlabeled API creation.
Disabling blank UI issues requires a form but does not restrict the API. Forms
therefore begin with the derived `triage:incomplete` flag until the workflow
has applied and checked their choices. A bypass through another tool is detected
after creation: missing, unknown or conflicting classification gets that flag
and an actionable workflow summary. Fix the actual labels; the guard removes
the flag when valid. Do not start work or move an incomplete card out of Triage.
The guard never closes issues or selects a priority for their author.

The workflow reads current GitHub state on issue creation, edits, label changes,
reopening, closure and transfer. It also audits all open issues on catalog/form
changes, label-definition changes, a manual run and every six hours. The audit
covers missed events, including events suppressed by `GITHUB_TOKEN`. Historical
closed cards and pull requests are outside the active issue invariant.
An incomplete card is reported and flagged without making source CI red;
API failures or a broken reconciler fail its workflow. The separate read-only
`python3 .github/scripts/issue_labels.py audit` command exits nonzero on violations.
The fast host gate tests the same policy offline through the check registry.

## Write the card

Start with the desired result and why it matters to a concrete consumer.
Then provide:

- Scope: owner paths, layer, target/profile and explicit exclusions.
- Current evidence: what is observed, inferred, unknown or already decided;
  immutable source, PR or applicable run references where available.
- Acceptance: observable conditions and the checks that establish them.
- Relations: parent, required artifact from each blocker, related work and
  the trigger for work that is deliberately parked.

A bug needs expected and observed behavior plus a reachable reproducer. A
research task needs a question, a bounded method and a usable conclusion.
A decision needs alternatives and the owner who chooses. A refactor names
the invariant rather than asking to make code cleaner. A validation task
names the subject, apparatus, measurements and tolerance before the run.

Split a card when outcomes have different owners, readiness, priority or
acceptance. Do not create implementation and qualification cards for every
speculative feature. Create them when the result is selected and the scope
is known. A tracking card supplies the integration criteria; it is not an
executable backlog item with an unbounded list of unrelated improvements.

## Compose work and express blockers

Use native sub-issues for **is part of**. A child has one primary parent;
other consumers link to it. Use native dependencies for **requires this
artifact before this step**. The blocked card names the exact artifact and
which acceptance step consumes it. Use ordinary links for shared context,
an optional improvement or a follow-up. A parent link alone never blocks
its child.

Keep dependency direction from prerequisite to consumer and avoid cycles.
Check a required artifact, not just the prerequisite's closed state. A
negative research outcome may close research while requiring a new owner
decision about the consumer. It does not make an implementation ready.

For hardware features, separate what each result proves:

| Result | Authority and output |
| --- | --- |
| Research | Reviewed source facts, applicability and limits; vendor behavior through the existing evidence workflow |
| Register/PAC publication | Typed meaning and transactions in the reviewed hardware model |
| Port or driver implementation | Ownership, lifecycle, errors and timing obligations, with focused tests |
| Portable protocol/service | Policy over the shared contract, with model tests independent of hardware |
| HIL validation | Observations of the specified image, board/revision, profile and apparatus |
| Qualification | Independent evaluation of those observations against the selected requirements |

These steps are not one mandatory serial chain. Protocol model work can run
alongside hardware research when its contract is already known. Publish an
unknown hardware semantic before relying on it; bring a materially different
contract to the owner before implementation. Host success, recovered vendor
code and a merged PR each establish their own scope. The
[verification and qualification contract](verification-and-qualification.md)
defines hardware readiness.

## Prioritize and select work

Priority means urgency toward the currently selected result, not technical
interest or task size. Do not use dates or priority to promise a delivery.

| Priority | Use |
| --- | --- |
| `priority:P0` | Unexpected failure of the mandatory main gate or a proven critical defect in an active path; interrupt other work |
| `priority:P1` | Required for the selected near-term product result or removal of its concrete blocker |
| `priority:P2` | Useful, scoped work that does not block the selected result |
| `priority:P3` | Optional or future work with an explicit trigger for activation |

Accepted gate exceptions stay explicit; they do not become new P0 incidents
merely because a dashboard remains red. An uncertain defect stays uncertain.
Do not claim a root cause or a fix from a clean run alone.

## Use one Project

Maintain the live Project configuration in GitHub; repository checks do not
create or enforce its fields, options, views or workflows. Use one Project
for the repository backlog. Priority remains a label so it is visible
outside the Project; do not maintain a second editable priority field.
Native open/closed state records closure. Project Status records execution,
never capability readiness.

If the Project uses a `Layer` navigation field, its options use the canonical
metadata names unchanged. Set it to the primary accepting package's layer;
the defining paths, other affected layers and detailed obligations stay in
the issue body. Maintain that field manually when the owner changes.

| Status | Entry condition |
| --- | --- |
| Triage | Supplied classification, scope or acceptance needs review; `triage:incomplete` must be cleared before leaving |
| Backlog | Accepted scope, not yet selected or waiting for its future trigger |
| Ready | Selected next work; required inputs and the next step are available |
| In progress | An owner is actively doing the named step |
| Review | The named deliverable is available for review or qualification evaluation |
| Waiting | Selected work awaits a named artifact, decision, apparatus or external event |
| Done | The issue is closed with its scoped outcome recorded |

At triage, review the supplied classification and priority, then inspect dependencies before
moving to Ready. Review Waiting cards when their named input changes, and
Backlog cards when their trigger happens. Do not infer In progress from a
recent edit or assign an owner who has not taken the work.

At closure, update the current summary, link the deliverable and state what
was established. Close a duplicate with a link to the retained card. A
tracking issue closes after its integration criteria, not merely when its
last implementation PR merges. Reopen on new applicable evidence instead
of rewriting an earlier uncertainty as a confirmed cause.
