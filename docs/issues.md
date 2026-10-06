# Issues and dependencies

An issue names one result, the owner that accepts it and the evidence needed
to close it. Keep the current scope and accepted decisions in the body. Keep
experiments and superseded proposals in comments, with a link from the current
summary when they explain a decision. Task tracking stays in GitHub; owner
documentation describes the implemented contract.

## Classify the result

Use one primary `kind:*`, one primary `area:*` and, after triage, one
`priority:*` on an executable task. A tracking issue can omit priority because
its children have different urgency. Add another area only when it helps an
actual consumer find the task. Add `target:*` only for chip-specific scope;
record revision, board, profile and features in the body.

The [label definitions](../.github/labels.json) hold the names, descriptions
and colors. A form's area or kind dropdown does not assign labels: triage
applies the matching labels. The change form deliberately leaves its kind
unset until its selected result is reviewed.

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
body name the defining paths and the layer: register/PAC, hardware mechanism,
port contract, protocol/model, service, runtime/composition, platform,
tooling, HIL or qualification. These layers are separate responsibilities,
not additional label families.

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

The repository backlog uses one Project. Priority remains a label so it is
visible outside the Project; do not maintain a second editable priority
field. Native open/closed state records closure. Project Status records
execution, never capability readiness.

Set the Project's `Layer` to the primary affected boundary for navigation:
Registers/PAC, Hardware/driver, Port contract, Protocol/service,
Runtime/composition, Platform, Tooling, HIL or Qualification. The defining
paths and detailed obligations stay in the issue body. Layer views separate
driver work from portable protocol work within the same area.

| Status | Entry condition |
| --- | --- |
| Triage | Kind, scope, acceptance or priority needs review |
| Backlog | Accepted scope, not yet selected or waiting for its future trigger |
| Ready | Selected next work; required inputs and the next step are available |
| In progress | An owner is actively doing the named step |
| Review | The named deliverable is available for review or qualification evaluation |
| Waiting | Selected work awaits a named artifact, decision, apparatus or external event |
| Done | The issue is closed with its scoped outcome recorded |

Keep views for executable P0/P1 work, hardware research, drivers/ports,
portable protocols, refactoring, validation and tracking. Filters use labels;
the layer and owner paths in the issue explain finer distinctions. HIL and
qualification work stays discoverable independently from implementation.

At triage, classify and assign priority, then inspect dependencies before
moving to Ready. Review Waiting cards when their named input changes, and
Backlog cards when their trigger happens. Do not infer In progress from a
recent edit or assign an owner who has not taken the work.

At closure, update the current summary, link the deliverable and state what
was established. Close a duplicate with a link to the retained card. A
tracking issue closes after its integration criteria, not merely when its
last implementation PR merges. Reopen on new applicable evidence instead
of rewriting an earlier uncertainty as a confirmed cause.
