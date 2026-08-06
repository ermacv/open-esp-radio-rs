# PHY binary parity

PHY parity is verified from compiled vendor and Rust code by
[`vendor-binary-workbench`](../../tools/vendor-binary-workbench/README.md). The generated
report is the inventory and open-work list; this directory does not maintain a
parallel, hand-written function ledger.

The verifier composes `svd/esp32s31-radio.svd` with the validator-only official
PAC subset in `svd/esp32s31-platform-radio-deps.svd`. A `MATCH` needs no
additional per-function document, but its row retains the strength of the
evidence: `evidence=symbolic` for normalized symbolic equality,
`evidence=scenario` for declared concrete cases with complete branch outcomes,
and `evidence=state` for a canonical semantic pre/post projection.
`evidence=composition-state-scenario` compares normalized vendor call/MMIO events and
call-time state payloads with the public actions and final typed state of a
Rust transition. It permits an async Rust architecture instead of requiring
the vendor polling-loop shape. Stateful roots additionally execute call
sequences on persistent ELF-backed RAM while resetting private stack and MMIO
environment state for every invocation. Concrete evidence is not presented as
exhaustive proof over an undeclared input domain.
Missing branch outcomes, calls, unresolved values, poison memory reads,
missing Rust probes and SVD gaps are emitted as `UNCOVERED-*`/`INCOMPLETE`
rows.

Use the regression gate to protect already established evidence while the
port is incomplete; use the completion gate to require the entire selected
vendor inventory. The machine-readable disposition manifest separates
not-yet-ported functions from implemented architectural replacements that do
not yet have a semantic contract. Qualification dependencies for those roots
are source-qualified `blocked-by` edges in that same manifest and are checked
against the vendor inventory. The tool README defines both gates and the
current floor.

There are currently no accepted parity exceptions. If one becomes necessary,
it must be a typed, reviewable rule in the verifier with a failing regression
test for any scope outside that rule; it must not be hidden in prose.

The chip/protocol boundary is documented in
[`docs/ARCHITECTURE.md`](../ARCHITECTURE.md), register provenance in
[`docs/esp32s31-radio-register-provenance.md`](../esp32s31-radio-register-provenance.md),
and hardware results under the
[ESP32-S31 qualification records](../../qualification/targets/esp32s31/records/README.md).
