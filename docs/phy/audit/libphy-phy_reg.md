# Line audit: `libphy.a[phy_reg.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines twelve external code functions. Every instruction,
relocation and branch was inspected. Eleven functions have closed strict
results. `phy_open_i2c_xpd_new` remains **BODY-AUDITED** pending closure of
its ROM wait child and proof of the external platform binding.

## `phy_set_rx_comp_new`

Size `0x28`. Strict status: **MATCHED**.

The function performs exactly two fresh-read RMWs:

1. replace bits 7:0 of `0x2010702c` with `0xed`;
2. replace bits 31:24 of `0x201070a0` with `0xed`.

`RadioRegisters::configure_rx_compensation` preserves both addresses, masks,
values and order.

## `phy_open_i2c_xpd_new`

Size `0xac`. Strict status: **BODY-AUDITED**.

For nonzero input the function:

1. replaces the upper halfword of `0x20704184` with zero;
2. clears bit 28 of `0x207040f0`;
3. calls `ets_delay_us(100)`.

Both input branches then:

1. set the upper halfword of `0x20704184` to all ones;
2. set bit 28 of `0x207040f0`;
3. read `0x20704208`;
4. if bit 30 was clear, set bit 30, clear bit 31, then set bit 31 through
   three independent fresh-read RMWs;
5. otherwise skip that three-write pulse;
6. read `0x20704208` again and set bit 31 if still clear;
7. tail-call `phy_wait_i2c_sdm_stable`.

Rust `OpenI2cXpdTransition` represents both branches, the 100-microsecond
edge, conditional pulse and SDM-stability observations. The strict proof
remains open because the ROM wait child and all response histories are not
yet closed, and the PMU operations are supplied through an external platform
trait rather than an implementation in this repository. Deadline termination
may qualify only through the documented vendor unbounded-poll exception.

## `phy_fe_reg_update`

Size `0x32`. Strict status: **MATCHED**.

It performs three fresh-read RMWs:

1. set bit 25 of `0x20100c08`;
2. set bit 26 of `0x20100c08`;
3. set bits 1:0 of `0x20100448`.

The PAC `update_front_end` leaf retains the repeated read of the first
register and has no ROM-only DAC-scale tail.

## `phy_set_ftm_en`

Size `0x14`. Strict status: **MISMATCH**.

The body replaces bit 0 of `0x20107d4c` with `input & 1`. Rust sets this bit
to one as the final child of post-initialization register update, but exposes
no corresponding clear path. It therefore matches the reached input `1` and
not the complete vendor input domain.

## `phy_start_tx_tone_step_new`

Size `0xc2`. Strict status: **MISMATCH**.

For six arguments `(enable0, selector0, step0, enable1, selector1, step1)` the
complete order is:

1. call `g_phyFuns[0x30 / 4](0)`, which resolves to
   `phy_txgain_comp_pacfg_new(0)`;
2. replace bits 1:0 of `0x20100428` with `selector0 & 3`;
3. replace bits 3:2 of the same register with `(selector1 << 2) & 0x0c`;
4. replace bits 27:0 of `0x2010041c` with
   `(enable0 << 18) | (selector0 >> 2) | ((-step0 & 0xff) << 10)`;
5. replace bits 27:0 of `0x20100420` with
   `(enable1 << 18) | (selector1 >> 2) | ((-step1 & 0xff) << 10)`;
6. tail-call the same callback with input `1`.

All arithmetic is 32-bit wrapping before the final low-28-bit masks.

Rust `configure_calibration_tone` reproduces the callback/register/callback
trace only for the used profile where the entire second path is zero and the
selector fits Rust's ten-bit contract. The Rust action carries only
`(enabled, selector, step)` and cannot reproduce nonzero
`enable1/selector1/step1`, nor the full vendor selector domain.

## `phy_stop_tx_tone_new`

Size `0x2c`. Strict status: **NOT-PORTED**.

The function:

1. clears bits 17:16 of `0x2010041c`;
2. clears bits 17:16 of `0x20100420`;
3. sets bits 1:0 of `0x2010040c`.

Rust has profile-specific tone restoration and a ROM
`phy_stop_tx_tone(1)` leaf, but the latter appends two DAC-scale writes. No
Rust operation implements this exact three-RMW archive function in isolation.

## `phy_txgain_comp_pacfg_new`

Size `0x54`. Strict status: **MATCHED**.

Input zero performs two full-word zero stores to `0x20100410` and
`0x20100414`. Every nonzero input performs four ordered fresh-read RMWs of
`0x20100410`, replacing its bytes with `[0x00, 0xfa, 0xff, 0x00]`.

The PAC `clear_tx_gain_compensation` and
`restore_tx_gain_compensation` leaves reproduce both branches and every
intermediate write. They are composed around the Rust calibration-tone
operation at the same positions as the two vendor callbacks.

## `phy_bb_txpwr_track`

Size `0xf4`. Strict status: **MISMATCH**.

The body contains fourteen fresh-read RMWs at `0x20107454`,
`0x20107458`, `0x20107460`, and `0x2010745c`. Only the first update is
input-dependent: bit 0 of `0x20107454` receives `input & 1`. The remaining
updates clear or publish the fixed field images represented in
`RadioRegisters::configure_tx_power_tracking`.

The PAC leaf matches all fourteen edges for Boolean input. The vendor accepts
an arbitrary integer and uses its low bit; Rust's Boolean API and reached
parent supply only zero or one. A raw input such as `2` selects zero in the
vendor but has no equivalent Rust call-domain mapping. This is a strict
all-input mismatch despite the matched cold call `phy_bb_txpwr_track(1)`.

## `phy_iccfr_en`

Size `0x2c`. Strict status: **NOT-PORTED**.

Nonzero input clears bits 26:25 of `0x2010747c`; zero input sets both bits.
Each branch is one fresh-read RMW. No Rust PHY operation owns this function.

## `phy_force_iccfr`

Size `0x80`. Strict status: **NOT-PORTED**.

The function performs five ordered fresh-read RMWs of `0x20107478`:

1. bit 14 receives `first_input & 1`;
2. bit 0 receives `second_input & 1`;
3. bit 15 is set;
4. bit 13 receives `first_input & 1`;
5. bits 16:1 are replaced: bits 12:1 receive the low twelve bits of the
   zero-extended third halfword argument, while bits 16:13 become zero.

It then tail-calls `phy_iccfr_en(second_input)`. The complete six-write
behavior is absent from Rust.

## `phy_config_hccfr`

Size `0x38`. Strict status: **NOT-PORTED**.

It replaces bit 22 of `0x20107468` with `first_input & 1`, then replaces
bits 11:0 of `0x2010746c` with `second_input & 0x0fff`. Rust has no owner
for these two RMWs.

## `phy_dc_mem_clr`

Size `0x1c`. Strict status: **MATCHED**.

The body pulses bit 20 of `0x2010703c`: one fresh-read RMW sets it, and a
second fresh-read RMW clears it. `RadioRegisters::clear_agc_dc_memory`
preserves both edges.

## Member conclusion

Four finite leaves are exact matches, and the reached cold calls of
`phy_open_i2c_xpd_new`, tone generation, FTM enable and TX-power tracking
have strong profile-level correspondence. Complete member parity is absent:

- ICCFR/HCCFR control is not ported;
- the standalone archive tone-stop function is not ported;
- the six-argument tone function is narrowed to one path;
- raw low-bit input semantics are narrowed to Boolean APIs;
- the analog-I2C power path still depends on unclosed ROM and target traits.

None of the missing functions or narrowed inputs is a vendor defect.
