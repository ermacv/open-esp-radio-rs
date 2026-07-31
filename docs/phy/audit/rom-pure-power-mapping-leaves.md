# Revision-zero ROM pure power/mapping helper audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to eight revision-zero ROM helpers
whose complete call graphs have no register access. Addresses refer to
`_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_temp_to_power` | `0x2f825f80` | `0x22` | NO-REGISTER-EFFECT | Pure signed temperature delta/division |
| `phy_byte_to_word` | `0x2f826034` | `0x1e` | NO-REGISTER-EFFECT | Pure four-byte little-endian load |
| `phy_get_rate_fcc_index` | `0x2f826d26` | `0x7e` | NO-REGISTER-EFFECT | Pointer/table reads and four caller-buffer byte stores only |
| `phy_get_chan_target_power` | `0x2f826da4` | `0x98` | NO-REGISTER-EFFECT | Pure 18-byte target clamp and optional FCC child |
| `phy_index_to_txbbgain` | `0x2f826afa` | `0x20` | NO-REGISTER-EFFECT | Pure five-entry halfword lookup |
| `phy_bt_index_to_bb` | `0x2f826b1a` | `0x1c` | NO-REGISTER-EFFECT | Pure three-value index conversion |
| `phy_bt_bb_to_index` | `0x2f826b36` | `0x1c` | NO-REGISTER-EFFECT | Pure inverse conversion with fallback |
| `phy_wifi_get_target_power` | `0x2f8270fa` | `0x22` | NO-REGISTER-EFFECT | Parameter-pointer wrapper around pure target clamp |

Every instruction and the only child edge in these eight bodies was
inspected. Their outputs can influence later parent decisions, but none
directly or transitively calls a register operation.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_temp_to_power,phy_byte_to_word,phy_get_rate_fcc_index,phy_get_chan_target_power,phy_index_to_txbbgain,phy_bt_index_to_bb,phy_bt_bb_to_index,phy_wifi_get_target_power \
  _oracles/esp32s31_rev0_rom.elf
```

## Arithmetic and byte helpers

`phy_temp_to_power(current, reference, selector)` subtracts with RV32
wrapping, sign-extends the low halfword, and selects a signed divisor:

| Condition | Divisor |
| --- | ---: |
| delta positive | `5` |
| delta nonpositive and selector zero | `3` |
| delta nonpositive and selector nonzero | `4` |

It uses signed division truncating toward zero and sign-extends the low result
byte. It touches no memory.

`phy_byte_to_word(pointer)` loads four unsigned bytes at offsets zero through
three and returns their little-endian `u32` composition. It has no store or
child call.

## TX baseband-gain mappings

`phy_index_to_txbbgain(index)` returns fallback `0x80` for every unsigned
index above four. Indices zero through four select exact halfwords:

```text
0000 0080 0100 0020 00a0
```

`phy_bt_index_to_bb` maps index `1 -> 0x80`, `2 -> 0x100`, and every other
word to zero. `phy_bt_bb_to_index` maps `0x80 -> 1`, `0x100 -> 2`, and every
other word to zero. All three functions only calculate a return value.

## FCC and target-power helpers

`phy_get_rate_fcc_index(rate, output, table_a, table_b)` reads three bytes
relative to `table_a + rate`. It chooses a fourth byte from `table_b` using
the complete rate partitions `3..=11`, `0..=2`, or the remaining values.
Each of the four values is unsigned-clamped to `0x54` and stored to
`output[0..4]`. There is no global or MMIO access.

`phy_get_chan_target_power(rate, maximum, output, target, selector,
table_a, table_b)` starts with the constant FCC bytes
`[0x54,0x54,0x54,0x54]`. When `selector == 1`, it replaces those bytes
through `phy_get_rate_fcc_index`. For exactly 18 signed target bytes, it
selects FCC limits over index ranges `0..=1`, `2..=5`, `6..=13` and
`14..=17`, takes the signed minimum with the target, then the signed minimum
with `maximum`, and writes the result byte to the caller buffer.

`phy_wifi_get_target_power` only loads the current `phy_param` pointer and
constructs those arguments from parameter offsets `0x06`, `0x50`, `0x64`,
`0x65` and `0x6e` before tail-calling the pure helper. Rust owns the relevant
maximum, 18 target bytes and regulatory selector in
`PhyTxTargetPowerProfile`; its safety handling for unsupported rate indices
is a parent-domain issue, not a hidden register transaction in this wrapper.
