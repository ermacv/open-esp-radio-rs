# Revision-zero ROM FBW, state and force-control leaf audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to twelve revision-zero ROM functions
at `0x2f827e38..0x2f828220`. Addresses refer to
`_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_wifi_fbw_sel` | `0x2f827e38` | `0x58` | MATCHED | Exact three-RMW zero/nonzero branches |
| `phy_bt_filter_reg` | `0x2f827e90` | `0x34` | MATCHED | Exact three fresh-read RMWs |
| `phy_rx_sense_set` | `0x2f827ec4` | `0x40` | NOT-PORTED | Missing four-RMW general RX-sense control |
| `phy_tx_state_set` | `0x2f827f04` | `0x4c` | NOT-PORTED | Missing four-register TX-state publication |
| `phy_close_pa` | `0x2f827f50` | `0x5e` | NOT-PORTED | Missing both ordered three-RMW PA branches |
| `phy_set_pbus_reg` | `0x2f8280a6` | `0x32` | NOT-PORTED | Rust saves the six words but has no inverse restore operation |
| `phy_wifi_rifs_mode_en` | `0x2f8280d8` | `0x14` | NOT-PORTED | Missing bit-zero replacement |
| `phy_nrx_freq_set` | `0x2f8280ec` | `0x32` | MISMATCH | Reads/order match; Rust narrows the divisor and uses unsigned division |
| `phy_fe_adc_on` | `0x2f82811e` | `0x5e` | NOT-PORTED | Missing zero branch and delayed nonzero three-edge sequence |
| `phy_force_pwr_index` | `0x2f82817c` | `0x3a` | NOT-PORTED | Missing two-RMW force/index publication |
| `phy_fft_scale_force` | `0x2f8281b6` | `0x3e` | NOT-PORTED | Missing three-RMW scale/force sequence |
| `phy_force_rx_gain` | `0x2f8281f4` | `0x2c` | NOT-PORTED | Missing high-byte gain and bit-23 force RMWs |

All twelve direct bodies, branches, input-dependent masks and delays were
inspected. No register-relevant child remains open in this group.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_wifi_fbw_sel,phy_bt_filter_reg,phy_rx_sense_set,phy_tx_state_set,phy_close_pa,phy_set_pbus_reg,phy_wifi_rifs_mode_en,phy_nrx_freq_set,phy_fe_adc_on,phy_force_pwr_index,phy_fft_scale_force,phy_force_rx_gain \
  _oracles/esp32s31_rev0_rom.elf
```

## FBW and BT-filter leaves

Both functions repeatedly update `0x20100874`.

`phy_wifi_fbw_sel` first clears its two one-bit FBW controls with mask
`0xfff6ffff`. It freshly clears the middle two-bit field, then writes value
one to that field for every nonzero input or leaves zero for input zero. It
freshly does the same to the high two-bit field. The branch-independent first
write and the two branch-dependent writes are always separate.

Rust `configure_bss_cbw_suffix` performs the same three PAC `modify`
operations. Its `cbw != 0` conversion preserves the ROM leaf's only two
register-result classes.

`phy_bt_filter_reg` freshly sets bit 25, freshly clears bit 22 with mask
`0xffbfffff`, then freshly clears bits 24:23 with mask `0xfe7fffff`.
The recovered fields used by `configure_bt_filter` produce the same three
intermediate words and order.

## `phy_rx_sense_set`

The unrestricted input word controls four RMWs:

1. at `0x20107010`, preserve bits 22:0 and OR `input << 23`;
2. repeat that transform at `0x20107014`;
3. at `0x20107044`, clear the low byte and OR the full input word;
4. at `0x20107108`, set bit 9 when the input is zero, otherwise clear it.

The third operation deliberately has no mask on the input before its OR, so
arbitrary high input bits can modify fields outside the low byte. No Rust
PHY/PAC/HAL owner implements this complete function.

## `phy_tx_state_set`

ROM performs one RMW at each of `0x20100834`, `0x20100838`,
`0x2010083c` and `0x20100840`. The first three use common preserve mask
`0x3f3f3f3f`; the first inserts fixed image `0x00404000`, while the second
and third insert input-derived images `input << 30` and
`(input << 6) & 0xc0`. The fourth clears bits 7:6.

There is no PHY register implementation. The MAC crate's TX-state definitions
refer to a different MAC surface and do not reproduce these four ROM
transactions.

## `phy_close_pa`

For every nonzero input, ROM:

1. replaces bits 11:10 of `0x20100890` with `0b10`;
2. freshly clears bit 2 of `0x2010088c`;
3. freshly clears bit 4 of `0x2010088c`.

For input zero, it reverses the ordering across the two registers:

1. set bit 2 of `0x2010088c`;
2. freshly set bit 4 of that word;
3. clear bits 11:10 of `0x20100890`.

Rust has no operation for either branch. Preserving only the final values
would not be enough because the intermediate write ordering differs by
branch.

## `phy_set_pbus_reg`

ROM loads the global `phy_param` pointer, reads six consecutive words at
offsets `0x30..0x44`, and writes them without masking to
`0x20100854`, `0x20100858`, `0x2010085c`, `0x20100860`,
`0x20100864` and `0x20100868`.

Rust implements the opposite `phy_save_pbus_reg` direction: it reads these
six registers and stores them in owned parameter state. It has no inverse
six-load/six-store restore leaf, so `phy_set_pbus_reg` is NOT-PORTED.

## `phy_wifi_rifs_mode_en`

ROM freshly reads `0x201070f4`, replaces bit zero with `input & 1`, and
writes once. No Rust radio owner performs this update.

## `phy_nrx_freq_set`

ROM reads `0x20107848` twice. The first sample supplies its high byte as an
RV32 shift count; the second sample supplies the high byte preserved in the
final word. It computes:

```text
numerator = 0x50 sll (first_sample >> 24)
quotient = signed_RV32_div(numerator, input)
result = (second_sample & 0xff000000) | (quotient & 0x000fffff)
```

It then performs one unrestricted full-word write. RV32 register shifts use
the low five bits of the count. Signed RISC-V division by zero returns
`0xffffffff`; signed overflow has the architecture-defined dividend result.

Rust preserves two distinct reads and the final write, and `wrapping_shl`
matches the shift-count rule. It accepts only a nonzero `u16` frequency and
uses unsigned `u32` division. Therefore:

- input zero produces a vendor register write with low image `0x000fffff`,
  while Rust asserts before writing;
- a shifted numerator with bit 31 set follows signed division in ROM and
  unsigned division in Rust;
- full-word divisors outside `u16` cannot be represented.

The reached positive-frequency/small-shift profiles may produce the same
image, but complete-function status is MISMATCH. These differences are not a
documented vendor-defect exception.

## `phy_fe_adc_on`

Both branches update bits 9:8 of `0x20100890`.

Input zero makes one RMW: clear both bits and set image `0b10`. For every
nonzero input, ROM:

1. clear both bits, set `0b10`, write;
2. delay one microsecond;
3. freshly set both bits to `0b11`, write;
4. delay one microsecond;
5. freshly clear both bits to `0b00`, write.

No Rust operation composes either the one-write branch or the delayed
three-write branch.

## Force-control leaves

`phy_force_pwr_index` makes two fresh RMWs of `0x20100408`: first replace
bit 23 with the low bit of input zero, then replace bits 22:17 with the low
six bits of input one.

`phy_fft_scale_force` makes three fresh RMWs of `0x20107800`: replace bits
27:20 with `input1 << 20`, freshly clear bit 19, then freshly replace bit 19
with the low bit of input zero. The first input insertion is not masked before
its OR beyond the preceding destination-field clear.

`phy_force_rx_gain` makes two fresh RMWs of `0x2010702c`: replace its high
byte with the low byte of input one, then freshly replace bit 23 with the low
bit of input zero.

None of these exact force operations exists in the Rust PHY, HAL or PAC
transaction layer.
