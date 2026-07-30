# ESP32-S31 vendor PHY defects and fragile contracts

This page records defects found in the pinned vendor oracle itself. It is
separate from [behaviour parity](behavior-parity.md): an unsafe or blocking
vendor behaviour is not automatically a Rust defect, and deliberately refusing
to reproduce it is not a parity regression on the successful hardware path.

Audit baseline: 2026-07-30. Addresses below refer to
`_oracles/esp32s31_rev0_rom.elf`.

## Classification

- **Confirmed defect**: the complete instruction body proves an invalid memory
  access or an outcome that violates the function's own data geometry.
- **Robustness defect**: normal hardware can complete the function, but a stuck
  or unexpected hardware state can block the CPU indefinitely or be converted
  into apparent success.
- **Fragile contract**: the code is safe only under an external lifecycle or
  ownership invariant that the function does not enforce. This is not called a
  confirmed defect unless a reachable invariant violation is demonstrated.

## VENDOR-DEFECT-001: invalid temperature DAC indexes past the table

Classification: **confirmed out-of-bounds table read for an invalid DAC
encoding**.

The ROM data symbol `phy_tsens_attribute` at `0x2f84d9ec` has size `0x1e`:
exactly five six-byte records. `phy_tsens_dac_to_index` at `0x2f825e2e`
maps the five valid DAC encodings as follows:

| DAC encoding | Index |
| ---: | ---: |
| 5 | 0 |
| 7 | 1 |
| 15 | 2 |
| 11 | 3 |
| 10 | 4 |
| any other value | 5 |

`phy_tsens_temp_read_local` at `0x2f825f1e` masks the sampled DAC to four
bits, calls that mapper, and reads from
`phy_tsens_attribute + 6 * index`. It also passes the same index to
`phy_tsens_dac_cal`, which reads the remaining fields of that six-byte record.
Index 5 therefore starts at `0x2f84da0a`, exactly one byte past the table.

The bytes consumed as the nonexistent sixth record are adjacent `.rodata`,
including the start of a diagnostic string, rather than a sixth temperature
record. The result can be a nonsensical temperature/range decision. It will
not necessarily trap because the adjacent ROM remains readable.

Vendor-controlled initialization may normally establish one of the five valid
DAC encodings before this path. That lifecycle precondition reduces normal-path
reachability but does not remove the out-of-bounds behaviour for reset,
corrupted or unexpected analog state.

The Rust implementation intentionally does not reproduce this defect: reset
DAC zero is initialized to DAC 5, and other invalid encodings return
`InvalidDac`.

## VENDOR-ROBUSTNESS-001: several hardware waits have no deadline

Classification: **confirmed hard-hang paths when a peripheral never reaches
the expected state**.

The following complete ROM bodies contain polling cycles with no retry bound,
counter comparison or error return:

| Function | Address | Unbounded condition |
| --- | ---: | --- |
| `phy_wait_freq_set_busy` | `0x2f824fc6` | waits for bit 8 at `0x20100028` to become set |
| `phy_chip_i2c_readReg_org` | `0x2f829ffa` | waits for PHY-I2C host busy bit 25 to clear after publication |
| `phy_chip_i2c_writeReg` | `0x2f82a30e` | waits for busy bit 25 both before and after publication |
| `phy_pbus_force_test` | `0x2f824228` | waits for the PBus status word's sign/busy bit to clear |
| `phy_pwdet_wait_idle` | `0x2f82664c` | waits for the detector state field to equal 7 |
| `phy_iq_est_enable` | `0x2f8289d4` | waits for estimator completion; its 16-bit diagnostic counter is never used as a limit |
| `phy_txdc_cal` | `0x2f82abbe` | waits at `0x2f82ac80` for the TXDC result-ready bit |

These functions are valid on responsive hardware, but a bus fault, lost clock,
bad reset state or analog block failure can retain the CPU forever. The
`phy_iq_est_enable` counter can wrap and continue; it is telemetry, not a
timeout.

Rust represents these waits as executor-owned readiness samples with finite
deadlines and typed failure. Successful hardware ordering is intended to
remain equivalent; failure semantics deliberately differ.

## VENDOR-ROBUSTNESS-002: bounded waits discard the failure result

Classification: **confirmed failure-propagation defect**.

`phy_wait_rfpll_cal_end` at `0x2f825874` performs at most 100 iterations with
a 20 microsecond delay. If lock never arrives, it prints a diagnostic on the
last iteration and returns through the same void path as success. Its caller
cannot distinguish a locked PLL from a roughly 2 ms timeout and can continue
calibration with an unlocked state.

`phy_wait_i2c_sdm_stable` at `0x2f823e76` similarly returns through one void
path when either the sampled value becomes `0x5b` or the wrapping counter delta
exceeds `0x270f`. In addition, each iteration calls the unbounded
`phy_i2c_readReg`, so a stuck I2C host prevents the outer counter deadline from
being observed.

This is distinct from the unbounded loops above: the outer algorithm has a
nominal bound, but it does not expose whether that bound expired.

## VENDOR-CONTRACT-001: I2C serialization hooks are no-ops

Classification: **fragile ownership contract; no concurrent failure has yet
been demonstrated in the qualified boot graph**.

Both `phy_i2c_enter_critical` and `phy_i2c_exit_critical` in
`libphy.a[phy_i2c.o]` are single-instruction `ret` stubs. Their corresponding
ROM defaults at `0x2f829f18` and `0x2f829f1a` are also `ret`.

This matters because `phy_chip_i2c_readReg_org` publishes its command without
first checking whether that host is already busy. A concurrent or stale
in-flight command can therefore be overwritten. `phy_chip_i2c_writeReg` is
different: it does wait for the host to become idle before publication,
although that wait itself has no deadline.

The vendor stack may guarantee single ownership elsewhere, in which case the
no-op hooks and read publication are valid under that contract. They should
not be treated as thread-safe primitives, and future AP/STA, BT/BLE and
coexistence work must not assume that these symbols provide mutual exclusion.

## What was not established

No incorrect register value or ordering was found in the qualified successful
Wi-Fi cold-init profile merely because the vendor code is written in C or
assembly. The strongest current vendor findings concern invalid-state table
handling, failure containment and hidden ownership assumptions.

In particular, `phy_rfpll_cap_init_cal` is bounded in the inspected ROM body;
it is not evidence of an infinite capacitor-search loop. Vendor bugs should be
added here only after the complete body, its data geometry and relevant caller
contract have been checked.
