# ESP32-S31 validation harness

This directory owns target-specific input for compiled vendor-to-Rust
validation. It is deliberately outside the generic validator tool.

- `target.spec` selects the RISC-V 32-bit backend, ILP32 calling convention,
  ESP32-S31 PHY harness and Rust recompilation target.
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

The protected `oracle-regression` GitHub environment must provide
`ESP32S31_ROM_SHA256` and `ESP32S31_LIBPHY_SHA256` as Actions configuration
variables. The workflow checks them before building or invoking the validator.
Those values are caller policy and deliberately do not live in this target
pack or the validator binary.

ABI versions, callback tables and lifecycle entry contracts are compiled from
the dedicated `tools/vendor-code-validator/crates/harness-esp32s31` fixture
crate. Typed semantic contracts live in the generic semantic crate and the
ESP32-S31 qualification adapters live in the target semantic harness; the CLI
facade only selects and runs them. See
[`docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md`](../../docs/VENDOR_CODE_VALIDATOR_ARCHITECTURE.md).

## libpp interrupt pilot

The first Wi-Fi vertical slice qualifies two generated PAC leaves and their
composition in the production MAC IRQ path. Build the caller-owned linked view
and the Rust probes first:

```console
OPEN_RADIO_LINKED_ORACLE_SPEC="$PWD/hil/vendor-oracle/esp32s31/trace-elf/linked-oracle-libpp.spec" \
cargo build --manifest-path hil/vendor-oracle/esp32s31/Cargo.toml \
  -p open-esp-radio-vendor-oracle-esp32s31-trace-elf \
  --target riscv32imafc-unknown-none-elf --release

cargo build --manifest-path hil/esp32s31/Cargo.toml \
  -p open-esp-radio-hil-esp32s31-trace-probes-elf \
  --target riscv32imafc-unknown-none-elf --release
```

Then run the focused regression gate. The caller supplies and authenticates
all three vendor inputs; no artifact path or hash is embedded in the tool:

```console
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-interrupt.disposition \
  --no-profiles --gate regression --match-floor 3 \
  --evidence-baseline validation/esp32s31/baselines/libpp-interrupt.evidence
```

The dedicated Rust prefix is part of the focused gate boundary. The same ELF
also contains PHY probes, but they are neither candidates nor orphans in this
libpp run. The expected result is three exact effect-contract matches, no
mismatch/incomplete/orphan row, and a passing evidence baseline.

## WDEVPWR interrupt boundary

The power-interrupt gate qualifies only the masked STATUS read and exact CLEAR
write. Production carries the acknowledged image into a separate Embassy
signal without decoding unqualified cause bits. HIL keeps the complete
WDEVPWR enable mask at zero, so this boundary is ready for later power policy
but does not enable modem sleep.

```console
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_irq_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-power-interrupt.disposition \
  --no-profiles --gate regression --match-floor 2 \
  --evidence-baseline validation/esp32s31/baselines/libpp-power-interrupt.evidence
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
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-modem-wakeup.disposition \
  --no-profiles --gate regression --match-floor 10 \
  --evidence-baseline validation/esp32s31/baselines/libpp-modem-wakeup.evidence
```

The expected result is ten exact-effect matches and a passing evidence
baseline. This is not a whole-function equivalence claim for vendor
`pm_sleep`: RF/PHY and clock gating, wake restoration, TIM/DTIM policy and
qualified interrupt-cause decoding remain separate required slices.

The adjacent two-register station TSF wake transaction is qualified
separately because it has a closed bool input domain and a non-symmetric
disable branch: both branches set bit 21 at `0x2010_d830`, while only bit 29 at
`0x2010_d858` follows the argument.

```console
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_power_tsf_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-sta-tsf-wakeup.disposition \
  --profiles validation/esp32s31/profiles/libpp-sta-tsf-wakeup.profile \
  --gate regression --match-floor 1 \
  --evidence-baseline validation/esp32s31/baselines/libpp-sta-tsf-wakeup.evidence
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
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:rom "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --source-prefix:rom hal_get_sta_tsf \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_rom_power_tsf_trace_ \
  --dispositions validation/esp32s31/dispositions/rom-sta-tsf-snapshot.disposition \
  --profiles validation/esp32s31/profiles/rom-sta-tsf-snapshot.profile \
  --gate regression --match-floor 1 \
  --evidence-baseline validation/esp32s31/baselines/rom-sta-tsf-snapshot.evidence
```

The expected result is four matching cases, complete coverage of both pointer
branches, `match=1`, and no mismatch, incomplete or orphan probe.

## Ordinary TX/DMA register slice

The next focused gate covers seven production operations: CCA publication,
trigger-flow sampling, finite enable/valid/invalid/disable queue access, and
the final TX queue doorbell. The four indexed profiles declare `arg-range 0 0
3`; all four logical queues must be executed, and the validator proves the
reversed `CONTROL[3-queue]` mapping without treating the out-of-domain
assertion as an admissible vendor input.

`hal_mac_txq_enable` is intentionally not labeled whole-function equivalent.
The vendor root first performs the exact CONTROL read/write, then changes its
private queue context, has an HE trigger-based branch, and updates vendor
statistics. The checked adapter therefore qualifies the register prefix,
requires `embassy-tx-queue-ownership`, records
`he-trigger-based-tx-disabled` as a current prerequisite, and allows only the
statistics suffix to be omitted as unused instrumentation.

```console
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_tx_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-tx-dma.disposition \
  --profiles validation/esp32s31/profiles/libpp-tx-dma.profile \
  --gate regression --match-floor 7 \
  --evidence-baseline validation/esp32s31/baselines/libpp-tx-dma.evidence
```

The expected focused result is `match=7`, `mismatch=0`, `incomplete=0`,
`orphan-rust-probe=0`, and a passing evidence baseline. The production
`start_prepared_mac_tx` calls the same qualified safe PAC transaction between
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
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libpp "$ESP32S31_LIBPP_LINKED_ELF" \
  --source-inventory:libpp "$OPEN_ESP_RADIO_ESP32S31_LIBPP_ARCHIVE" \
  --source-companion:libpp "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libpp_rx_trace_ \
  --dispositions validation/esp32s31/dispositions/libpp-rx-dma.disposition \
  --no-profiles --gate regression --match-floor 9 \
  --evidence-baseline validation/esp32s31/baselines/libpp-rx-dma.evidence
```

The expected result is eight exact PAC-leaf matches plus one compiled
composition match, with no mismatch/incomplete/orphan row. The
`wDev_AppendRxBlocks` adapter deliberately qualifies an architectural
replacement rather than C-layout identity. It pins the vendor chain guard,
old-tail publication, leaf-call order and exact `0x186a1` reload bound, then
executes the production Rust descriptor/staging owner for immediate settle,
two one-microsecond Embassy edges, terminal-frontier base repair, and the full
100,001-sample timeout. Every scenario is repeated with two private-stack
padding fills and must retain identical MMIO, delay and return observables.
Vendor `wDevCtrl`, `g_osi` locking, linked-list diagnostics and optional
statistics are not imported into the runtime.

## Infrastructure-STA Authentication/Association slice

`ieee80211_sta_new_state` is deliberately qualified as an architectural
replacement, not as whole-function or private-layout equivalence. The vendor
root combines ordinary station management with NVS/configuration reads,
`g_osi` timers and locks, diagnostics, power/coexistence, mesh branches and
private interface/node state. The open implementation instead owns typed
Authentication/Association protocol state, accepts station configuration from
its caller, and exposes the deadline to an Embassy executor.

```console
cargo vendor-code-validator verify inventory \
  --target-spec validation/esp32s31/target.spec \
  --source-artifact:libnet80211 "$ESP32S31_LIBNET80211_LINKED_ELF" \
  --source-inventory:libnet80211 "$OPEN_ESP_RADIO_ESP32S31_LIBNET80211_ARCHIVE" \
  --source-companion:libnet80211 "$OPEN_ESP_RADIO_ESP32S31_ROM_ELF" \
  --rust-artifact "$ESP32S31_RUST_TRACE_PROBES_ELF" \
  --rust-prefix open_libnet80211_trace_ \
  --dispositions validation/esp32s31/dispositions/libnet80211-sta-join.disposition \
  --no-profiles --gate regression --match-floor 1 \
  --evidence-baseline validation/esp32s31/baselines/libnet80211-sta-join.evidence
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
power-state behavior are explicitly outside this qualification boundary.
