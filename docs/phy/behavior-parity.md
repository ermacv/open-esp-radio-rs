# ESP32-S31 PHY behaviour parity

## Executive result

The current Rust implementation is close to the vendor implementation for the
successful, default Wi-Fi cold-init profile, but it is not globally
behaviour-equivalent.

This page is the earlier profile comparison. It is not the complete
instruction-by-instruction audit. Global coverage and the stricter register
trace criterion are maintained in
[function-audit-ledger.md](function-audit-ledger.md) and
[audit-method.md](audit-method.md).

The audited default profile is:

- full calibration on a new parameter image;
- 40 MHz crystal;
- channel 11 unless the open-only channel constructor is used;
- CBW selector zero;
- frequency offset zero;
- channels 1 through 13;
- channel-14 MIC feature disabled;
- 802.11p feature disabled;
- uncontended PHY-I2C and eventually-ready hardware.

Within that profile, the restored arithmetic, tables, finite MMIO operations
and ordering are well covered by source comments and 289 passing PAC/HAL/PHY
unit tests. One unconditional vendor child is still absent:
`phy_bt_tx_gain_init`.

## Primary parent comparison

| Vendor root | Vendor behaviour | Rust owner | Assessment |
| --- | --- | --- | --- |
| `register_chipv7_phy` (`phy_init.o`, `0x1e6`) | init profile, calibration-record mode selection, RF init, BB init, temperature/tail and release | `PhyRegisterTransition` | **Partial**. Default full-calibration radio path is represented; validation/recovery/backup modes are not composed |
| `phy_rf_init` (`phy_init.o`, `0x122`) | frontend/BB clocks, BBPLL/bias/I2C power, PBus, I2C/RC/SAR/RFPLL, XTAL duty, FE update, frequency table | `PhyRfColdInit` plus `PhyRfInitPrefixTransition` | **Profile-matched** for the cold full-calibration graph |
| `phy_bb_init` (`phy_init.o`, `0x16a`) | guarded Wi-Fi calibration, CFR, BT gain init, PBus memory, RX calibration/table, register/AGC update, channel 11 and tracking | `PhyBbInitTransition` | **Partial/divergent** because `phy_bt_tx_gain_init` is skipped |
| `phy_chip_set_chan` (`phy_rfpll.o`, `0x10e`) | channel/frequency normalization, AGC/BBPLL/temp, RF frequency, misc/TX gain, optional channel-14/11p work, callback, cleanup | `PhyChipChannelTransition` | **Profile-matched** only for channels 1–13 and disabled optional features |

## Ordered `phy_bb_init` comparison

For a fresh parameter image, the vendor parent calls:

1. `phy_txdc_cal_init`;
2. `phy_pwdet_code_cal`;
3. `phy_tx_cap_init`;
4. first `phy_tsens_temp_read`;
5. `phy_tx_pwctrl_init`;
6. `phy_txdc_cal_pwdet_init`;
7. `phy_dcode_cal_init`;
8. `phy_txiq_cal_init`;
9. `phy_set_tx_cfr_mem(32)`;
10. `phy_bt_tx_gain_init`;
11. `phy_set_pbus_mem`;
12. second `phy_tsens_temp_read`;
13. `phy_rxiq_cal_init`;
14. `phy_rx_table_init`;
15. RX saturation prepare/check;
16. `phy_set_rx_gain_table(0x985, 0)`;
17. RX saturation restore;
18. `phy_reg_init`;
19. `phy_bb_agc_reg_update`;
20. `phy_reg_update_new`;
21. `phy_enable_agc`;
22. `phy_chip_set_chan(11, 0)`;
23. idle-mode/optional Wi-Fi disable;
24. `phy_i2c_txrate_init`;
25. `phy_bb_txpwr_track(1)`.

The Rust parent retains this order except for item 10. Diagnostic-only print
branches with fixed debug argument zero are not represented.

## Findings

### PHY-PARITY-001: unconditional BT gain child is absent

Severity: **high for future BT/BLE/coexistence; transaction mismatch today**.

Vendor `phy_bb_init` unconditionally calls `phy_bt_tx_gain_init` after CFR
publication and before PBus-memory setup. Rust moves directly from CFR to
PBus memory. The Rust test
`complete_parent_skips_only_the_bt_coexistence_child` explicitly fixes this
omission, so it is known behaviour rather than an accidental untested gap.

The complete 90-byte archive root is not merely a gain-table write. Its ordered
graph is:

1. `phy_set_channel_rfpll_freq(0x985, phy_param[0x4f], 0)`, selecting the
   2437 MHz calibration frequency;
2. `phy_set_txcap_reg(phy_param + 0xdc, 6)`;
3. `phy_bt_txdc_cal_new()`, which conditionally calibrates three BT baseband
   gain points into `phy_param[0x104..0x11c]`;
4. `phy_bt_tx_pwctrl_init()`, which conditionally performs BT power-control
   calibration and updates the BT parameter fields;
5. `phy_txdc_cal_pwdet_init(1, 0, 1)`;
6. `phy_bt_set_tx_gain_new(0)`, which obtains the BT tables through the
   target callback and publishes 16 gain entries to gain-memory bank 1 using
   the calibrated data at `phy_param[0x104..0x11c]`.

Several generic RFPLL, TXDC, PWDET and gain-memory leaves already have Rust
owners, but the BT-specific parameter profile, conditional calibration flags,
table generation and ordered parent do not. Reusing the shared leaves alone
would therefore not close this finding.

The current Wi-Fi path can still work because later Wi-Fi gain publication is
separate, and hardware HIL has received frames. Nevertheless, the complete
register transaction stream and BT calibrated state differ from the vendor.
This is a blocker before claiming complete BT/BLE/coexistence PHY init.

### PHY-PARITY-002: calibration-record lifecycle modes are not composed

Severity: **medium; cold full-calibration profile unaffected**.

The vendor parent accepts calibration data/mode inputs, validates version,
identity and checksum, may recover a saved 508-byte parameter image, and may
back up a new calibration result. The Rust crate contains owned calibration
record/checksum transforms, but `PhyRegisterTransition` deliberately always
begins full Wi-Fi calibration and does not expose validation, recovery or
backup in the top-level graph.

Consequences are different startup latency and calibration reuse across boots.
The current implementation only matches the vendor’s forced/full-calibration
branch.

### PHY-PARITY-003: non-default channel branches are missing or rejected

Severity: **high for channel 14/802.11p; default channels 1–13 unaffected**.

Complete vendor `phy_chip_set_chan`:

- accepts channel 14;
- when `phy_param[0x26]` is nonzero, calls `phy_chan14_mic_cfg_new(channel ==
  14)`, including the disable call on other channels;
- when `phy_param[0x28]` is nonzero, calls `phy_11p_set` with
  `phy_param[0x29]`;
- accepts already-normalized frequencies through the same public root.

Rust rejects channel 14 and returns `Channel14MicEnabled` whenever the optional
MIC feature flag is set, even for channels 1–13. It has no 802.11p action in
this transition. This early failure does not reproduce the vendor branch and
should be treated as an actual coverage/correctness problem for those
profiles, not merely a different API.

### PHY-PARITY-004: bounded async failures replace vendor blocking

Severity: **intentional safety divergence**.

The vendor ROM busy-waits in PHY-I2C, PBus, estimator, TXDC and
frequency-ready paths. The individual RFPLL lock wait is bounded, but reports
expiry only by printing and returns through the same path as success. Rust
exposes delays and readiness samples to the executor and converts missed
deadlines or impossible searches into typed failures, usually followed by
explicit cleanup. The instruction-level vendor cases are recorded in
[vendor-defects.md](vendor-defects.md).

On hardware that becomes ready within the same bounds, the operation ordering
and final register state are intended to be scheduling-equivalent. On stuck
hardware, behaviour is deliberately different: Rust returns instead of
blocking forever. Any parity claim must state whether it covers the successful
path or failure semantics.

### PHY-PARITY-005: PHY-I2C pre-command failure semantics differ

Severity: **low under unique ownership; observable under contention**.

`try_start_read` rejects an already-busy host before publishing a new command.
The ROM read leaf publishes immediately and only polls after publication.
The ROM write leaf does wait before publication, whereas Rust samples once and
returns `Busy` to its executor owner instead of spinning. With the unique Rust
radio owner these differences should be unreachable on the successful path,
but under stale hardware state or an ownership violation the command stream or
failure result differs.

### PHY-PARITY-006: temperature invalid/reset handling differs

Severity: **low for the qualified cold profile; correctness boundary for
unexpected analog state**.

For the five valid DAC encodings, the conversion and range changes match the
ROM. Rust handles reset DAC zero by programming DAC 5 before sampling, based
on cold-start HIL evidence. Other invalid DAC values produce
`InvalidDac`. The ROM path can use an out-of-range default table index instead.
Rust therefore fails closed instead of preserving invalid-memory behaviour.
The vendor defect and exact table geometry are recorded in
[vendor-defects.md](vendor-defects.md).

### PHY-PARITY-007: crystal and initial-channel profiles are narrower

Severity: **medium for portability; current board/default constructor
unaffected**.

The top-level Rust prelude uses a fixed 40 MHz XTAL platform operation. Vendor
code contains 26/32/40 MHz parameter handling. In addition,
`with_default_profile_on_channel` can make the baseband parent initialize a
channel other than 11, while vendor `phy_bb_init` itself always calls
`phy_chip_set_chan(11, 0)`. The default Rust constructor uses channel 11 and
is the comparable path; the custom-channel constructor is an open-driver
extension.

The strict all-input audit additionally found that
`PhyChipChannelParameters::frequency_offset` is not consumed anywhere in the
Rust channel transition. Vendor `phy_chip_set_chan` always passes its signed
`phy_param[0x20]` value to `phy_set_channel_rfpll_freq`, and
`phy_chip_set_chan_offset` can update and immediately apply it. Nonzero
frequency-offset profiles are therefore an actual register-programming
mismatch, not only an unexposed convenience API.

### PHY-PARITY-008: frequency-table API is narrower than the vendor

Severity: **medium outside the default cold profile; exact register-data
mismatch for non-Boolean parameter values**.

The default parent calls `phy_get_rf_freq_init(85, 0)` and
`phy_freq_i2c_data_write(1)`, which are the values represented by Rust. The
vendor children themselves accept a general table count, signed frequency
offset and descriptor-memory enable input. Rust fixes those values to 85,
zero and enabled.

In addition, vendor `phy_freq_get_i2c_data` uses the complete byte at
`phy_param[0x1af]`: it shifts the byte left by two and ORs the truncated result
into the front-end descriptor. Rust converts every nonzero byte to `bool` and
can only OR bit 2. Values 0 and 1 match; a value such as 2 writes different
frequency-memory data. The default image is 1, but there is no vendor
instruction masking the global byte to one bit at the point of use.

The exact instruction trace is in
[the `phy_hw_freq.o` audit](audit/libphy-phy_hw_freq.md). These narrower input
domains are not vendor-defect exceptions.

### PHY-PARITY-009: runtime/wakeup/tracking PHY is not ported

Severity: **high for a complete radio lifecycle; not a cold-init regression**.

`phy_wakeup_init`, close/power-down roots, background parameter and TX-power
tracking, and most BT/BLE-specific parents are absent. Some shared leaves exist
because cold Wi-Fi reaches them, but no complete Rust lifecycle owner
corresponds to these vendor roots.

### PHY-PARITY-010: diagnostic output is intentionally absent

Severity: **no radio-state impact for audited fixed-debug branches**.

Several translated functions omit `ets_printf`/debug formatting when the
audited parent passes debug zero or when the branch has no hardware or
parameter-state effect. This differs at the logging ABI but not at the
qualified radio-state boundary. Debug entry points such as `phy_reg_check`,
`phy_i2c_check` and `phy_cal_print` are not ported.

## What is currently safe to claim

The repository can claim a source-only, working ESP32-S31 Wi-Fi cold
full-calibration implementation for the qualified default profile. It should
not yet claim:

- byte-for-byte or transaction-for-transaction equivalence to complete
  `phy_bb_init`;
- calibration-record lifecycle parity;
- channel 14 or 802.11p PHY parity;
- general frequency-table count, signed-offset or descriptor-control parity;
- BT/BLE/coexistence PHY initialization parity;
- wakeup, shutdown or runtime tracking parity;
- identical failure behaviour on stuck hardware.

Future audits should close findings by comparing the complete parent and all
reachable leaves, then adding the capability to the functional inventory. A
name match or a shared ROM leaf is not sufficient.
