# Reviewed HIL applicability

An applicability review admits an existing observation for one capability's HIL
obligation on an explicitly identified destination build. The same evaluator
uses this decision in `evaluate`, `gate`, and the manifest-backed engineering
map. It is an engineering conclusion about applicability, not a new execution.
No review is inferred from an unchanged commit, a later PASS, or a capability's
implementation status.

The default remains `current-clean-composition`. An optional `hil-reviews` list
in a capability declaration references repository-relative TOML files. Keep
accepted records with the capability's reviewed inputs; generated reports and
run archives remain in their owner's ignored outputs. Each capability may have
one active review per required scenario, including its explicit control.

## Preparing a review

Read a program-backed report, for example:

```console
cargo qualification status --manifest qualification/targets/esp32s31/wifi-sta.toml --capability runtime-phy-calibration --json-report target/qualification/phy-status.json
```

For each entry, `evidence.hil_decisions` exposes:

- `property.sha256`: the current capability scope, dependency scopes, source
  contracts, selected checks, repetition requirement and scenario definition;
- `property.required_inputs` and `property.current_inputs`: owner files from the
  capability and its dependency closure, plus existing workspace manifest,
  lockfile and toolchain file, with current SHA-256 hashes;
- `property.image_sensitive`: whether this obligation requires identical
  application bytes even when its owners are unchanged;
- `property.unmapped_capabilities`: dependency scopes whose source contracts
  still need mapping before transfer can be assessed;
- `observations`: immutable observation IDs, original outcomes and failures,
  completion seals, application bytes, build provenance, procedure, fixture and
  source-snapshot identities where recorded;
- `reviews`: each referenced review's hash, rationale and evaluation status.

The observation ID binds the run, scenario and completion seal. Resealing changed
material changes its ID. Old bundles can lack application or procedure
identities; their absence stays explicit and cannot establish a transfer.

Choose an original complete passing observation as the source. The destination
is a sealed, build-bearing observation in the selected program's run index; it
may belong to another scenario and need not pass. This identifies the actual
application being assessed without demanding a repeat of the source experiment.
It must contain the selected image class and checkable source provenance.

Review the actual source/build differences and property scope. The evaluator
checks recorded bindings; it does not discover every implicit dependency or
supply the reviewer's engineering conclusion. Include additional relevant files
in `inputs` when the required owner paths are insufficient. Update the canonical
source contracts when their ownership mapping was incomplete.

## Record format

This illustrative TOML uses placeholders for hashes; replace them with the
64-character lowercase SHA-256 values from the report. It is not accepted
as-is and does not claim evidence for any real capability.

```toml
schema = 1
id = "att-owner-unchanged"
capability = "secure-att"
scenario = "secure-att-exchange"
property-sha256 = "PROPERTY_SHA256"
kind = "unchanged-functional-contract"
reviewer = "reviewer-identity"
reason = "Relevant owners, hardware contracts and peer conditions remain applicable between these builds."

[source]
id = "SOURCE_OBSERVATION_ID"
image = "correctness"
application-sha256 = "SOURCE_APPLICATION_SHA256"

[destination]
id = "DESTINATION_OBSERVATION_ID"
image = "correctness"
application-sha256 = "DESTINATION_APPLICATION_SHA256"

[[inputs]]
path = "crates/example/src/att.rs"
sha256 = "CURRENT_OWNER_SHA256"
# Include every property.required_inputs path and any additional reviewed inputs.
```

Reference the record in the owning capability:

```toml
hil-reviews = ["qualification/reviews/att-owner-unchanged.toml"]
```

Paths must be contained regular files, without symlink components. Unknown
fields, malformed identities, duplicate inputs/reviews and undeclared scenarios
are validation errors. A well-formed but stale review remains visible with its
reason, such as `property-changed`, `current-input-changed`,
`build-binding-mismatch` or `source-observation-missing`; it admits no evidence.
`next` explains that applicability needs reassessment. Independently eligible
current evidence can still satisfy the obligation.

## What the evaluator checks

`unchanged-functional-contract` requires the reviewed input hashes to agree in
both builds and the current checkout. A sealed source snapshot is checked
independently, including its identity, complete archive membership and each
file's content. For a clean-commit build, the evaluator reads the named Git
blobs; missing objects cannot establish the binding. Dirty sources require the
captured snapshot. A dirty current checkout is allowed when these explicit
bindings hold; unrelated dirty files do not silently invalidate the observation.

`identical-image` requires equal application bytes and binds the destination
sources to the current reviewed inputs. Numeric throughput/silence checks also
require equal application bytes, even with the functional kind. Whole scenarios
with performance/latency criteria and existing GATT stack-headroom, memory
benchmark, timebase and boot-placement assertions follow the same rule. Selecting
published functional checks binds only those checks, while still requiring a
complete original scenario PASS. Future quantitative RF measurements need their
own explicit observation contracts.

Both kinds require coherent archived application/build provenance, matching
build selection/features and unchanged declared external source composition.
Replay artifacts cannot be admitted by this mechanism. A review cannot invent
missing measurements, complete interrupted repetitions, combine checks from
different runs or turn a failed scenario into PASS. Required controls must pass
in the original run with the same repetition set and must themselves be
applicable; historical controls can have their own property-scoped review.

The status `applied` describes the review's bindings. The obligation may still
be `unresolved-failure` or missing its applicable control. Original exclusions
are retained alongside the explicit applicability decision.

## Resolving a failure

Applying a transfer also considers recorded failures of the same scenario,
including failures previously excluded by commit/cleanliness policy. Neither
an older PASS nor a later PASS hides them. Explain each irrelevant failure or
bind its fix to a later complete passing observation on the destination
application, within the same record:

```toml
[[failures]]
observation = "FAILED_OBSERVATION_ID"
disposition = "fixed"
reason = "The fix was exercised by the later complete destination experiment."
resolving-observation = "LATER_PASS_OBSERVATION_ID"

[[failures]]
observation = "OTHER_FAILED_OBSERVATION_ID"
disposition = "not-applicable"
reason = "Explain the concrete difference that excludes this observation from the reviewed property."
```

`fixed` requires the later observation to pass the entire obligation and its
original control; its application hash must match the destination.
`not-applicable` requires an explicit reason and forbids a resolving observation.
Both remain visible alongside the original failure. A newly indexed relevant
failure has no disposition and reopens the decision. For a fix confirmed on the
current build, source and destination may name that same passing observation;
the older failure is still recorded separately.
