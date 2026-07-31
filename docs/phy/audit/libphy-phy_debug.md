# Line audit: `libphy.a[phy_debug.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines twelve external code functions. Every instruction,
relocation, branch and loop was inspected. All twelve functions have closed
strict results: six have no direct or transitive register effect and six
diagnostic hardware operations are not ported.

None of the differences in this member is classified as a vendor defect.
In particular, the channel-14 final state of `rfpll_cap_check` and the
hardware-mutating suffix of `phy_cal_print` are observable parts of the vendor
diagnostic contract.

## `get_bias_ref_code`

Size `0x04`. Strict status: **NO-REGISTER-EFFECT**.

The complete body loads zero into the return register and returns. It has no
memory access, MMIO or child call. Rust has no standalone equivalent.

## `get_dc_value`

Size `0x0e`. Strict status: **NO-REGISTER-EFFECT**.

For output pointer `a0` and input word `a1`, the body stores the high halfword
at `output + 0`, stores the low halfword at `output + 2`, and returns. It only
modifies the caller buffer and has no child call or register effect. Rust uses
typed IQ coefficient values instead of exposing this raw helper.

## `get_phy_version_str`

Size `0x4c`. Strict status: **NO-REGISTER-EFFECT**.

The body calls `phy_get_rf_cal_version`, which is the complete six-byte
constant-return leaf `return 100`. It formats that number and the fixed strings
`"db88322f"`, `"Jun 12 2026"` and `"18:28:10"` into the 40-byte
`phy_version_str` object, then returns the object address. The complete call
graph performs no MMIO. Rust does not retain this vendor build-string API.

## `phy_version_print`

Size `0x4a`. Strict status: **NO-REGISTER-EFFECT**.

The function obtains the same constant RF-calibration version, reads
`phy_param[0x1a9]` and `_rom_eco_version`, and tail-calls `phy_printf` with
the fixed vendor build strings. It only reads software data. Diagnostic
formatting is absent from Rust and has no radio-register effect.

## `phy_reg_check`

Size `0x3d2`. Strict status: **NOT-PORTED**.

The body contains 21 branch-back loops. Every iteration performs one volatile
32-bit load, prints the block-relative offset and loaded value, increments the
address by four, and compares it with the exclusive end. There are no writes.
The complete order, bounds and load counts are:

| Order | Printed block | Half-open address range | Reads |
| ---: | --- | --- | ---: |
| 1 | `pmu` | `[0x20704000, 0x20704228)` | 138 |
| 2 | `i2c_mst` | `[0x2010f800, 0x2010f840)` | 16 |
| 3 | `FECTRL` | `[0x20100800, 0x201008e4)` | 57 |
| 4 | `FECOEX` | `[0x20100000, 0x20100058)` | 22 |
| 5 | `FEDATA` | `[0x20100400, 0x2010048c)` | 35 |
| 6 | `FEDATA_WIFI` | `[0x20100c00, 0x20100c58)` | 22 |
| 7 | `MODEM_SYSCON` | `[0x20109c00, 0x20109c34)` | 13 |
| 8 | `MODEM_LPCON` | `[0x2010f000, 0x2010f05c)` | 23 |
| 9 | `agc` | `[0x20107000, 0x2010720c)` | 131 |
| 10 | `bb` | `[0x20107c00, 0x20107d50)` | 84 |
| 11 | `bbtx` | `[0x20107400, 0x20107480)` | 32 |
| 12 | `brx` | `[0x20108000, 0x2010809c)` | 39 |
| 13 | `nrx` | `[0x20107800, 0x20107a44)` | 145 |
| 14 | `btagc` | `[0x20102800, 0x2010290c)` | 67 |
| 15 | `bt_v3_2` | `[0x20102000, 0x20102200)` | 128 |
| 16 | `btmac` | `[0x20101400, 0x20101fb8)` | 750 |
| 17 | `802154_reg` | `[0x20103000, 0x201031a4)` | 105 |
| 18 | `zbbb` | `[0x20102c00, 0x20102c4c)` | 19 |
| 19 | `LP_APM` | `[0x20706c00, 0x20706d14)` | 69 |
| 20 | `clkrst` | `[0x20701000, 0x2070106c)` | 27 |
| 21 | `lpperi` | `[0x20710000, 0x2071002c)` | 11 |

That is exactly 1933 ordered MMIO loads. The recovered SVD records the radio
block portions as finite evidence ranges; it does not imply that every word is
safe to read in an arbitrary live state. Rust has typed access to many of the
individual blocks, but no operation reproduces this complete ordered dump.

## `phy_i2c_check`

Size `0x1f6`. Strict status: **NOT-PORTED**.

The function performs ten sequential byte-index loops. Each iteration calls
`phy_i2c_readReg(block, host, index)` and prints the returned value. The exact
order and half-open index ranges are:

| Order | Logical bank | Block | Host | Register indices | Reads |
| ---: | --- | ---: | ---: | --- | ---: |
| 1 | BBTOP | `0x67` | 1 | `[0, 57)` | 57 |
| 2 | TXRF | `0x6b` | 1 | `[1, 22)` | 21 |
| 3 | RFPLL_SDM | `0x63` | 1 | `[0, 7)` | 7 |
| 4 | RFPLL | `0x62` | 1 | `[0, 22)` | 22 |
| 5 | BIAS | `0x6a` | 1 | `[0, 4)` | 4 |
| 6 | BBPLL | `0x66` | 0 | `[0, 11)` | 11 |
| 7 | ULP | `0x61` | 1 | `[0, 11)` | 11 |
| 8 | SAR1 | `0x10` | 0 | `[0, 7)` | 7 |
| 9 | PERIF | `0x69` | 0 | `[0, 13)` | 13 |
| 10 | DIG_REG | `0x6d` | 0 | `[0, 15)` | 15 |

The trace therefore has exactly 168 PHY-I2C reads. Rust has no diagnostic
owner for this sequence. Its typed `PhyI2cAddress` also rejects block `0x10`
and the other low block IDs already identified by the complete I²C audit, so
the sequence cannot be reconstructed through the current public typed
surface.

## `phy_tx_gain_print`

Size `0x1ee`. Strict status: **NO-REGISTER-EFFECT**.

The first indirect call is `g_phyFuns[9]`,
`phy_wifi_get_tx_tab_new(phy_param[0x11c], wifi_diggain, wifi_bbgain,
wifi_pagain, 0)`. The second is `g_phyFuns[10]`,
`phy_bt_get_tx_tab_new(bt_pagain, bt_bbgain, bt_diggain, 0)`. Both callback
graphs were completely audited with `phy_tx_gain.o` and the corresponding ROM
TX-gain leaves; they only calculate caller-buffer data and optionally print.

The remaining body prints, in order:

1. 18 signed bytes from `phy_param[0x50..0x62)` as Wi-Fi initial power;
2. 32 Wi-Fi PA-gain halfwords, 32 BB-gain halfwords and 32 digital-gain bytes;
3. signed correction byte `phy_param[0x123]`;
4. 16 BT PA-gain halfwords, 16 BB-gain halfwords and 16 digital-gain bytes;
5. signed correction byte `phy_param[0x124]`.

There is no direct or transitive register access. Rust owns the Wi-Fi
calculation but omits this diagnostic formatter and has no BT table generator.

## `phy_get_vdd33`

Size `0x88`. Strict status: **NOT-PORTED**.

The complete child-call and hardware order is:

1. `phy_pbus_debugmode()`;
2. `phy_pbus_force_test(5, 1, 0x80)`;
3. `phy_i2c_writeReg_Mask(0x6b, 1, 0x13, 7, 7, 1)`;
4. sample `phy_get_sar2_vol(3)`;
5. `phy_i2c_writeReg_Mask(0x6b, 1, 0x13, 7, 7, 0)`;
6. `phy_pbus_force_test(5, 1, 0)`;
7. `phy_pbus_workmode()`.

The saved unsigned sample is multiplied by 230, divided unsigned by 100, and
truncated to 16 bits for the return value. Rust has shared PBus, masked-I²C and
SAR primitives, but no VDD33 operation that composes this exact setup, sample
and cleanup trace.

## `rfpll_cap_check`

Size `0x100`. Strict status: **NOT-PORTED**.

For each channel from 1 through 14, inclusive, the body performs:

1. `phy_chip_set_chan(channel, 0)`;
2. `phy_read_pll_cap()`, saved as a halfword;
3. `phy_i2c_readReg(0x62, 1, 6)`, saved as a byte.

A zero argument returns after the 14 iterations. A nonzero argument then
prints all fourteen capacitor values and all fourteen RFPLL register-six
values. The function does not save or restore the entry channel: after either
branch, the programmed channel is 14. No Rust root performs this 14-channel
destructive diagnostic sweep.

## `phy_cal_print`

Size `0x5fa`. Strict status: **NOT-PORTED**.

The prefix repeats `ets_delay_us(1000)` followed by
`phy_read_hw_noisefloor()` exactly three times and saves the three results.
It then calls, in order, `phy_version_print`, `phy_get_vdd33` and
`phy_tsens_temp_read`. The central body only reads and formats calibration
state from `phy_param`; its IQ halfwords are decoded through the pure
`phy_get_iq_value` helper.

The hardware-mutating suffix is not optional:

1. call `phy_wifi_set_tx_gain_new(phy_param[0x11c], 0)`, including its
   conditional 32-entry gain-memory publication;
2. print `phy_param[0x1a]` and `[0x1c]`;
3. call `phy_tx_gain_print`;
4. print bytes `phy_param[0x19e..=0x1a0]`.

Consequently this is not merely a formatter. It reaches the three
noise-floor reads, the complete VDD33 PBus/I²C/SAR trace, temperature-sensor
hardware and Wi-Fi gain publication. Rust deliberately has no aggregate
diagnostic root with this transaction order.

## `phy_pbus_print`

Size `0xf4`. Strict status: **NOT-PORTED**.

The body calls `phy_pbus_rd` exactly eleven times, in this order:

```text
(0,1), (0,2),
(4,1), (4,2), (5,1),
(1,1), (1,2),
(2,1), (3,1), (2,2), (3,2)
```

Formatting calls occur after the second, fifth, seventh and eleventh reads.
The recovered PBus PAC exposes the result windows used by the ROM child, but
Rust has no entry point that preserves this complete selector/path sequence
and its eleven hardware reads.

## `phy_debug_print_line`

Size `0x48`. Strict status: **NO-REGISTER-EFFECT**.

The body reads halfwords `phy_param[0x1a..=0x1d]`, prints them, then prints its
two input values. There are no register accesses or register-relevant child
calls.

## Member conclusion

This member adds no Rust register-parity success. It closes six software-only
diagnostic helpers as **NO-REGISTER-EFFECT** and exposes six absent hardware
diagnostics as **NOT-PORTED**. The most important finite evidence is the
1933-load direct-MMIO dump, the 168-read logical-I²C dump, the eleven-read
PBus selector sequence, the VDD33 setup/cleanup trace and the destructive
14-channel RFPLL sweep.
