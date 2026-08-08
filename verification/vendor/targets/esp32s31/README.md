# ESP32-S31 vendor verification target

`vendor-project.toml` is the preferred project entry point. It composes the
existing target pack with `memory.toml`, whose MMIO regions are independent of
SVD register names:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run \
  --json-report /tmp/esp32s31-mmio.json
```

The checked project deliberately omits a run spec because vendor artifact
paths are caller-owned. `--target-spec` examples below remain valid as direct,
single-command invocations.

The public project configuration can be checked without proprietary inputs:

```console
cd verification/vendor/targets/esp32s31
cargo vendor-binary-workbench project doctor

cargo vendor-binary-workbench project status \
  --project vendor-project.toml
```

The missing run spec is a readiness warning rather than a configuration error.
The status report therefore shows ready configuration, verification and
publication phases and incomplete private-input/analysis phases until a local
run spec is supplied.

With a private run spec, the complete generated-evidence workflow is:

```console
cargo vendor-binary-workbench project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run

cargo vendor-binary-workbench project analyze --check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

This generates or checks the complete symbol inventory, MMIO/interface facts,
both linked-IR profiles, and the register/function reviews, then validates the
reviewed register, interface, and function files.
It deliberately does not update `svd/esp32s31-radio.svd` or production PAC
code. The public register release gate needs no private run spec:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check
```

This strictly validates the model, API, lint and evidence packs, then verifies
the configured SVD, PAC and binding index as one preflighted publication.

## Register project

The checked `registers/device.toml` and its peripheral fragments are the
workbench's editable ESP32-S31 radio register model. The workbench loads this
model directly; generated XML is not required before MMIO discovery, IR
export, or verification. The separate
`../../../../svd/esp32s31-platform-radio-deps.svd` project input contributes
official platform registers used by the radio call graph without transferring
their runtime ownership to this project.

Inspect the model and generated review with:

```console
cargo vendor-binary-workbench registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The clean SVD is written to `svd/esp32s31-radio.svd`. Discovery evidence
remains in the ignored `generated/findings/mmio.json`. `registers review`
writes the ignored `generated/reports/register-review.md`, joining addresses
to read/write functions, write masks and current model identities and emitting
copyable drafts for gaps. Users edit reviewed names, fields, access rules,
reset values and enumerations only in `registers/peripherals/*.toml`; the
generated report never feeds SVD or PAC generation.

The checked project defines separate `rom-phy` and `archive-phy` linked-IR
profiles. Each primary input receives the other linked ELF as its reviewed
companion through `run.spec`, then register review merges both reports:

```console
cargo vendor-binary-workbench ir build \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run

cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

This adds poll and predicate field candidates plus links to guarded RTOS,
delay, NVS and logging operations. Those operation names remain navigation
evidence and are not promoted to SVD semantics. Use `ir build --check` to
verify both generated views, or `registers review --no-ir-reports` when only
the base MMIO-discovery report is wanted.

Private artifact paths remain in the local run spec. The generic profile
format and companion rules are documented in
[`project-ir-build.md`](../../../../tools/vendor-binary-workbench/docs/project-ir-build.md).

`registers/api.toml` owns the reviewed ESP32-S31 safe compound transactions,
ownership split and device-access helper. `project publish` is the normal
production gate. The PAC can still be checked directly when diagnosing that
single stage:

```console
cargo vendor-binary-workbench registers generate-pac \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check --deny-unreviewed
```

The pack is cross-validated against the clean schema-2 register model and
produces the checked-in PAC byte-for-byte. Use `--no-api-pack --output PATH`
when a plain architecture-neutral svd2rust output is useful for inspection.
Reviewed provenance, confidence vocabulary and coarse dump ranges now live in
the functional catalogs under `registers/evidence/`. Validation resolves every
source used by the model and API pack, and checks evidence ranges plus all
modeled registers against `memory.toml`. `registers/lints.toml` retains the
ESP32-S31 policy against synthetic `PRESERVED` fields without imposing that
naming rule on generic projects. The retired generator migration and current
publication ownership are recorded in
[`pac-gen-migration.md`](../../../../tools/vendor-binary-workbench/docs/pac-gen-migration.md).

The project also owns the neutral PAC address/path index. Diagnose that stage
independently of the production PAC with:

```console
cargo vendor-binary-workbench registers generate-bindings \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --check --deny-unreviewed
```

This produces `svd/esp32s31-radio.bindings` from the same schema-2 model and
records the Rust PAC crate name used by `driver generate`.

The project also configures the generic interface workspace. Generate facts
from a caller-owned run spec, initialize the reviewed pack once, and validate
it after edits or vendor updates:

```console
cargo vendor-binary-workbench interfaces discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run

cargo vendor-binary-workbench interfaces init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench interfaces validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Generated facts are ignored because they expose local paths and artifact
digests. The reviewed `interfaces/reviewed.toml` is intended to become a
shareable project asset after manual review. Reusable RTOS, NVS, logging, and
delay operations come from the tool's semantic catalog; the project pack owns
only ESP32-S31 anchors, layout versions, and slot ABI. Validation also retains
the generic discovery evidence for each concrete call site and recovered
argument expression; those facts do not make the reviewed semantic a runtime
execution claim.

The same project configures a reviewed function/context workspace over both IR
profiles. Generate IR, initialize the pack once, then edit names and roles in
`functions/reviewed.toml` and regenerate the reading view:

```console
cargo vendor-binary-workbench functions init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench functions validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench functions review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The ignored `generated/reports/function-review.md` puts reviewed roles and
context field names beside pseudo-code, exact validated interface call sites,
recovered call arguments, exact linked-IR CFG guards when available,
RTOS/NVS/logging/delay links, trampoline counts, and closure blockers. It is
not source reconstruction and
does not feed the register SVD. Register names remain in `registers/`, and
external table ABI/semantics remain in `interfaces/` plus the reusable catalog.

This directory owns target-specific input for compiled vendor-to-Rust
verification. It is deliberately outside the generic verification engine.

- `target.spec` selects the generic RISC-V 32-bit backend, ILP32 calling
  convention and Rust recompilation target.
- `platform.toml` is the project-mode platform pack: it composes the
  ESP32-S31 radio harness with reusable RTOS/NVS/logging/delay vocabulary.
- `interfaces/reviewed.toml` alone binds concrete observed table slots to that
  vocabulary; the platform pack does not identify vendor layouts.
- `run.spec.example` documents the separate caller-owned artifact bindings.
- `profiles/` contains concrete compiled-equivalence scenarios.
- `dispositions/` maps vendor inventory symbols to Rust components and
  executable contracts.
- `baselines/` contains expected evidence classifications.

No file here selects a proprietary artifact path or authenticates one. The
caller validates the desired vendor revision and passes absolute paths at run
time, either as command options or through an untracked copy of
`run.spec.example` passed with `--run-spec`. Private integration tests
recognize these explicit variables:

- `OPEN_ESP_RADIO_ESP32S31_ROM_ELF`
- `OPEN_ESP_RADIO_ESP32S31_LIBPHY_ARCHIVE`
- `OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE`

Legacy executable ABI fixtures and lifecycle entry contracts remain compiled
in the ESP32-S31 semantic harness. New callback-table discovery and review use
the project interface pack so names and layouts do not enter the generic
backend. Typed executable contracts live in the generic semantic crate and
ESP32-S31 verification adapters live in the target semantic harness. See
[`docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md`](../../../../docs/VENDOR_BINARY_WORKBENCH_ARCHITECTURE.md).

## libpp interrupt pilot

The first Wi-Fi vertical slice verifies two generated PAC leaves and their
composition in the production MAC IRQ path. Build the caller-owned linked view
and the Rust probes first:

```console
OPEN_RADIO_LINKED_ORACLE_SPEC="$PWD/verification/vendor/targets/esp32s31/oracle-firmware/trace-elf/linked-oracle-libpp.spec" \
cargo build --manifest-path verification/vendor/targets/esp32s31/oracle-firmware/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release

CARGO_TARGET_DIR="$PWD/target/verification/esp32s31-probes" \
cargo build --manifest-path verification/vendor/targets/esp32s31/probes/Cargo.toml \
  -p open-esp-radio-verification-esp32s31-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

Then run the focused regression gate. The caller supplies and authenticates
all three vendor inputs; no artifact path or hash is embedded in the tool:

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-interrupt.disposition \
  --no-profiles --gate regression --match-floor 3 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-interrupt.evidence
```

The dedicated Rust prefix is part of the focused gate boundary. The same ELF
also contains PHY probes, but they are neither candidates nor orphans in this
libpp run. The expected result is three exact effect-contract matches, no
mismatch/incomplete/orphan row, and a passing evidence baseline.

## WDEVPWR interrupt boundary

The power-interrupt gate verifies only the masked STATUS read and exact CLEAR
write. Production carries the acknowledged image into a separate Embassy
signal without decoding unverified cause bits. HIL keeps the complete
WDEVPWR enable mask at zero, so this boundary is ready for later power policy
but does not enable modem sleep.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_irq_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-power-interrupt.disposition \
  --no-profiles --gate regression --match-floor 2 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-power-interrupt.evidence
```

The expected result is two exact-effect matches, no
mismatch/incomplete/orphan row, and a passing evidence baseline. Cause-bit
meaning, interrupt enable policy and the resulting RF/PHY/clock transition are
explicitly outside this gate.

## Connected modem wake counters

Ten safe typed PAC transactions cover the finite register sequence selected
by the connected vendor PM path: beacon-miss timeout/limit, both counter wake
gates, modem-state sleep limit, wake protection lead time, and optional TBTT
auto-period enable/disable/interval. `StaModemWakeConfig` bounds every field
before MMIO, and `RadioRegisters::configure_station_modem_wakeup` composes the
same operations in vendor order without importing vendor PM context.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-modem-wakeup.disposition \
  --no-profiles --gate regression --match-floor 10 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-modem-wakeup.evidence
```

The expected result is ten exact-effect matches and a passing evidence
baseline. This is not a whole-function equivalence claim for vendor
`pm_sleep`: RF/PHY and clock gating, wake restoration, TIM/DTIM policy and
verified interrupt-cause decoding remain separate required slices.

The adjacent two-register station TSF wake transaction is verified
separately because it has a closed bool input domain and a non-symmetric
disable branch: both branches set bit 21 at `0x2010_d830`, while only bit 29 at
`0x2010_d858` follows the argument.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_tsf_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-sta-tsf-wakeup.disposition \
  --profiles verification/vendor/targets/esp32s31/profiles/libpp-sta-tsf-wakeup.profile \
  --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-sta-tsf-wakeup.evidence
```

Both profile cases must have four ordered MMIO events and complete branch
coverage. The gate previously rejected an extra Rust `fence`; the production
method now retains exactly the vendor-observed transaction.

The planner also needs a live STA-TSF sample to reject a wake target that has
already passed while RX/control work was queued. The focused ROM gate closes
all four optional-output-pointer combinations of `hal_get_sta_tsf`; the
production `RadioRegisters::station_tsf` specializes the same safe PAC
register transaction to both output words.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "rom=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --source-prefix rom=hal_get_sta_tsf \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_rom_power_tsf_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/rom-sta-tsf-snapshot.disposition \
  --profiles verification/vendor/targets/esp32s31/profiles/rom-sta-tsf-snapshot.profile \
  --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/rom-sta-tsf-snapshot.evidence
```

The expected result is four matching cases, complete coverage of both pointer
branches, `match=1`, and no mismatch, incomplete or orphan probe.

## Ordinary TX/DMA register slice

The next focused gate covers seven production operations: CCA publication,
trigger-flow sampling, finite enable/valid/invalid/disable queue access, and
the final TX queue doorbell. The four indexed profiles declare `arg-range 0 0
3`; all four logical queues must be executed, and the verifier proves the
reversed `CONTROL[3-queue]` mapping without treating the out-of-domain
assertion as an admissible vendor input.

`hal_mac_txq_enable` is intentionally not labeled whole-function equivalent.
The vendor root first performs the exact CONTROL read/write, then changes its
private queue context, has an HE trigger-based branch, and updates vendor
statistics. The checked adapter therefore verifies the register prefix,
requires `embassy-tx-queue-ownership`, records
`he-trigger-based-tx-disabled` as a current prerequisite, and allows only the
statistics suffix to be omitted as unused instrumentation.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_tx_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-tx-dma.disposition \
  --profiles verification/vendor/targets/esp32s31/profiles/libpp-tx-dma.profile \
  --gate regression --match-floor 7 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-tx-dma.evidence
```

The expected focused result is `match=7`, `mismatch=0`, `incomplete=0`,
`orphan-rust-probe=0`, and a passing evidence baseline. The production
`start_prepared_mac_tx` calls the same verified safe PAC transaction between
its two device fences; vendor context layout and statistics are absent from
runtime code.

## RX descriptor-walker register slice

The RX gate covers eight finite leaves used by the production ring owner:
walker enable/disable, raw last/next reads, base publication, complete
last-pointer reconstruction, and reload-bit read/set. Safe typed PAC
transactions implement the same Effect Contracts exercised by the probes;
the handwritten `RxRingStopped`/`RxRingLive` types retain lifecycle and
descriptor memory ownership.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libpp=$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory "libpp=$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion "libpp=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_rx_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libpp-rx-dma.disposition \
  --no-profiles --gate regression --match-floor 9 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libpp-rx-dma.evidence
```

The expected result is eight exact PAC-leaf matches plus one compiled
composition match, with no mismatch/incomplete/orphan row. The
`wDev_AppendRxBlocks` adapter deliberately verifies an architectural
replacement rather than C-layout identity. It pins the vendor chain guard,
old-tail publication, leaf-call order and exact `0x186a1` reload bound, then
executes the production Rust descriptor/staging owner for immediate settle,
two one-microsecond Embassy edges, terminal-frontier base repair, and the full
100,001-sample timeout. Every scenario is repeated with two private-stack
padding fills and must retain identical MMIO, delay and return observables.
Vendor `wDevCtrl`, `g_osi` locking, linked-list diagnostics and optional
statistics are not imported into the runtime.

## Infrastructure-STA Authentication/Association slice

`ieee80211_sta_new_state` is deliberately verified as an architectural
replacement, not as whole-function or private-layout equivalence. The vendor
root combines ordinary station management with NVS/configuration reads,
`g_osi` timers and locks, diagnostics, power/coexistence, mesh branches and
private interface/node state. The open implementation instead owns typed
Authentication/Association protocol state, accepts station configuration from
its caller, and exposes the deadline to an Embassy executor.

```console
cargo vendor-binary-workbench verify inventory \
  --target-spec verification/vendor/targets/esp32s31/target.spec \
  --source-artifact "libnet80211=$ESP32S31_LIBNET80211_LINKED_ELF" \
  --source-inventory "libnet80211=$OPEN_ESP_RADIO_ESP32S31_LIBNET80211_ARCHIVE" \
  --source-companion "libnet80211=$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libnet80211_trace_ \
  --dispositions verification/vendor/targets/esp32s31/dispositions/libnet80211-sta-join.disposition \
  --no-profiles --gate regression --match-floor 1 \
  --evidence-baseline verification/vendor/targets/esp32s31/baselines/libnet80211-sta-join.evidence
```

`ESP32S31_LIBNET80211_LINKED_ELF` is a caller-built linked view of the same
authenticated archive, analogous to the libpp linked view above. The raw
archive remains the authoritative symbol inventory; an archive path is not a
substitute for the executable input.

The gate pins the vendor Authentication and Association management-send
branches, their timeout callbacks and the exact 1,000-ms state deadline. It
then executes the production `StaJoinRunner` with a finite PAC/DMA test adapter
and monotonic clock in four compiled scenarios: first-attempt Open
Authentication success, the Authentication attempt limit, Association
success, and Association retries through the exact deadline. Each scenario is
repeated with two private-stack fills and must produce the same result without
MMIO or blocking-delay effects. RX is serviced before timeout at an equal
deadline; successful Association transfers the still-live ring to the WPA2
phase instead of silently stopping and recreating DMA ownership.

The three-attempt Authentication limit and 160-ms Association retransmission
cadence are currently source-owned open-driver policies; the inspected vendor
root does not establish them. Only the 1,000-ms state deadline is claimed as a
vendor-anchored timing invariant. NVS, logging, RTOS synchronization, mesh and
power-state behavior are explicitly outside this verification boundary.
