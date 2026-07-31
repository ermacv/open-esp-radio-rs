# Line audit: `libphy.a[phy_tx_cal.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines ten external code functions. Every instruction,
relocation, branch, loop and constant table was inspected. Strict results are
one **NO-REGISTER-EFFECT**, two **NOT-PORTED** and seven **MISMATCH**.

The most important reached mismatch is shared by four Rust TX-calibration
owners. Vendor `phy_txcal_work_mode` reaches ROM `phy_pbus_force_mode(0)`,
whose second work-mode pulse delay is 2 microseconds. Rust
`PhyTxCalibrationEnvironmentTransition` and the TXDC/PWDET cleanup use only
1 microsecond. This is an ordinary Rust timing defect, not a vendor-defect
exception.

## `phy_bt_txdc_cal_new`

Size `0xfe`. Strict status: **NOT-PORTED**.

Bit 12 of the guard word at `phy_param + 0xa4` suppresses the complete body.
When clear, the exact order is:

1. `phy_pbus_debugmode()`;
2. `phy_pbus_xpd_tx_on(15, 0)`;
3. read `phy_pbus_rd(1, 1)`, set bit 1 and force the result to `(1, 1)`;
4. force `(4, 2, phy_param[0x14] << 3)`;
5. for BT gain indices 0, 1 and 2, call `phy_bt_index_to_bb`, force the
   resulting value to `(1, 2)`, and call `phy_txdc_cal` into the eight-byte
   rows at `phy_param + 0x104`, `+0x10c` and `+0x114`;
6. `phy_pbus_xpd_rx_on(0)`;
7. `phy_pbus_workmode()`;
8. set guard bit 12.

Rust has no BT TXDC owner or these three BT DCO rows.

## `phy_txiq_cal_init`

Size `0x14c`. Strict status: **MISMATCH**.

Guard bit 14 at `phy_param + 0xa4` suppresses the complete body. Otherwise:

1. program RFPLL frequency `0x985` with crystal selector
   `phy_param[0x4f]` and offset zero;
2. `phy_set_txcap_reg(phy_param + 0xdc, 6)`;
3. `phy_set_channel_dcode(6)`;
4. read RFPLL `(0x62, host 1, reg 0x13, bit 6)`;
5. if that bit is nonzero, save bits 5:0 of registers `0x13` and `0x14` at
   `phy_param[0x198..=0x199]`; otherwise save bits 5:0 of registers `0x11`
   and `0x12`;
6. saturate signed `phy_param[0x18]` to `[0, 120]`;
7. call `phy_rfcal_txiq(0, param+0xa8, param+0xd0, 0x80,
   attenuation, 0)`;
8. call `phy_get_tsens_value()` and store the halfword at `0x19a`;
9. saturate `attenuation - 40` to `[0, 120]`;
10. call `phy_rfcal_txiq(0, param+0xa8, param+0xe6, 0x80,
    second_attenuation, 2)`;
11. set guard bit 14.

Rust represents both RFPLL/D-code branches, both calibrations and the
temperature edge. Its transitive `phy_rfcal_txiq` environment uses the shared
1-microsecond PBus pulse delay instead of the vendor 2 microseconds, so even
the reached cold branch is not trace-equivalent.

## `phy_txdc_cal_init`

Size `0x110`. Strict status: **MISMATCH**.

The four arguments are output pointer, two values forwarded to
`phy_pbus_xpd_tx_on`, and a Boolean-like optional force flag. The direct body:

1. reads diagnostic bit 4 from `phy_param[0x10]`;
2. enters PBus debug mode and calls
   `phy_pbus_xpd_tx_on(input1, input2)`;
3. for a nonzero fourth input, reads `(1,1)`, sets bit 1 and forces it back;
4. for exactly five indices, calls `phy_index_to_txbbgain(index)`, forces that
   value to `(1,2)`, calls `phy_txdc_cal(output + index * 8)`, and optionally
   prints the four result halfwords;
5. calls `phy_pbus_xpd_rx_on(0)` and `phy_pbus_workmode()`;
6. optionally prints the closing diagnostic line.

Rust reproduces the five-row cold call
`(output=owned rows, input1=15, input2=0, input3=0)` and uses the correct
2-microsecond pulse delay in its dedicated TXDC transition. It does not expose
the other PBus inputs or the nonzero fourth-input force branch, so the complete
function domain is a mismatch.

## `phy_txdc_cal_pwdet_new`

Size `0x3b4`. Strict status: **MISMATCH**.

The body copies four input DCO halfwords, selects the I or Q component from the
Boolean interpretation of its fourth input, and performs two bounded search
phases. Every hardware sample has the exact repeated shape:

1. modify the selected DCO halfword;
2. `phy_pbus_set_dco` with all four working halfwords;
3. `ets_delay_us(10)`;
4. call `g_phyFuns[11](2)`, the tone/SAR sample callback.

The precheck begins with positive and negative steps of 10 and may reduce a
side by 5 according to the unsigned 15-count comparison. The scan has a hard
50-point bound and can stop after the first six points when the sample rises
more than 30 above the running minimum. The body orders the measured samples,
performs the bounded interpolation/averaging pass, stores the selected final
halfwords through the caller pointer, and calls `phy_pbus_set_dco` once more
with the final four-halfword image. Its second argument only controls
formatting and its third argument is not consumed.

`PhyTxDcPwdetSearchTransition` preserves both component branches, every
10-microsecond measurement edge, the data-dependent finite scan and the final
PBus image. Omitting diagnostic prints does not alter register behaviour.
However, each `phy_pbus_set_dco` child expands into four
`phy_pbus_force_test` calls. The Rust hardware binding performs an additional
`0x20100890` busy read before every command publication, while ROM publishes
first and polls only afterwards. The direct search algorithm matches, but its
now-closed register-relevant child proof makes the complete function a strict
mismatch. See [the ROM PBus audit](rom-pbus-core.md).

## `phy_txdc_cal_pwdet_init`

Size `0x208`. Strict status: **MISMATCH**.

The first input is unused. The second input controls diagnostic output and,
more importantly, a post-calibration early return. The third input selects
Wi-Fi versus BT DCO rows and PBus gain mapping.

The setup first saves `0x20100814` and `0x20100808`, replaces the low byte of
the former with `0xf0`, replaces bits 11:4 of the latter with `0x78`, enables
the TX clock and power detector, enters PBus debug mode, forces TX and RX off,
starts tone `(1, 0x200, 0x78, 0, 0, 0)`, delays 1 microsecond, forces TX on,
and replaces bits 13:12 of `0x2010080c` with 1.

It calibrates exactly three rows. A zero third input uses Wi-Fi
`phy_index_to_txbbgain` and rows at `phy_param + 0xa8`; a nonzero input uses
`phy_bt_index_to_bb`, the additional BT PBus setup, and rows at
`phy_param + 0x104`. Each row calls
`phy_txdc_cal_pwdet_new(row, second_input, third_input, 1)`.

A nonzero second input returns immediately after the third row and deliberately
leaves setup active. A zero second input:

1. loads the fixed four-halfword `0x0100` DCO image;
2. forces TX off;
3. stops the tone with `(0, 0x80, 0x78, 0, 0, 0)`;
4. calls `phy_pbus_workmode()` and disables the TX clock;
5. restores the saved low byte in `0x20100814` and the saved bits in
   `0x20100808`;
6. sets bits 13:12 in `0x2010080c`.

Rust implements only the cold Wi-Fi `(second=0, third=0)` composition. It
omits the BT and skip-cleanup traces, and its cleanup holds the second PBus
pulse for 1 microsecond instead of the vendor 2 microseconds.

## `phy_tx_cap_init`

Size `0xe6`. Strict status: **MISMATCH**.

The function derives its diagnostic flag from bit 7 of `phy_param[0x10]`,
enters TX-cal debug mode through `g_phyFuns[4]`, then processes the constant
channels 1, 6 and 11. For each channel it:

1. programs the channel frequency with crystal selector `phy_param[0x4f]`;
2. writes TXRF register 2 bits 3:0 to 7 and bits 7:4 to 13;
3. on channel 1 only, calls
   `phy_get_power_atten(0x80, current_attenuation, 40,
   phy_param_s16[0x0e], diagnostic)` and stores the signed result at
   `phy_param[0x18]`;
4. calls `phy_rfcal_txcap(0x80, attenuation, diagnostic,
   phy_param + 0xdc + channel_index * 2)`.

It tail-calls `phy_txcal_work_mode()` after the third channel. Rust preserves
the three channels, two writes, first-channel attenuation search and six
result bytes, but its shared work-mode transition uses a 1-microsecond second
pulse instead of 2 microseconds.

## `phy_tx_pwctrl_init_cal_new`

Size `0x18c`. Strict status: **MISMATCH**.

This four-argument calibration helper supports two hardware modes. Zero first
input uses Wi-Fi constants `(selector=0x80, channel adjustment=52,
curve adjustment=12, limit=16)`. Every nonzero value uses the BT constants
`(selector=phy_param[3], channel adjustment=8, curve adjustment=10,
limit=22)` and first programs TX-cap row 6.

It sets `phy_param[0x1aa]=1`, then loops over constant channels 1, 6 and 11.
For every channel it programs RFPLL; Wi-Fi also selects the corresponding
TX-cap row. It calls complete ROM `phy_rfcal_pwrctrl` with the current
attenuation, reference-code pointers, power offset, diagnostic bit and the
caller output byte. The middle result receives an additional 2. Each result
plus the mode-specific adjustment becomes the next attenuation at
`phy_param[0x18]`.

After three points, the body either keeps the base correction or saturates an
adjustment to `[-40,40]`, updates all three curve bytes and publishes the
correction byte, then always clears `phy_param[0x1aa]`.

Rust implements the complete Wi-Fi mode used by `phy_tx_pwctrl_init`, but not
the nonzero/BT mode and its different selector, limits, TX-cap selection and
output contract. This is a strict domain mismatch even before the parent
work-mode timing defect.

## `phy_tx_pwctrl_init`

Size `0x9a`. Strict status: **MISMATCH**.

Guard bit 20 at `phy_param + 0xa4` suppresses the body. When clear:

1. enter TX-cal debug mode through `g_phyFuns[4]`;
2. program RFPLL channel 1 using `phy_param[0x4f]`;
3. select TX-cap row 1 from `phy_param + 0xdc`;
4. `phy_pwdet_ref_code(80)`;
5. call `phy_tx_pwctrl_init_cal_new(0, param+0xf1, param+0xf7,
   param+0xf4)`;
6. `phy_txcal_work_mode()`;
7. set guard bit 20 and store channel 11 at `phy_param[0x11c]`.

Rust owns this Wi-Fi branch and guard, but the shared work-mode tail uses the
wrong 1-microsecond second pulse. The reached cold trace is therefore a
mismatch.

## `phy_tx_atten_comp`

Size `0x16`. Strict status: **NO-REGISTER-EFFECT**.

The body adds 3, with byte wrapping, to caller byte 1 and adds 4, with byte
wrapping, to caller byte 2. It has no global, child call or MMIO access. Rust
does not expose the raw two-byte adjustment helper.

## `phy_bt_tx_pwctrl_init`

Size `0x1ae`. Strict status: **NOT-PORTED**.

Guard bit 15 at `phy_param + 0xa4` suppresses the complete body. Otherwise the
function:

1. saves BBTOP register `0x1c` and bits 5:0 of register `0x1e`;
2. writes full value 2 to registers `0x1c` and `0x1d`, then writes bits 5:0
   of registers `0x1e` and `0x1f` to 2;
3. enters TX-cal debug mode;
4. performs the BT PBus TX-path, loopback and DCO setup derived from
   `phy_param[0x13]`, `[0x14]` and `phy_bt_bb_to_index(0)`;
5. calls `phy_tx_pwctrl_init_cal_new(1, param+0xfb, param+0xfe,
   param+0xf8)`;
6. stores zero at `phy_param[0x100]`;
7. restores full BBTOP registers `0x1c` and `0x1d` from the saved register
   `0x1c` byte and restores bits 5:0 of `0x1e` and `0x1f` from the saved
   field;
8. calls `phy_txcal_work_mode()` and sets guard bit 15.

The unusual duplicated restore values are the literal vendor trace. No Rust
BT power-control calibration owner exists.

## Member conclusion

The cold Wi-Fi algorithms are substantially represented, but the member is
not register-equivalent. Four reached calibration paths share the wrong
second PBus pulse delay, several typed owners narrow vendor input branches,
and both BT parents are absent. No divergence in this member qualifies as a
proved vendor-defect exception.
