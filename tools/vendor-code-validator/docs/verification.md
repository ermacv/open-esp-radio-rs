# Verification

## Source and inventory verification

Verify every ROM function against a conventionally named Rust probe and report
missing probes as uncovered work:

```console
cargo vendor-code-validator verify source \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --vendor-artifact "$ESP32S31_ROM_ELF" \
  --rust-artifact \
    target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/\
open-esp-radio-verification-esp32s31-probes-elf
```

Generate the authoritative combined report for both vendor sources:

```console
# First copy verification/vendor/targets/esp32s31/run.spec.example to an untracked local file
# and replace its placeholder paths with authenticated inputs.
cargo vendor-code-validator verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --run-spec /path/to/local-esp32s31.run \
  --gate regression --match-floor 104 \
  --json-report oracle-regression.json
```

Run-spec paths are resolved relative to that file. Explicit command options
override the same role from the run spec, and repeated `input companion` lines
remain available for workflows that accept multiple companions. Authentication
is intentionally outside the validator; the protected oracle workflow checks
caller-configured SHA-256 values before it creates its run spec.

`verify-all` treats `(vendor source, symbol)` as function identity. This is
necessary because `phy_fe_reg_update` exists in both ROM and `libphy.a`; the
two implementations remain separate rows and the inventory total is therefore
305 + 161 = 466, not the 465 unique spellings. It emits per-source summaries
and one `TOTAL-SUMMARY`. Rust probes are checked against the combined
inventory, so a probe belonging to one source is not falsely reported as an
orphan while the other source is being processed.

## Dispositions and effect contracts

The disposition manifest classifies every exact implemented replacement and
uses fail-closed defaults for the rest. A `semantic-contract` names executable
validator logic; an `effect-contract` names the canonical effect comparator.
A Rust component without either remains
`IMPLEMENTED-UNQUALIFIED` and does not count as evidence. Executable root
contracts are reported separately as `composition-match`: they compare their
declared action/state projection but do not imply an independent proof for
every transitive leaf. Such an entry can use
one or more `blocked-by SOURCE SYMBOL` directives. The verifier rejects a
missing blocker target and prints the source-qualified blockers in the report,
so an architectural root cannot hide an unported child behind prose. Protocol
classification is independent, so shared PHY/RF, Wi-Fi, Bluetooth, BLE, Coex
and 802.15.4 scope are not inferred from completion status.

Effect Contract v1 is the common boundary between a vendor implementation and
a Rust implementation. Its closed effect vocabulary is MMIO read/write,
projected state read/write, delay, await-ready, typed platform call and four
named semantic-boundary events. Each vendor effect must resolve to one exact
manifest rule with one of these closed dispositions: `required`,
`replaced-by-async`, `platform-provided-input`, `platform-provided-service`,
`published-event`, `initialization-prerequisite`, `platform-owned`,
`forbidden`, or `allowed-omission` with one of `debug-diagnostic`,
`nvs-calibration-cache`, `rtos-scheduling-adapter`, and
`unused-instrumentation`. The four semantic replacements require the compiled
Rust trace to publish the exact named boundary; declaring one is not permission
to silently omit the vendor effect. This is how a constructor-supplied MAC
address, an Embassy wake service, a typed driver event or a separately proven
MAC-clock prerequisite replaces vendor-owned eFuse/RTOS/global-init behavior.
Unknown effect kinds, platform operations, omission reasons, unclassified
vendor effects and extra Rust effects are errors. An async replacement also
fixes one named condition and one non-zero `attempts=COUNT` or
`deadline-us=COUNT`; an arbitrary await or a changed deadline cannot satisfy
the rule.

The first direct vertical slice is intentionally small and exact:

```text
function rom phy_disable_agc
disposition direct
rust-component open_esp_radio_esp32s31_hal::phy_agc::set_enabled
binding v1
rust-probe open_phy_trace_disable_agc
effect-contract exact-effects-v1
effect mmio-read 32 0x20107030 required
effect mmio-write 32 0x20107030 required
```

The verifier derives those two effects independently from the caller-supplied ROM ELF,
the recompiled generated reference and the compiled production Rust probe.
Binding v1 verifies that the exact `rust-probe` symbol exists in the supplied
Rust ELF. Input revision and authenticity are deliberately caller-owned. The
verifier selects that probe from the binding instead of falling back to the
naming convention. `compare-return true` additionally binds the observable
ABI return register; without it the contract compares effects only. The flag
is deliberately opt-in because a machine value in `a0` is not evidence that
an unavailable C prototype declared a return value.
The effect evidence digest covers the canonical binding, policy, comparator,
binding validator, generator, generated harness, normalized generated source,
re-extracted effects and exact Rust compiler identity, so weakening or changing
any part of the proof requires a reviewed baseline change. Local artifact path
spellings are excluded from the source identity, while computed content
identities remain included as descriptive provenance.

The blocking-to-async slice is now executable for ROM
`phy_iq_est_enable`. Its closed `esp32s31-iq-est-enable-v1` driver adapter
compares three things independently: concrete ROM execution, the release/LTO
probe compiled from the production HAL/PAC leaves, and the public actions of
`PhyDcIqEstimateTransition`. Three scenarios cover immediate ready, inactive
then ready, and active/inactive/ready; together they cover all four ROM branch
outcomes. The vendor `phy_param+0x1ac` halfword is projected onto the typed
`readiness_activity_edges` state field. The one-microsecond delay must become
`timer-1us deadline-us=1`, each live ready sample must become
`iq-estimator-ready attempts=10000`, and the typed timeout must traverse the
complete disable tail. The evidence also binds the generated reference source,
the selected vendor and release-probe code closures, scenario inputs, adapter,
transition, target-port, execution engine and comparator sources. Whole
artifact digests remain reported as caller-owned provenance, but unrelated
linked functions do not enter this adapter baseline.

`PROTOCOL-INVENTORY` reports `executable-bindings` separately from exact
disposition entries. This keeps migration honest: legacy semantic contracts
remain visible but are not presented as artifact/probe-bound until they adopt
Binding v1. A Binding v1 function containing an unresolved call cannot reach
effect comparison; the ordinary extractor marks the trace incomplete. Typed
call dispositions are added only when a pilot needs a deliberate composition,
async replacement, platform boundary, or closed omission.

## Regression and completion gates

The two verification gates answer different questions:

- `--gate regression --match-floor 104 --evidence-baseline PATH` passes when
  there are no mismatches, incomplete comparisons, or orphan probes, at least
  104 functions retain evidence, and every source-qualified baseline function
  retains the same evidence kind. A lost state proof cannot be hidden by a new
  scenario match elsewhere. New evidence is reported as `EVIDENCE-ADDITION`
  and does not require weakening the existing baseline. Profile evidence also
  contains a hash of the parsed scenario contract, its explicit ABI argument
  domain, and the parser, comparison, reachability and execution-engine
  sources. Narrowing inputs or `arg-range`, changing observations or scripted
  responses, or weakening the verifier therefore requires a reviewed baseline
  change.
  Composition evidence contains a SHA-256 over the contract label, scenario
  wiring, semantic normalizer/footprints and execution engine sources. Editing
  the validator itself therefore also requires an explicit baseline review.
- `--rust-prefix` scopes convention-paired probes and orphan accounting for a
  particular verification run. Exact Binding v1 probes remain selectable by
  their full symbol names even when they use another prefix. A focused pilot
  sharing one Rust probe ELF with other suites must set its own prefix so
  unrelated probes do not weaken or fail its orphan-probe gate.
- `--gate completion` (the default) additionally requires every vendor
  function in the selected inventory to have a matching Rust probe.

The explicit floor is mandatory for the regression gate so the total amount
of established evidence cannot silently decrease. The current ESP32-S31
inventory has 466 source-qualified functions; 104 have evidence. Of the
remaining 362, two are implemented architectural roots that still need
semantic contracts and 360 are classified `not-yet-ported`.

## Results and evidence classes

For ROM, `verify` and `verify-all` map `phy_NAME` to
`open_phy_trace_NAME`. Archive symbols use their full name, so archive
`phy_NAME` maps to `open_phy_trace_phy_NAME`; this keeps identically named ROM
and archive functions distinct. A function with an
observable return uses `open_phy_trace_ret_NAME`; the verifier then compares
the symbolic RISC-V `a0` result in addition to MMIO. Its per-function outcomes
are:

- `MATCH`: the selected comparison method completed and agreed;
- `MISMATCH`: both traces are complete but differ;
- `INCOMPLETE`: a present pair cannot yet be proved;
- `UNCOVERED`: no Rust comparison probe exists.

Each `MATCH` row reports `evidence=symbolic`, `evidence=effect-contract`,
`evidence=scenario`, `evidence=state`, or
`evidence=composition-state-scenario`. Symbolic equality proves the normalized
straight-line trace. Effect-contract equality additionally proves that every
effect has an explicit closed policy. Scenario equality proves only the
explicitly declared inputs plus complete branch-outcome coverage. State evidence
additionally compares the declared canonical pre/post projection without
depending on Rust object layout. Composition-state-scenario evidence compares
normalized root actions and final state for a declared transition matrix
without claiming independent proof of all transitive children. None of the
concrete contracts claims exhaustive equality over an undeclared input domain.
`--json-report PATH` writes a versioned machine-readable `verify-all` result
with the summary, evidence identities and SHA-256 of every input artifact and
policy file.

When a paired function cannot be closed by the straight-line symbolic engine,
`verify` uses its named concrete profile. It promotes the result to `MATCH`
only when every case matches and both ELF branch inventories have no uncovered
outcome or unresolved edge. The final `SUMMARY` therefore combines both proof
methods while keeping their evidence counters separate; it does not leave a
profile-confirmed function as `INCOMPLETE`.

The engine fails closed on control flow, calls and tail jumps, unresolved MMIO
write values, and MMIO registers absent from the SVD. Every such site is
printed as an `UNCOVERED` row followed by aggregate `SUMMARY` counters. The
current direct engine does not claim path coverage for loops, input-dependent
branches, indirect calls or table-derived addresses.

Function-by-function behavior descriptions do not belong in `docs/phy` once
the compiled comparison proves them. The executable scenarios, coverage
summary and reported input identity are the audit record; documentation is
reserved for tool operation and exceptional rules that cannot be encoded in
the verifier.

## Trust boundary and development checks

`verify` analyzes exactly the paths supplied by the caller. Artifact revision
and authenticity checks belong in the invoking CI job or local harness. A new
chip or ROM revision therefore adds a target/harness pack and an evidence
baseline, not a digest constant in the validator.

`extract` and `compare` remain available for focused investigation of one
symbol. Run the command without arguments for their complete syntax.

`cargo test --workspace --locked` does not require the ignored private oracle
directory. Decoder, memory and policy behavior use synthetic fixtures; the two
inventory-count checks report a skip when the private ROM/archive paths are
not supplied through the documented environment variables. The explicit
qualification commands above remain the required private-oracle integration
checks. The repository CI runs formatting,
workspace tests, strict validator-only clippy, PAC generation checks and the
source-only audit. A separate
`Private oracle regression` workflow runs only on protected `main` or manual
dispatch using a dedicated self-hosted runner and approved
`oracle-regression` environment; it uploads both text and JSON reports and
never executes pull-request code with proprietary oracle access.

No parity exceptions are currently accepted. A future exception belongs in
the verifier as a typed rule with exact artifact, symbol and behavior scope,
plus tests; it must never turn an unrelated incomplete trace into `MATCH`.
