# Verification contracts

This document covers the reviewed semantic layer between generic binary
analysis and platform-specific replacement policy. The RISC-V backend knows
neither RTOS, NVS, logging nor ESP32 lifecycle semantics. Those claims enter
through target-owned dispositions, bindings and semantic/effect contracts.

## Dispositions

The disposition manifest classifies exact `(source, symbol)` identities and
uses fail-closed defaults for everything else. Supported dispositions are:

- `direct`;
- `state-transition`;
- `replaced-by-composition`;
- `generation-candidate`;
- `not-yet-ported`.

Implemented entries name a `rust-component`. An implemented component without
an executable semantic or effect contract remains
`IMPLEMENTED-UNQUALIFIED`; prose and source presence do not become evidence.
Each `[[functions.blocked-by]]` table expresses an exact qualification
dependency through `source` and `symbol`. The loader rejects missing blocker
targets and duplicate entries.

The component id is checked against the current Cargo workspace source AST and
the exact suite ELF/DWARF facts in the aggregate project report. This catches
stale module paths and distinguishes a compiled production item or inline
frame from a merely present `no_mangle` probe. The join is navigation and
currency evidence; qualification still comes only from the declared semantic
or effect contract.

Protocol classification is independent from completion. Shared PHY/RF,
Wi-Fi, Bluetooth, BLE, Coex and 802.15.4 counts therefore remain visible even
when a function is not yet ported.

## Bindings

Binding v1 selects one exact compiled Rust probe. It is independent of the
convention-based `--rust-prefix`. `compare-return = true` additionally compares
the observable ABI return register; it is opt-in because a machine value in
`a0` alone does not prove the unavailable C prototype declared a return.

```toml
schema = 1
default-disposition = "not-yet-ported"
default-protocol = "unknown"

[[functions]]
source = "rom"
symbol = "phy_disable_agc"
disposition = "direct"
rust-component = "open_esp_radio_esp32s31_hal::phy_agc::set_enabled"
binding = "v1"
rust-probe = "open_phy_trace_disable_agc"
effect-contract = "exact-effects-v1"

[[functions.effects]]
selector = { kind = "mmio-read", width = 32, address = 0x20107030 }
disposition = { kind = "required" }

[[functions.effects]]
selector = { kind = "mmio-write", width = 32, address = 0x20107030 }
disposition = { kind = "required" }
```

## Effect Contract v1

The effect contract is the closed comparison boundary between vendor and Rust
implementations. Its vocabulary contains MMIO read/write, projected state
read/write, delay, await-ready, typed platform calls and named semantic
boundary events.

Every vendor effect must resolve to exactly one rule. Closed dispositions are
`required`, `replaced-by-async`, `platform-provided-input`,
`platform-provided-service`, `published-event`,
`initialization-prerequisite`, `platform-owned`, `forbidden`, and
`allowed-omission`. Omission requires one of the enumerated reasons, such as
`debug-diagnostic`, `nvs-calibration-cache`, `rtos-scheduling-adapter`, or
`unused-instrumentation`.

Semantic replacements must appear as the exact named Rust boundary event;
declaring a policy never permits silent omission. Async replacement fixes a
named condition and a non-zero attempts or deadline bound. Unknown effects,
unclassified vendor effects, extra Rust effects and arbitrary omissions fail
closed.

Semantic contracts are platform-owned executable checks for larger
composition/state transitions. They compare their declared projection without
claiming an independent proof for every transitive leaf. Platform packs select
reusable semantic catalogs, while reviewed interface packs bind concrete
trampoline slots to those semantics.

## Evidence binding

Effect evidence hashes the canonical policy and binding, comparator, binding
verifier, reference generator, generated reference proof and relevant adapter
or execution sources. Semantic evidence similarly includes every registered
contract source. Changing or weakening the proof boundary therefore produces
a new evidence identity that must pass the explicit review workflow.
