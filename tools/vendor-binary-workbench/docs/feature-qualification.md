# Feature qualification and vendor-effect coverage

A passing comparison of one convenient function does not qualify a driver
feature. The feature pack closes either a complete vendor review surface or a
narrow, explicitly replayed property against the intended Rust policy. These
are different claims and must not be inferred from one another.

Feature-pack schema 3 makes that boundary explicit with `coverage`.
`review-scopes` requires every explicit root of every selected review scope to
have exactly one `[[features.effects]]` disposition:

```toml
schema = 3

[[features]]
id = "wifi-sta-connected-no-power-save"
description = "Connected STA keeps the hardware beacon filter disabled."
coverage = "review-scopes"
scopes = ["wifi-sta-beacon-filter-policy"]

[[features.effects]]
id = "filter-disable"
source = "wifi-sta-lifecycle"
symbol = "hal_disable_sta_beacon_filter"
disposition = "verified"
requirement = "filter-disable-proof"
rationale = "The complete disable transaction is required on connected entry."

[[features.effects]]
id = "filter-enable"
source = "wifi-sta-lifecycle"
symbol = "hal_enable_sta_beacon_filter"
disposition = "excluded-by-feature-policy"
rationale = "Power-save is absent and software monitoring owns every beacon."
```

`verified` must name a requirement for the same source and symbol.
`excluded-by-feature-policy` must contain a rationale and cannot borrow an
unrelated proof. Missing dispositions and dispositions left behind after a
scope change both block qualification. Consequently, adding
`hal_set_sta_beacon_filter` to the scope cannot silently leave a previously
green feature green.

Use `bounded-evidence` only when the release assertion is deliberately narrower
than whole-function or whole-scope equivalence. It must not select review
scopes, every declared effect must be `verified`, and at least one explicit
requirement and effect are mandatory:

```toml
[[features]]
id = "wifi-ap-sta-key-role"
description = "The production key builders encode STA context 0 and AP context 1."
coverage = "bounded-evidence"

[[features.requirements]]
id = "key-role-proof"
description = "Pinned vendor propagation agrees with executed production Rust builders."
suite = "wifi-key-role"
source = "wifi-key-role"
symbol = "wDev_Insert_KeyEntry"
claim = "rust-conformance"

[[features.effects]]
id = "connection-context"
source = "wifi-key-role"
symbol = "wDev_Insert_KeyEntry"
disposition = "verified"
requirement = "key-role-proof"
rationale = "The two-bit context property is independently pinned and replayed."
```

This qualifies only the named property. It does not claim that
`wDev_Insert_KeyEntry`, its callees, or a related review scope has complete
semantic coverage. Policy exclusions therefore belong only to
`review-scopes`; they are rejected for bounded evidence because that mode has
no discovered surface denominator from which an omission could be justified.

The generated review-scope schema 8 stores
`replacement_function_keys`; these are the exact denominator for effect
coverage. Reachable private helpers remain analysis inventory and do not force
a fictitious one-to-one Rust function. Promote a helper to an explicit scope
root when its transaction is independently part of the feature boundary.

The matching disposition for a deliberately narrow proof is
`bounded-feature`. The verifier emits `bounded-match` only for a successful
non-whole-function adapter claim. Project loading rejects a bounded
disposition that is not selected by a required feature, and `project check`
evaluates those required features as a separate fail-closed gate.

`project status`, the Features TUI view and the application snapshot expose the
coverage mode and covered/total surface effects separately from verification
requirements. A `review-scopes` feature is qualified only when scope analysis
is complete and every discovered effect has a current disposition. A
`bounded-evidence` feature is qualified only for its explicit effects. In both
modes every referenced proof must satisfy its requested claim and reviewed
production binding.
