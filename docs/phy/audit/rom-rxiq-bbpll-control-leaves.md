# Revision-zero ROM RXIQ, noise-floor and BBPLL control audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to nine revision-zero ROM functions at
`0x2f827c7e..0x2f827e38`. Addresses refer to
`_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_rxiq_set_reg` | `0x2f827c7e` | `0x54` | MATCHED | Exact signed saturation and one-RMW coefficient publication |
| `phy_bb_wdg_test_en` | `0x2f827d16` | `0x26` | NOT-PORTED | Two unrestricted packed full-word stores are absent |
| `phy_noise_floor_auto_set` | `0x2f827d3c` | `0x36` | MATCHED | Exact four fresh-read set-bit RMWs |
| `phy_read_hw_noisefloor` | `0x2f827d72` | `0x1a` | MATCHED | Exact one-read trace and arithmetic are fused into the Rust MAC-facing decode |
| `phy_iq_corr_enable` | `0x2f827d8c` | `0x24` | MATCHED | Exact two fresh-read set-field RMWs |
| `phy_wifi_agc_sat_gain` | `0x2f827db0` | `0x0c` | MATCHED | Exact two unrestricted full-word stores |
| `phy_bbpll_cal` | `0x2f827dbc` | `0x1c` | BODY-AUDITED | Rust action and two encodings exist, but the target MMIO implementation is external |
| `phy_bbpll_recal` | `0x2f827dd8` | `0x1c` | NOT-PORTED | No Rust operation preserves set/read/clear as one contiguous trace |
| `phy_ant_init` | `0x2f827df4` | `0x44` | MATCHED | Exact three fresh-read RMWs and intermediate masks |

All nine direct bodies and their branches were inspected. `phy_bbpll_cal` is
not promoted to MATCHED merely because a trait method has the right Boolean
contract: this repository does not contain the implementation that proves its
read, mask and write.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_rxiq_set_reg,phy_bb_wdg_test_en,phy_noise_floor_auto_set,phy_read_hw_noisefloor,phy_iq_corr_enable,phy_wifi_agc_sat_gain,phy_bbpll_cal,phy_bbpll_recal,phy_ant_init \
  _oracles/esp32s31_rev0_rom.elf
```

## `phy_rxiq_set_reg`

Inputs are a signed coefficient word and a kind word. The function calls the
pure `phy_get_data_sat` child before its single register RMW.

For every nonzero kind, it saturates to `[-31,31]`, freshly reads
`0x20100438`, replaces bits 21:16 with the low six bits and writes. For kind
zero, it saturates to `[-63,63]` and replaces bits 28:22 with the low seven
bits. The exact preserve masks are `0xffc0ffff` and `0xe0400000`.

`PhyRxIqCoverTransition` bounds gain and phase before issuing its register
action. The PAC setters then publish the same six- or seven-bit image with one
fresh RMW. Although Rust carries an `i8` rather than a full RV32 coefficient,
every full-word vendor input collapses through saturation to one of the
represented field images. Unlike the TXIQ leaf documented separately, the
RXIQ transition does perform the required saturation at both extrema.

## `phy_bb_wdg_test_en`

This six-input function has no reads and makes two full-word stores:

```text
0x20107c3c =
    (input3 << 16) | input2 | (input0 << 30) | (input1 << 31)

0x20107c40 =
    (input4 << 31) | (input5 << 29) | 0x40000000
```

The shifts and ORs use unrestricted RV32 inputs; there is no final field mask.
The existing Rust baseband-watchdog function programs a different production
configuration through RMWs. It neither exposes these six inputs nor performs
these full stores, so this test-control function is NOT-PORTED.

## Noise-floor leaves

`phy_noise_floor_auto_set` performs four separate fresh-read RMWs:

1. set bit 23 of `0x20107018`;
2. freshly read the same word and set bit 28;
3. set bit 0 of `0x20107c44`;
4. set bit 0 of `0x20107c50`.

`configure_noise_floor_auto` retains all four `modify` operations and their
order.

`phy_read_hw_noisefloor` freshly reads `0x2010708c`, masks its low twelve
bits, subtracts `0x1000`, sign-extends the low halfword and arithmetically
shifts right by two. It has no write or branch. Rust's
`read_noise_floor_dbm` performs this exact read and first transform, then
immediately fuses the MAC caller's `(quarter_db + 2) >> 2` conversion and
signed-byte retention. Rust does not expose the intermediate quarter-dB
return as a separate API, but the vendor child-plus-caller register trace and
final value are preserved.

## `phy_iq_corr_enable`

ROM freshly reads `0x20100438`, sets bits 30:29 and writes. It then freshly
reads `0x20100c0c`, sets bits 14:13 and writes. Rust
`enable_iq_correction_modes` performs the same two RMWs in the same order
while preserving all coefficient fields.

## `phy_wifi_agc_sat_gain`

The input word is stored without masking to `0x20107064` and then
`0x20107114`. Rust `set_agc_saturation_gain` uses two ordered
`write_with_zero` full-word stores and accepts the same `u32` domain.

## `phy_bbpll_cal`

ROM freshly reads `0x2010f818`, clears bits 3:2, and writes encoded value
`0b10` for every nonzero input or `0b01` for input zero. This is exactly one
RMW in either branch.

The Rust Boolean action preserves the only two register-result equivalence
classes, and the HAL test fixes the two shifted encodings `0x08` and `0x04`.
Execution, however, delegates to
`PhyI2cMasterControl::set_phy_i2c_bbpll_calibration`. No implementation of
that target trait exists in this repository, so the actual fresh read,
preserve mask and write cannot yet be checked. The direct body is closed but
the Rust binding proof remains open.

## `phy_bbpll_recal`

The complete ROM trace is:

1. freshly read `0x2010f818`, clear bits 3:2, set them to `0b10`, write;
2. freshly read `0x2010f818` again and discard the sampled value;
3. tail-call `phy_bbpll_cal(0)`, which performs a third fresh read, clears
   bits 3:2, sets them to `0b01`, and writes.

Rust channel and initialization owners can request the two Boolean
`phy_bbpll_cal` states, but they do not provide this contiguous
write/read/read/write recalibration operation. In particular, omitting the
otherwise unused middle MMIO read violates the strict transaction standard.
The function is NOT-PORTED.

## `phy_ant_init`

ROM performs three fresh-read RMWs:

1. at `0x2010711c`, AND with `0xffffe800`, clearing bits 10:0 and bit 12;
2. at `0x20107030`, AND with `0xffc007ff`, then OR `0x0001a000`;
3. at `0x20107120`, AND with `0x00ff00ff`, then OR `0x1e001e00`.

Rust `configure_agc_antenna` performs the same three reads and writes. Its
field image `0x34` at the second register is shifted by the recovered field
offset to `0x0001a000`; its two `0x1e` fields produce the third ROM image.
