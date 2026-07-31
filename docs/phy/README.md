# PHY binary parity

PHY parity is verified from compiled vendor and Rust code by
[`open-esp-radio-phy-trace`](../../tools/phy-trace/README.md). The generated
report is the inventory and open-work list; this directory does not maintain a
parallel, hand-written function ledger.

The verifier reads register identity from `svd/esp32s31-radio.svd`. A `MATCH`
needs no additional per-function document, but its row retains the strength of
the evidence: `evidence=symbolic` for normalized symbolic equality or
`evidence=scenario` for the declared concrete cases with complete branch
outcomes. `evidence=state` compares a canonical semantic pre/post projection:
vendor offsets are decoded at the oracle boundary while Rust publishes typed
state through a stable test-only exporter. It does not inspect the private Rust
layout. Concrete evidence is not presented as exhaustive proof over an
undeclared input domain. Missing branch outcomes, calls, unresolved values,
poison memory reads, missing Rust probes and SVD gaps are emitted as
`UNCOVERED-*`/`INCOMPLETE` rows.

Use the regression gate to protect already established evidence while the
port is incomplete; use the completion gate to require the entire selected
vendor inventory. The tool README defines both gates and the current floor.

There are currently no accepted parity exceptions. If one becomes necessary,
it must be a typed, reviewable rule in the verifier with a failing regression
test for any scope outside that rule; it must not be hidden in prose.

The chip/protocol boundary is documented in
[`docs/ARCHITECTURE.md`](../ARCHITECTURE.md), register provenance in
[`docs/esp32s31-radio-register-provenance.md`](../esp32s31-radio-register-provenance.md),
and hardware results under [`docs/hil/`](../hil/README.md).
