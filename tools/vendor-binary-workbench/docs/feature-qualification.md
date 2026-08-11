# Feature qualification and vendor-effect coverage

A passing comparison of one convenient function does not qualify a driver
feature. The feature pack closes the boundary between an explicit vendor review
scope and the intended Rust policy.

Feature-pack schema 2 requires every explicit root of every selected review
scope to have exactly one `[[features.effects]]` disposition:

```toml
schema = 2

[[features]]
id = "wifi-sta-connected-no-power-save"
description = "Connected STA keeps the hardware beacon filter disabled."
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

The generated review-scope schema 7 stores
`replacement_function_keys`; these are the exact denominator for effect
coverage. Reachable private helpers remain analysis inventory and do not force
a fictitious one-to-one Rust function. Promote a helper to an explicit scope
root when its transaction is independently part of the feature boundary.

`project status`, the Features TUI view and the application snapshot expose
covered/total scope effects separately from verification requirements. A
feature is qualified only when scope analysis is complete, every effect has a
current disposition, and every referenced proof satisfies its requested claim
and reviewed production binding.
