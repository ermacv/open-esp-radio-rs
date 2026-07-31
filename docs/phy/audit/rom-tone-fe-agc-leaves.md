# Revision-zero ROM tone, front-end and AGC leaf audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to seven adjacent revision-zero ROM
functions. Addresses refer to `_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_dac_scale_set` | `0x2f82873a` | `0x3c` | MATCHED | Both zero and nonzero register traces are present in the Rust tone owners |
| `phy_start_tx_tone_step` | `0x2f828776` | `0x102` | MISMATCH | Rust matches the reached enabled first-path profile, not the complete six-word input domain or zero/zero branch |
| `phy_stop_tx_tone` | `0x2f828878` | `0x30` | MATCHED | Exact five-RMW cleanup trace |
| `phy_fe_reg_update` | `0x2f8288a8` | `0x36` | MISMATCH | Rust implements the archive three-RMW variant and omits this ROM function's two-RMW DAC tail |
| `phy_txgain_comp_pacfg_` | `0x2f8288de` | `0x66` | MISMATCH | Zero branch matches; the four nonzero byte images differ from the Rust/archive implementation |
| `phy_rfrx_sat_rst` | `0x2f828944` | `0x42` | MATCHED | Exact full store and both two-RMW branches |
| `phy_force_rx_gain_trig` | `0x2f828986` | `0x4e` | NOT-PORTED | Conditional three-RMW, one-microsecond pulse is absent |

All seven bodies, branches, direct calls and MMIO operations were inspected.
The similarly named ROM and archive functions are treated as separate
oracles: a match against `libphy.a[phy_reg.o]` is not automatically a match
against the older ROM body.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_dac_scale_set,phy_start_tx_tone_step,phy_stop_tx_tone,phy_fe_reg_update,phy_txgain_comp_pacfg_,phy_rfrx_sat_rst,phy_force_rx_gain_trig \
  _oracles/esp32s31_rev0_rom.elf
```

## `phy_dac_scale_set`

The input is reduced to two equivalence classes: zero becomes byte `0x00`,
and every nonzero word becomes byte `0xff`. ROM then performs two separate
fresh-read RMWs of `0x20100c04`:

1. replace bits 23:16 with the selected byte;
2. freshly read the result and replace bits 15:8 with the same byte.

The masks are respectively `0xff00ffff` and `0xffff00ff`. Rust performs the
same two ordered PAC `modify` operations. `configure_power_control_tone`
supplies the zero image and `stop_power_detector_tone` supplies the nonzero
image. The Rust API does not need the same standalone word-valued function:
all vendor input words collapse to the two represented transaction traces.

## `phy_start_tx_tone_step`

The six input words are
`(enable0, selector0, step0, enable1, selector1, step1)`. ROM first computes
`combined_enable = enable0 | enable1` and the low bytes of the wrapping
negations of both steps.

When `combined_enable != 0`, it performs this prefix:

1. freshly read `0x2010040c`, clear bits 1:0 and write;
2. call `phy_dac_scale_set(0)`, producing the two RMWs above;
3. call the function-table target at offset `0x30` with input zero.

It then always performs four fresh-read RMWs:

1. replace bits 1:0 of `0x20100428` with `selector0 & 3`;
2. replace bits 3:2 of the same word with `(selector1 << 2) & 0x0c`;
3. preserve the high nibble of `0x2010041c` and replace its low 28 bits with
   `(enable0 << 18) | (selector0 srai 2) |
   ((u8)(-step0) << 10)`;
4. do the corresponding update of `0x20100420` from path-one inputs.

All operations before the final `0x0fffffff` mask use full RV32 words.
In particular, the selector shift is arithmetic and an arbitrary enable word
can affect more than the Boolean arm bit.

When `combined_enable != 0`, ROM now returns and deliberately leaves DAC scale
and gain compensation disabled. When both enable words are zero, it instead:

1. delays five microseconds;
2. freshly reads `0x2010040c`, sets bits 1:0 and writes;
3. calls the function-table target at offset `0x30` with input one;
4. tail-calls `phy_dac_scale_set(1)`.

Rust `configure_power_control_tone(selector, step)` reproduces the complete
enabled trace for the reached first-path selector/step values, including
`(1, 0x80, 0x50, 0, 0, 0)`, provided the function-table target is the
installed archive gain-compensation leaf. It
has no second-path inputs, converts the first enable to `bool`, uses a
logical shift of a `u16` selector, and does not implement the zero/zero
branch as one exact operation.

The TX-DC owner's zero path is not an exact substitute. It composes the
archive tone helper and ROM stop helper, thereby adding two arm-bit clears
after restoring the archive gain bytes. The complete standalone ROM function
is therefore a mismatch even though its reached nonzero first-path profile is
represented.

## `phy_stop_tx_tone`

This function has one unconditional trace:

1. freshly read `0x2010041c`, clear bit 18 with mask `0xfffbffff`, write;
2. freshly read `0x20100420`, clear bit 18 with the same mask, write;
3. freshly read `0x2010040c`, set bits 1:0, write;
4. tail-call `phy_dac_scale_set(1)`, which freshly writes DAC bits 23:16;
5. freshly read the DAC register again and write bits 15:8.

`stop_power_detector_tone` preserves all five reads, writes, masks and order.
The constant one is an argument to the DAC child; `phy_stop_tx_tone` itself
does not branch on a caller input.

## `phy_fe_reg_update`

The ROM function performs:

1. a fresh RMW setting bit 25 of `0x20100c08`;
2. another fresh RMW setting bit 26 of that word;
3. a fresh RMW setting bits 1:0 of `0x20100448`;
4. a tail-call to `phy_dac_scale_set(1)`, adding two RMWs of `0x20100c04`.

The first three operations are identical to the archive
`libphy.a[phy_reg.o]::phy_fe_reg_update`. Rust `update_front_end` deliberately
implements that archive body because it is the target installed and called
by the current initialization graph. It has no operation representing the
longer same-named ROM function, so the ROM leaf is a strict mismatch rather
than evidence against the already matched archive row.

## `phy_txgain_comp_pacfg_`

For input zero, ROM performs full-word zero stores to `0x20100410` and then
`0x20100414`. Rust `clear_tx_gain_compensation` matches both stores.

For every nonzero input, ROM makes four separate fresh-read RMWs of
`0x20100410`. It replaces bytes from least to most significant with:

```text
fd f8 fd fb
```

The exact masks are `0xffffff00`, `0xffff00ff`, `0xff00ffff` and
`0x00ffffff`.

The installed archive replacement
`phy_txgain_comp_pacfg_new` uses:

```text
00 fa ff 00
```

Rust `restore_tx_gain_compensation` correctly implements the archive bytes,
not the older ROM bytes. Consequently the ROM nonzero branch is not matched.
This is not a proven vendor error; it is a versioned behavioural difference
between the two pinned vendor artifacts.

## `phy_rfrx_sat_rst`

ROM first writes the full word `0x00000404` to `0x20107068`. It then freshly
reads `0x2010705c`.

For every nonzero input:

1. OR `0xd1080000` into that sampled word and write it;
2. freshly read again, clear bits 18:0, set them to `0x0800`, and write.

For input zero:

1. AND the sampled word with `0x2ef7ffff` and write it;
2. freshly read again, clear bits 18:0, set them to `0x0400`, and write.

Rust `configure_rf_rx_saturation(bool)` represents the same zero/nonzero
classes. Its first PAC `modify` covers precisely the discontiguous high mask
and its second `modify` is a distinct fresh read for the low nineteen bits.
The initial full-word store, intermediate values and order all match.

## `phy_force_rx_gain_trig`

ROM first reads `0x20100884`. If bit zero is clear, it returns with no write.
If the bit is set, it:

1. freshly reads `0x2010702c`, preserves bits 23:0, replaces bits 31:24 with
   `0x46`, and writes;
2. freshly reads the same word, sets bit 23 and writes;
3. delays one microsecond;
4. freshly reads the word, clears bit 23 and writes.

No Rust PHY action, HAL leaf or PAC owner composes this conditional trigger.
The addresses exist only as generic recovered register identities. The
function is therefore NOT-PORTED, not a vendor-defect exception.
