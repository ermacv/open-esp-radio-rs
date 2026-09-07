# PHY compiled comparison

PHY comparison uses authenticated vendor and Rust artifacts through
[Blobray verification](../../tools/blobray/docs/verification.md). The
[PHY feature inventory](../../crates/hardware/esp32s31/phy/FEATURES.md) describes
implemented primitives and protocol consumers; comparison reports describe
evidence for exact declared boundaries, not general RF readiness.

## Inputs and ownership

The [ESP32-S31 investigation](../../verification/vendor/projects/esp32s31/README.md)
selects the reviewed radio model, upstream platform register catalog, vendor
identities, dispositions, execution profiles and compiled Rust bindings.
Caller-owned artifact paths belong in its ignored run spec. The separate
[source-only publication](../../registers/esp32s31/publication/README.md)
checks generated PAC/model consistency without authenticating private binaries.

[Comparison probes](../../verification/vendor/projects/esp32s31/probes/README.md)
retain entry points into production PHY/HAL code. A probe's name or a generated
reference is not evidence that the shipping entry executes that behavior.
Dispositions describe replacement ownership and scope; profiles define the
explicit input domain, environment, observations and comparison policy.

## Evidence and limits

The current verifier records `evidence_class` as `production-trace`,
`shared-core` or `static-analysis`. Only exact compiled production execution
can supply production-trace evidence. Matching a model or a shared child does
not qualify the composed PHY registration or channel-switch entry.

Function statuses include `match`, `bounded-match`, `mismatch`, `incomplete`,
`implemented-unqualified` and `uncovered`. Missing probes, unresolved calls,
unknown state or incomplete execution must remain explicit. A bounded match
applies only to its declared preconditions. State-only comparison does not
prove equality of omitted MMIO or calls; reviewed call and effect contracts
retain their separately declared scope.

Stateful profiles carry explicitly retained writable memory across ordered
cases, while stacks, MMIO responses and device models remain phase-local.
The [verification contract](../../tools/blobray/docs/verification.md)
defines the accepted profile policies and evidence-baseline rules. Regression
gates protect accepted evidence; completion gates require the selected scope.
Neither can manufacture an implementation or hide an observed difference.

## Readiness

The [qualification evaluator](../../qualification/README.md) independently
consumes release-eligible evidence and current clean HIL bundles. It checks
source freshness, declared capability requirements and dependencies; Blobray
comparison alone cannot establish readiness. The current
[Wi-Fi target](../../qualification/targets/esp32s31/wifi-sta.toml) includes cold
registration, RF/baseband initialization and channel selection, with explicit
remaining evidence gaps. Shared PHY consumer/lifecycle limits also appear in
the Bluetooth and IEEE 802.15.4 qualification programs.

Reviewed source identities and immutable evidence retain their original
meaning. Update the exact source contract and produce new evidence when a
compiled boundary changes; do not relabel historical outputs as current proof.
