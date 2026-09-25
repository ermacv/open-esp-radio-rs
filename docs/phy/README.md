# PHY compiled comparison

PHY comparison executes authenticated vendor artifacts and compiled Rust probes
in the [typed vendor scenarios](../../verification/vendor/projects/esp32s31/README.md). The
[PHY inventory entry point](../../crates/hardware/esp32s31/phy/FEATURES.md) links
the canonical catalog and its generated primitive/consumer view; comparison reports describe
evidence for exact declared boundaries, not general RF readiness.

## Inputs and ownership

Each scenario authenticates its private vendor inputs by SHA-256, selects exact
vendor roots and compiled production probe entries, and declares its input
domain, peripheral models and expected verdicts. Private artifact paths are
explicit scenario arguments. The separate
[source-only publication](../../registers/esp32s31/publication/README.md)
checks generated PAC/model consistency without authenticating private binaries.

[Comparison probes](../../verification/vendor/projects/esp32s31/probes/README.md)
retain entry points into production PHY/HAL code. A probe's name or a generated
reference is not evidence that the shipping entry executes that behavior.

## Evidence and limits

Every case compares vendor execution with the exact compiled production entry
and has an expected verdict: `MATCH`, `DIFF` or `INCOMPLETE`. Missing probes,
unresolved calls, unknown state or incomplete execution stay explicit and never
become a match. Effect contracts name each ignored or omitted vendor effect
with its reason; any other unclassified effect fails the comparison. Matching a
shared child does not qualify the composed PHY registration or channel-switch
entry.

Ordered cases carry explicitly retained writable memory, while stacks, MMIO
responses and peripheral models remain phase-local. Negative cases with
deliberately changed inputs must produce `DIFF`, so a comparison that cannot
distinguish them fails. A scenario cannot manufacture an implementation or hide
an observed difference.

## Readiness

The [qualification evaluator](../../qualification/README.md) independently
consumes release-eligible vendor evidence and applicable sealed HIL observations,
including explicitly reviewed transfers. It checks
source freshness, declared capability requirements and dependencies; Blobray
comparison alone cannot establish readiness. The current
[Wi-Fi target](../../qualification/targets/esp32s31/wifi-sta.toml) includes cold
registration, RF/baseband initialization and channel selection, with explicit
remaining evidence gaps. Shared PHY consumer/lifecycle limits also appear in
the Bluetooth and IEEE 802.15.4 qualification programs.

Reviewed source identities and immutable evidence retain their original
meaning. Update the exact source contract and produce new evidence when a
compiled boundary changes; do not relabel historical outputs as current proof.
