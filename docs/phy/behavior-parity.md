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
and ordering are well covered by source comments and unit tests, but the
strict audit has since found reached RX-gain differences that those tests did
not detect. One unconditional vendor child is also absent:
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

This earlier sequence-level statement does not imply transaction parity:
strict review of items 14 and 16 found the parameter-byte, guard-offset and
PBus-delay defects recorded below.

## Findings

### PHY-PARITY-000: RX gain cold path is not transaction-equivalent

Severity: **high; reached default cold path**.

Complete `phy_rx_table_init` stores halfword `0x4f4f` at
`phy_param + 0x120`, setting both bytes `0x120` and `0x121`. Rust sets only
byte `0x120`, preserves the old byte `0x121`, and supplies that old value to
the following AGC register initializer.

Complete `phy_set_rx_gain_table` reads and updates its `0x80/0x200` guard bits
in the word at `phy_param + 0xa4`. The active Rust owner reads and commits
those flags at `+0xb4`, sixteen bytes later. It also omits the vendor's
unconditional read of `0x20100434` on cached paths.

Finally, each vendor `phy_wr_rx_gain_mem_new` cleanup reaches
`phy_pbus_force_mode(0)`, which holds the work-mode pulse for `2 µs`. The Rust
RX-gain publisher requests only `1 µs` before clearing the pulse. The PAC/HAL
set and clear operations are correct; the duration supplied by the PHY
transition is not.

These are open-code defects, not vendor errors. Exact instruction evidence is
in [the `phy_rx_gain.o` audit](audit/libphy-phy_rx_gain.md).

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
branch. The complete parent branches, 508-byte payload copy and 130-word
checksum are recorded in
[the `phy_init.o` audit](audit/libphy-phy_init.md).

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

The complete `phy_init.o` audit fixes the shutdown and wakeup boundary:
`phy_close_fe_bb_clk`, `phy_xpd_rf_new`, `phy_close_rf`, and the 392-byte
`phy_wakeup_init` composition are all absent. In particular, wakeup uses the
parallel `phy_i2c_init2` path and restores frequency, PBus, TX-cap, CBW and
channel state; rerunning cold initialization is not the same transaction
trace.

All nine `phy_track.o` bodies are now instruction-audited. The missing
software layer includes four temperature-band TXRF I²C profiles, thresholded
Wi-Fi and BT gain regeneration, RFPLL tracking, RX/dcode recalibration and
radio-specific TXDC/PWDET recalibration. Hardware background-power enablement
does not reproduce those parent traces. The vendor Wi-Fi branch compares
against temperature offset `0x1f8` but explicitly commits offset `0x48`;
this remains a defect candidate rather than an accepted exception until every
child is proved not to update `0x1f8`. See
[the `phy_track.o` audit](audit/libphy-phy_track.md).

### PHY-PARITY-010: diagnostic output is intentionally absent

Severity: **no radio-state impact for audited fixed-debug branches**.

Several translated functions omit `ets_printf`/debug formatting when the
audited parent passes debug zero or when the branch has no hardware or
parameter-state effect. This differs at the logging ABI but not at the
qualified radio-state boundary. Debug entry points such as `phy_reg_check`,
`phy_i2c_check` and `phy_cal_print` are not ported.

The complete debug-member audit makes this boundary exact:
`phy_reg_check` performs 1933 ordered word loads from 21 fixed MMIO ranges,
`phy_i2c_check` performs 168 ordered logical-bank reads, and
`phy_pbus_print` performs eleven selector/path reads. `phy_cal_print` is not
read-only: it also reaches VDD33 and temperature measurement and conditionally
rewrites the 32-entry Wi-Fi gain memory. No current Rust root reproduces those
traces. See [the `phy_debug.o` audit](audit/libphy-phy_debug.md).

### PHY-PARITY-011: complete PHY-I2C surface is narrower

Severity: **high for paths that select parallel initialization; default cold
command-RAM path unaffected**.

Rust exactly publishes the 45 command-RAM words and represents the 26 writes
of `phy_i2c_init1`. It does not implement `phy_i2c_init2`, whose vendor body
sets the shared read-mask field, publishes 22 pairs of PHY-I2C write commands,
and restores the host-map field. Five nonzero read-mask inputs below logical
block `0x61` are also absent from `PhyI2cAddress`.

The chip-level `PhyI2cMasterControl` is an integration trait with no
implementation in this repository. Consequently the action graph and HAL
contract exist, but source in this repository alone does not prove that the
host-map RMW and command words are executed on target. The exact arrays and
register order are recorded in
[the `phy_i2c.o` audit](audit/libphy-phy_i2c.md).

The same target-binding gap keeps ROM `phy_bbpll_cal` open: the Boolean
encodings are correct, but the implementation of the required fresh RMW at
`0x2010f818` is external. The separate ROM `phy_bbpll_recal` is not ported;
between its mode-two write and mode-one child it performs an additional fresh
read whose value is discarded. Two independently scheduled Boolean actions
are not an exact substitute for this contiguous trace. See
[the ROM RXIQ/BBPLL audit](audit/rom-rxiq-bbpll-control-leaves.md).

### PHY-PARITY-012: general tone and CFR controls are not complete

Severity: **medium for calibration/debug extensions; reached one-path tone
profiles remain covered**.

Rust's calibration-tone leaf reproduces the archive call profiles currently
used by cold calibration, where the second tone path is all zero. Vendor
`phy_start_tx_tone_step_new` accepts six arguments and programs both path
images. Rust has no representation for nonzero second-path enable, selector,
or step values. The separate archive `phy_stop_tx_tone_new` and ICCFR/HCCFR
control functions are also absent.

The older ROM `phy_start_tx_tone_step` also accepts two complete paths. Its
reached enabled first-path profile is represented, including DAC and gain
disable, but its both-paths-disabled branch is not one exact Rust operation.
That branch delays five microseconds, restores the stop bits, calls the
installed gain callback and restores two DAC-scale bytes. The Rust TX-DC
composition adds two arm-bit-clearing RMWs after its archive-style restore.

The exact masks and ordering are recorded in
[the `phy_reg.o` audit](audit/libphy-phy_reg.md) and
[the ROM tone audit](audit/rom-tone-fe-agc-leaves.md). These are ordinary
coverage gaps, not vendor-defect exceptions.

### PHY-PARITY-013: shared TX-calibration PBus pulse is one microsecond short

Severity: **high for cold analog-calibration trace parity**.

Complete ROM `phy_pbus_force_mode(0)`, reached by
`phy_txcal_work_mode`, delays 2 microseconds after asserting the second
work-mode pulse. Rust `PhyTxCalibrationEnvironmentTransition` and the
TXDC/PWDET cleanup delay only 1 microsecond. This affects the reached
`phy_tx_cap_init`, `phy_tx_pwctrl_init`, `phy_txdc_cal_pwdet_init` and TXIQ
child graph. The dedicated `phy_txdc` transition already uses the correct
2-microsecond pulse and is not affected.

This is the same lower timing contract exposed by the RX-gain audit, but a
separate shared Rust owner. It is not a vendor defect. See
[the `phy_tx_cal.o` audit](audit/libphy-phy_tx_cal.md).

### PHY-PARITY-014: RX-calibration cleanup and standalone domains differ

Severity: **high for reached cold cleanup; medium outside the cold input
domain**.

Complete `phy_set_rx_gain_cal_dc_new` reaches
`phy_pbus_force_mode(0)` after each bank. Rust
`PhyRxGainDcTransition` expands that tail, but holds its second conditional
work-mode pulse for one microsecond instead of the vendor's two. This is a
reached cold-path timing mismatch.

Complete `phy_check_rx_sat` reaches the same ROM work-mode child after its 100
status samples. `PhyRxSaturationMmioBinding` invokes the correct initial HAL
leaf, but discards its `wifi_baseband_is_enabled` result. When that result is
true, Rust omits the vendor's one-microsecond settle delay, pulse assertion,
two-microsecond pulse delay and pulse clear. Its eleven-command setup,
five-microsecond delay, 100 reads and one-way saturation flag otherwise match.

Two standalone input domains are also narrower. Vendor
`phy_pbus_rx_dco_cal_1step_new` sign-extends its two caller DCO halfwords
before correction and clamping; Rust retains them as unsigned values, which
differs for negative halfword images. Vendor `phy_set_rx_gain_cal_iq_new`
accepts a nonzero first input that saves, clears and restores an I2C bit and
uses a caller-supplied tone selector. Rust owns only the zero/`0x80` cold
profile.

None of these differences is a vendor defect. Exact branches and transaction
order are recorded in
[the `phy_rx_cal.o` audit](audit/libphy-phy_rx_cal.md).

### PHY-PARITY-015: PBus selector zero uses the wrong result register

Severity: **high for any selector-zero result consumer**.

Complete ROM `phy_pbus_rd_addr` uses `0x201008a0` for selector zero,
independently of the path input. Its companion shift helper selects bit 9 for
non-path-one and bit 18 for path one. Rust `read_pbus_result` instead reads
those windows from `0x201008a4`; the recovered SVD and PAC contain the same
incorrect address claim. Selectors 1 through 5 match the ROM tables.

This is an ordinary Rust/SVD defect, not a vendor exception. The jump-table
words, expanded address/shift map and complete read body are recorded in
[the ROM PBus audit](audit/rom-pbus-core.md).

### PHY-PARITY-016: Rust adds a PBus read before every command

Severity: **medium under unique ownership; observable transaction mismatch**.

ROM `phy_pbus_force_test` freshly reads and publishes the command word before
its first `BUSY` sample. Rust `try_start_force_test` first reads `BUSY`, then
publishes only if that sample is clear. On a ready bus this is still one
additional MMIO read. On a busy bus Rust returns a typed failure while ROM
overwrites the command image and polls.

Replacing the unbounded post-publication ROM loop with executor-owned samples
and a deadline is justified by `VENDOR-ROBUSTNESS-001`. The pre-publication
sample is separate: it changes the successful register trace and is therefore
not covered by that exception. Every composite Rust helper that uses the
binding inherits this difference even when its selector/path/value tuples
match. See [the ROM PBus audit](audit/rom-pbus-core.md).

### PHY-PARITY-017: estimator completion performs one extra MMIO read

Severity: **medium; reached by cold analog calibration**.

ROM `phy_iq_est_enable` reads `0x2010047c` for readiness. Only when readiness
is clear does it then read activity from `0x201008d0` and update its diagnostic
counter. A ready observation returns without the activity read. Rust
`sample_iq_estimator_readiness` unconditionally reads both words, including
on the final ready observation.

Response-indexed async polling and a finite deadline can preserve successful
ROM ordering while avoiding the documented unbounded wait. This additional
completed-state read is not required by that safety change and violates the
strict no-invented-transaction rule. It propagates through
`phy_dc_iq_est`, `phy_rxdc_est_min`, both RX-DCO calibrators and their archive
parents. The complete instruction proof is in
[the ROM DC/IQ and RX-DCO audit](audit/rom-dc-iq-rx-dco.md).

### PHY-PARITY-018: packed TX-IQ extrema bypass vendor saturation

Severity: **medium; input-dependent coefficient error**.

ROM `phy_get_iq_value` decodes gain to `[-32,31]` and phase to `[-64,63]`.
ROM `phy_txiq_set_reg` then saturates the values to `[-31,31]` and
`[-63,63]` before writing bits 5:0 or 12:6 of `0x20100c0c`. Rust decodes the
same ranges, but its PAC setters only mask the `i8`.

For packed gain `-32`, vendor writes field image `0x21` (`-31`) while Rust
writes `0x20`. For phase `-64`, vendor writes `0x41` (`-63`) while Rust
writes `0x40`. This closes archive `phy_set_lb_txiq_new` as a mismatch and is
not a vendor-defect exception. See
[the ROM DC/IQ and RX-DCO audit](audit/rom-dc-iq-rx-dco.md).

### PHY-PARITY-019: same-named ROM and archive control leaves are not interchangeable

Severity: **medium for callers bound directly to the ROM versions**.

Three adjacent ROM functions differ materially from their closest
archive/Rust operations:

- ROM `phy_fe_reg_update` performs the same first three RMWs as the archive
  function, then appends two fresh DAC-scale RMWs. Rust correctly implements
  the installed archive function and therefore does not match the standalone
  ROM function.
- ROM `phy_txgain_comp_pacfg_(nonzero)` restores the four bytes of
  `0x20100410` to `[fd,f8,fd,fb]`. The archive replacement and Rust use
  `[00,fa,ff,00]`. Their zero branches both perform the same two full-word
  zero stores.
- ROM `phy_force_rx_gain_trig` conditionally replaces the high byte of
  `0x2010702c`, pulses bit 23 around a one-microsecond delay, and has no Rust
  implementation.

These differences are not classified as vendor defects. They show why each
bound function body must be audited instead of treating an archive `_new`
replacement as proof for a same-named ROM symbol. The instruction traces are
in [the ROM tone/front-end/AGC audit](audit/rom-tone-fe-agc-leaves.md).

### PHY-PARITY-020: older ROM RX compensation uses a different constant

Severity: **low for the current archive-bound graph; high for direct ROM-leaf
substitution**.

ROM `phy_set_rx_comp_` writes byte `0xeb` to the low byte of `0x2010702c`
and the high byte of `0x201070a0`. The installed archive
`phy_set_rx_comp_new` writes `0xed` to both locations, and Rust correctly
implements the archive version reached by channel setup.

This is not evidence that either vendor constant is erroneous. It is another
versioned function-body difference, so the Rust/archive match cannot be used
as proof for the standalone ROM symbol. The same audit also finds the ROM CCA
pair, RX-filter selector and FE TX/RX reset pulse unported. See
[the ROM AGC/CCA/channel audit](audit/rom-agc-cca-channel-leaves.md).

### PHY-PARITY-021: NRX frequency division changed signedness and zero handling

Severity: **medium outside the reached positive-frequency profile**.

ROM `phy_nrx_freq_set` samples `0x20107848` twice, computes a shifted
numerator, and applies signed RV32 `div` with the full caller word. Rust
preserves the two reads and final write, but accepts a nonzero `u16` and uses
unsigned `u32` division.

For divisor zero, RISC-V produces quotient `0xffffffff`, so ROM still writes
low field image `0x000fffff`; Rust asserts before the write. Shifted
numerators with bit 31 set also distinguish signed from unsigned division,
and full-word divisors are absent from the Rust API. This is a normal
complete-domain mismatch, not a vendor-defect exception. See
[the ROM FBW/force-control audit](audit/rom-fbw-force-control-leaves.md).

### PHY-PARITY-022: channel-CBW helper narrows an unmasked ROM word path

Severity: **low for byte-valued channel owners; observable for standalone
full-word calls**.

ROM `phy_bb_cbw_chan_cfg` accepts a full word. When its high portion is
nonzero, it subtracts one, zero-extends the low byte, clears only bits 3:0 of
`0x20104400`, and ORs the entire normalized byte. Values above `0x0f` can
therefore set bits 7:4. Rust accepts `u8` and publishes through a four-bit PAC
field, while otherwise preserving the four RMW operations.

The reached channel owner supplies a byte and is unaffected. Complete
standalone function parity is still a mismatch, not a vendor-defect
exception. See
[the ROM CBW/feature/watchdog audit](audit/rom-cbw-feature-watchdog-leaves.md).

## What is currently safe to claim

The repository can claim a source-only, working ESP32-S31 Wi-Fi cold
full-calibration implementation for the qualified default profile. It should
not yet claim:

- byte-for-byte or transaction-for-transaction equivalence to complete
  `phy_bb_init`;
- calibration-record lifecycle parity;
- channel 14 or 802.11p PHY parity;
- general frequency-table count, signed-offset or descriptor-control parity;
- `phy_i2c_init2` or the complete vendor PHY-I2C input domain;
- general dual-path tone, ICCFR or HCCFR parity;
- BT/BLE/coexistence PHY initialization parity;
- wakeup, shutdown or runtime tracking parity;
- identical failure behaviour on stuck hardware.

Future audits should close findings by comparing the complete parent and all
reachable leaves, then adding the capability to the functional inventory. A
name match or a shared ROM leaf is not sufficient.
