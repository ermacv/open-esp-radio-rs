# Feature qualification and vendor-effect coverage

A passing comparison of one convenient function does not qualify a driver
feature. The feature pack closes either a complete vendor review surface or a
narrow, explicitly replayed property against the intended Rust policy. These
are different claims and must not be inferred from one another.

Feature-pack schema 5 makes that boundary explicit with `coverage` and named
lifecycle phases. `review-scopes` requires every effect-bearing function in
the reachable closure of every selected review scope to have exactly one
fingerprinted `[[features.effects]]` disposition. Pure routing helpers stay in
the call graph but do not inflate the transaction denominator:

```toml
schema = 5

[[features]]
id = "wifi-sta-connected-no-power-save"
description = "Connected STA keeps the hardware beacon filter disabled."
coverage = "review-scopes"
scopes = ["wifi-sta-beacon-filter-policy"]

[[features.phases]]
id = "connected-policy"
description = "Transfer beacon delivery ownership to the software monitor."
scopes = ["wifi-sta-beacon-filter-policy"]

[[features.effects]]
id = "filter-disable"
phase = "connected-policy"
vendor = { source = "wifi-sta-lifecycle", symbol = "hal_disable_sta_beacon_filter", fingerprint = "sha256:REVIEW" }
disposition = "verified"
requirement = "filter-disable-proof"
rationale = "The complete disable transaction is required on connected entry."

[[features.effects]]
id = "filter-enable"
phase = "connected-policy"
vendor = { source = "wifi-sta-lifecycle", symbol = "hal_enable_sta_beacon_filter", fingerprint = "sha256:REVIEW" }
disposition = "excluded-by-feature-policy"
rationale = "Power-save is absent and software monitoring owns every beacon."
```

`verified` must name a requirement; that proof may establish a composed Rust
replacement rather than only the vendor leaf with the same symbol.
`excluded-by-feature-policy` must contain a rationale and cannot borrow an
unrelated proof. Missing dispositions and dispositions left behind after a
scope change both block qualification. A transaction fingerprint is computed
from ordered canonical MMIO, RAM, semantic-call, delay and event effects. It
deliberately excludes instruction PCs, so relinking alone does not create
review churn while a semantic change does. Consequently, adding a reachable
helper with an observable transaction cannot silently leave a previously green
feature green.

Use `bounded-evidence` only when the release assertion is deliberately narrower
than whole-function or whole-scope equivalence. It must not select review
scopes, every declared effect must be `verified`, and at least one explicit
requirement and effect are mandatory:

```toml
[[features]]
id = "wifi-ap-sta-key-role"
description = "The production key builders encode STA context 0 and AP context 1."
coverage = "bounded-evidence"

[[features.phases]]
id = "key-install"
description = "Select the role-specific hardware key context."

[[features.requirements]]
id = "key-role-proof"
phase = "key-install"
description = "Pinned vendor propagation agrees with executed production Rust builders."
suite = "wifi-key-role"
source = "wifi-key-role"
symbol = "wDev_Insert_KeyEntry"
claim = "rust-conformance"

[[features.effects]]
id = "connection-context"
phase = "key-install"
vendor = { source = "wifi-key-role", symbol = "wDev_Insert_KeyEntry", fingerprint = "sha256:REVIEW" }
disposition = "verified"
requirement = "key-role-proof"
rationale = "The two-bit context property is independently pinned and replayed."
```

This qualifies only the named property. It does not claim that
`wDev_Insert_KeyEntry`, its callees, or a related review scope has complete
semantic coverage. Policy exclusions therefore belong only to
`review-scopes`; they are rejected for bounded evidence because that mode has
no discovered surface denominator from which an omission could be justified.

Use `selected-evidence` when the feature boundary is a finite list of named
vendor functions rather than a reachable review scope. Unlike
`bounded-evidence`, it may require `whole-function-equivalence`; every listed
transaction still needs a pinned fingerprint, verified disposition and current
verification result. It deliberately makes no statement about unlisted
functions, so a product capability should compose it with the other required
leaves instead of treating it as complete artifact coverage.

Use `composed-features` for a product capability whose proof is the conjunction
of already reviewed features. It cannot select scopes or repeat requirements
and effects. Dependencies may select the complete feature or one lifecycle
phase, and cycles or unknown selectors are rejected while loading the pack:

```toml
[[features]]
id = "wifi-sta-ap-register-prerequisites"
description = "Role-specific register leaves needed by concurrent STA+AP."
coverage = "composed-features"

[[features.phases]]
id = "register-prerequisites"
description = "Collect independently qualified register contracts."

[[features.dependencies]]
feature = "wifi-ap-sta-interface-identity"
phase = "interface-identity"

[[features.dependencies]]
feature = "wifi-ap-sta-key-role"
phase = "key-install"
```

The composed report contains one concise blocker per failed dependency and
keeps the leaf blockers on the leaf report. This prevents an umbrella feature
from duplicating hundreds of transaction diagnostics while preserving a
fail-closed path to the exact evidence.

The generated review-scope schema 9 stores `transactions`, their canonical
fingerprints and every root-to-transaction path. This is the exact denominator
for `review-scopes` coverage. `replacement_function_keys` remains useful for
navigation and Rust ownership, but is no longer a loophole through which a
reachable vendor side effect can disappear.

The matching disposition for a deliberately narrow proof is
`bounded-feature`. The verifier emits `bounded-match` only for a successful
non-whole-function adapter claim or an explicitly preconditioned
`reviewed-domain-equivalence` execution profile. Project loading validates
bounded declarations across every feature, while the binding audit gates only
the transitive dependency closure of required features. This permits honest
research-only bounded work without letting it block or silently qualify an
unrelated release. `project check` evaluates the required closure as a
separate fail-closed gate.

`project status`, the Features TUI view and the application snapshot expose the
coverage mode and covered/total surface effects separately from verification
requirements. A `review-scopes` feature is qualified only when scope analysis
is complete and every discovered effect has a current disposition. A
`bounded-evidence` feature is qualified only for its explicit effects. A
`selected-evidence` feature is qualified only for its explicit named
transactions and accepts the claim strength requested by each requirement. A
`composed-features` feature is qualified only when all selected dependencies
are qualified. In every mode each referenced proof must satisfy its requested
claim and reviewed production binding.

Use `project feature FEATURE` for the focused human report. It shows the
lifecycle phases, current transaction coverage and proof blockers without
loading or serializing the full IR. `--details` adds paths and canonical
effects; `--write-review-draft PATH` writes a deliberately non-authoritative
TOML candidate for new or changed transactions.

## Hardware qualification

Static qualification and hardware evidence are separate trust levels. A
feature with a `[features.hardware]` contract reports `QUALIFIED` when its
static boundary is closed and `HARDWARE-QUALIFIED` only when the configured
evidence also has enough successful runs, every required observation and
current digests for every required artifact:

```toml
[features.hardware]
minimum-successful-runs = 20
required-observations = ["beacon", "wpa2-association", "bidirectional-data"]
required-artifacts = ["firmware", "project-manifest", "feature-pack", "register-model"]
```

The project points to the HIL-produced JSON document explicitly:

```toml
[qualification]
pack = "features/reviewed.toml"
required-features = ["radio-product-contract"]
hardware-evidence = "generated/evidence/feature-hardware.json"
```

Evidence artifact paths are relative to the evidence document. Their SHA-256
digests are recomputed before a feature can become hardware-qualified. Normal
`project check` gates static qualification; `project check --hardware` also
requires every required feature to be `HARDWARE-QUALIFIED`.

The HIL producer writes one strict JSON document; this is evidence, not a
hand-edited review pack:

```json
{
  "schema": 1,
  "command": "project hardware evidence",
  "features": [{
    "id": "radio-product-contract",
    "passed": true,
    "successful_runs": 20,
    "observations": ["beacon", "wpa2-association", "bidirectional-data", "clean-stop"],
    "artifacts": [
      {"id": "firmware", "path": "../../firmware.elf", "sha256": "..."},
      {"id": "project-manifest", "path": "../../vendor-project.toml", "sha256": "..."},
      {"id": "feature-pack", "path": "../../features/reviewed.toml", "sha256": "..."},
      {"id": "register-model", "path": "../../registers/device.toml", "sha256": "..."}
    ]
  }]
}
```
