# Line audit: `libphy.a[phy_track.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines nine external code functions. Every instruction,
relocation, branch and loop was inspected. All nine functions are strictly
**NOT-PORTED**: the Rust PHY has cold-calibration primitives, but no runtime
temperature/RFPLL/TX-power tracking owner that reproduces these parents,
guards and transaction order.

## `phy_txpwr_cal_track_new`

Size `0x142`. Strict status: **NOT-PORTED**.

Inputs are radio selector, enable and diagnostic-print flag. Zero selector is
Wi-Fi; every nonzero selector is BT.

The body computes an update threshold from the absolute difference between
current signed temperature `phy_param[0]` and calibration temperature
`phy_param[0x12e]`: the threshold is 2 at a difference through 7 and 4 above
7. A nonzero byte at `0x1ab` overrides it with 10.

It saturates the current temperature through pure ROM `phy_get_data_sat` to
`[-60, 80]` for Wi-Fi or `[-60, 105]` for BT. If the absolute difference from
the last TX-power temperature at `phy_param[4]` is below the threshold, the
candidate correction is the signed cached byte at `0x122`; otherwise it calls
pure `phy_temp_to_power(saturated, calibration_temperature, selector)`.

Zero enable returns without hardware access. An enabled call also returns if
the candidate equals the selected cached correction byte (`0x123` for Wi-Fi,
`0x124` for BT). Otherwise the exact state/hardware order is:

1. `phy_bbpll_cal(1)`;
2. store candidate byte at `phy_param[0x122]`;
3. copy current temperature to the halfword at `phy_param[4]`;
4. store the candidate at `0x123` and call
   `phy_wifi_set_tx_gain_new(phy_param[0x11c], 0)` for Wi-Fi, or store it at
   `0x124` and call `phy_bt_set_tx_gain_new(0)` for BT;
5. optionally print the two correction bytes and temperature state;
6. tail-call `phy_bbpll_cal(0)`.

Rust has a cold Wi-Fi TX-gain calculator and publisher, but it has no
temperature-driven update owner, no BT publisher and no matching BBPLL
bracket.

## `phy_tx_i2c_track`

Size `0x14a`. Strict status: **NOT-PORTED**.

This is a four-band state machine over signed current temperature
`phy_param[0]` and state byte `phy_param[0x4d]`. On a band transition it emits
two masked writes to TXRF block `0x6b`, host 1, then updates the state byte:

| Temperature band | Required old-state difference | Register 3 bits 3:0 | Register 7 bits 3:0 | New state |
| --- | --- | ---: | ---: | ---: |
| `< -19` | state is not 1 | 10 | 15 | 1 |
| `-19..=54` | state is not 0 | 10 | 13 | 0 |
| `55..=94` | state is not 2 | 8 | 15 | 2 |
| `> 94` | state is not 3 | 6 | 15 | 3 |

The emitted operation is
`phy_i2c_writeReg_Mask(0x6b, 1, register, 3, 0, value)`. The body tests the
cold, middle, hot and reset bands in its compiled order rather than through a
single switch; the unsigned range idioms preserve the table above over the
16-bit temperature representation. Rust has no runtime TXRF temperature-band
state or corresponding two-write transitions.

## `phy_bt_track_tx_power_new`

Size `0x0e`. Strict status: **NOT-PORTED**.

This branch-free wrapper moves its two inputs into the enable and diagnostic
positions, sets selector 1, and tail-calls
`phy_txpwr_cal_track_new(1, input0, input1)`. The entire BT tracking and
gain-publication path is absent from Rust.

## `phy_wifi_track_tx_power_new`

Size `0x0e`. Strict status: **NOT-PORTED**.

This branch-free wrapper moves its two inputs into the enable and diagnostic
positions, sets selector 0, and tail-calls
`phy_txpwr_cal_track_new(0, input0, input1)`. Rust does not expose the
corresponding runtime Wi-Fi parent even though some cold gain children exist.

## `phy_param_track`

Size `0x68`. Strict status: **NOT-PORTED**.

The exact call/control order is:

1. `phy_i2c_enter_critical()`; the pinned weak target is a no-op;
2. if `phy_param[0x17]` is nonzero, skip directly to step 7;
3. `phy_tsens_temp_read()`;
4. when `phy_param[0x0a]` is nonzero, call
   `phy_rfpll_cap_track(phy_param[9])`;
5. `phy_wifi_track_tx_power_new(input0, input1)`;
6. `phy_bt_track_tx_power_new(input0, input1)`;
7. tail-call `phy_i2c_exit_critical()`; the pinned weak target is a no-op.

The early gate suppresses temperature acquisition and every tracking child.
Rust has temperature and RFPLL cold primitives but no owner with this gate,
dual Wi-Fi/BT update and critical-section boundary.

## `phy_cal_param_track`

Size `0x25a`. Strict status: **NOT-PORTED**.

Inputs are diagnostic-print flag and radio selector. The temperature threshold
is normally 30. If bit 1 of `phy_param[0x1b0]` is set, the full byte at
`phy_param[0x1b1]` replaces it. The function then evaluates three independent
calibration sections in this exact order.

### RX/dcode section

When `abs(phy_param_s16[0x190] - phy_param_s16[0]) >= threshold`:

1. `phy_pbus_clear_reg()`;
2. save channel halfword `0x11c` and signed CBW byte `0x11f`;
3. optionally print old and current temperatures;
4. `phy_dcode_cal_init()`;
5. fresh-read `0x20109c18`, clear bits 1:0, write it;
6. fresh-read the word at `phy_param + 0xa4`, clear bits `0x280`, write it;
7. `phy_set_rx_gain_table(0x985, 0)`;
8. `phy_chip_set_chan(saved_channel, saved_cbw)`;
9. `phy_mac_enable_bb()`;
10. copy current temperature to `phy_param[0x190]`.

### Wi-Fi TXDC/PWDET section

When `abs(phy_param_s16[0x1f8] - current) >= threshold` and selector is zero:

1. `phy_dis_hw_set_freq()`;
2. `phy_force_txrx_off(1)`;
3. `phy_pbus_clear_reg()`;
4. fresh-read `0x20109c18`, clear bits 1:0, write it;
5. save CBW byte `0x11f` and channel halfword `0x11c`;
6. optionally print old and current temperatures;
7. `phy_bb_cbw_chan_cfg(0)`;
8. `phy_txdc_cal_pwdet_init(0, 0, 0)`;
9. `phy_wifi_set_tx_gain_new(saved_channel, 0)`;
10. copy current temperature to `phy_param[0x48]`;
11. `phy_bb_cbw_chan_cfg(saved_cbw)`;
12. `phy_mac_enable_bb()`;
13. `phy_force_txrx_off(0)`;
14. `phy_en_hw_set_freq()`.

The comparison uses baseline `0x1f8`, but the explicit parent store updates
`0x48`, not `0x1f8`. The initialization path gives both offsets the same
temperature. This asymmetry is a vendor-defect candidate because a persistent
temperature delta can retrigger the section, but it is not accepted as a
`VENDOR-DEFECT-EXCEPTION` until the complete child graph proves that no child
updates `0x1f8`.

### BT TXDC/PWDET section

When `abs(phy_param_s16[0x1fa] - current) >= threshold` and selector equals
one, the trace is the same disable/force/PBus/MMIO/CBW bracket, with:

- `phy_txdc_cal_pwdet_init(0, 0, 1)`;
- `phy_bt_set_tx_gain_new(0)`;
- an explicit current-temperature store back to `phy_param[0x1fa]`.

After all three sections, the function unconditionally tail-calls
`g_phyFuns[12](1)`, which resolves to
`phy_txgain_comp_pacfg_new(1)`. That child performs four fresh-read RMWs of
`0x20100410` and restores the PA compensation image even when no temperature
threshold was crossed.

Rust owns several individual cold-calibration children but none of these
runtime gates, brackets, parameter commits or the unconditional compensation
tail.

## `phy_param_track_tot`

Size `0xae`. Strict status: **NOT-PORTED**.

The body enters the no-op I²C critical boundary and reads gate bytes
`phy_param[0x17]` and `[0x195]`. If either is nonzero, it skips all work and
exits the boundary.

Otherwise:

1. optionally call `phy_rfpll_cap_track(phy_param[9])` when byte `0x0a` is
   nonzero;
2. if second input is nonzero, call
   `phy_bt_track_tx_power_new(phy_param[0x0b], phy_param[9])`, then, when byte
   `0x192` is zero, call `phy_cal_param_track(phy_param[9], 1)`;
3. if first input is nonzero, call `phy_tx_i2c_track()`, then
   `phy_wifi_track_tx_power_new(1, phy_param[9])`, then, when byte `0x192` is
   zero, call `phy_cal_param_track(phy_param[9], 0)`;
4. call `phy_tsens_temp_read()` after the selected tracking work;
5. tail-call `phy_i2c_exit_critical()`.

Unlike `phy_param_track`, this parent samples temperature at the end, so its
tracking decisions use the previously stored temperature. Rust has no
equivalent background scheduler/root.

## `phy_bt_track_pll_cap`

Size `0x0c`. Strict status: **NOT-PORTED**.

The complete wrapper sets arguments `(wifi = 0, bt = 1)` and tail-calls
`phy_param_track_tot(0, 1)`. It therefore may track RFPLL/BT power, perform BT
recalibration and sample temperature. No matching Rust operation exists.

## `phy_tx_pwctrl_background`

Size `0x0c`. Strict status: **NOT-PORTED**.

The complete wrapper sets arguments `(wifi = 1, bt = 0)` and tail-calls
`phy_param_track_tot(1, 0)`. It therefore may update TXRF I²C temperature
bands, Wi-Fi TX power and Wi-Fi calibration before sampling temperature. The
Rust cold path enables hardware background power detection, but that is not
this vendor software tracking root and does not reproduce its transactions.

## Member conclusion

All nine runtime/background functions are absent as compositions, even where
Rust already owns a cold-calibration child. This is a complete-radio lifecycle
gap, not a cold-start mismatch and not a vendor-defect exception. The
`0x1f8` comparison versus `0x48` commit remains explicitly open as a possible
vendor defect; Rust may not diverge on that basis without completing the child
proof.
