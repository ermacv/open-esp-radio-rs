# Revision-zero ROM CBW, feature and watchdog leaf audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to fifteen revision-zero ROM functions
at `0x2f828220..0x2f82873a`. Addresses refer to
`_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_wifi_enable_set` | `0x2f828220` | `0x18` | BODY-AUDITED | Correct Boolean platform operation; physical target RMW remains external |
| `phy_bb_cbw_chan_cfg` | `0x2f828238` | `0x74` | MISMATCH | Byte domain matches; full-word high-path arithmetic and unmasked first OR do not |
| `phy_vht_support` | `0x2f8282ac` | `0x1a` | NOT-PORTED | Missing bit-five replacement |
| `phy_csidump_force_lltf_cfg` | `0x2f8282c6` | `0x1c` | NOT-PORTED | Missing bit-fifteen replacement |
| `phy_hemu_ru26_good_res` | `0x2f8282e2` | `0x24` | NOT-PORTED | Missing ordered set-bit-24/clear-bit-25 RMWs |
| `phy_freq_band_reg_set` | `0x2f828306` | `0x1c` | NOT-PORTED | Missing inverse band bit and VHT tail |
| `phy_sifs_reg_init` | `0x2f828532` | `0x44` | NOT-PORTED | Missing three-RMW fixed SIFS initialization |
| `phy_bbtx_outfilter` | `0x2f828576` | `0x3e` | NOT-PORTED | Missing three input-bit replacements |
| `phy_bb_wdt_rst_enable` | `0x2f8285b4` | `0x1c` | MISMATCH | Rust implements only the set branch during watchdog initialization |
| `phy_bb_wdt_int_enable` | `0x2f8285d0` | `0x20` | NOT-PORTED | Missing general interrupt-enable replacement |
| `phy_bb_wdt_timeout_clear` | `0x2f8285f0` | `0x14` | NOT-PORTED | Missing timeout-clear set edge |
| `phy_bb_wdt_get_status` | `0x2f828604` | `0x0a` | NOT-PORTED | Missing standalone full-word status read |
| `phy_bb_dcmem_clr` | `0x2f8286b4` | `0x1c` | MATCHED | Exact fresh-read set/clear pulse |
| `phy_i2c_txrate_init` | `0x2f8286d0` | `0x38` | MATCHED | Exact two RMWs and installed archive gain-restore child |
| `phy_lltf_mask_en` | `0x2f828708` | `0x32` | NOT-PORTED | Missing two fresh input-bit replacements |

All fifteen direct bodies, branches and direct or indirect child bindings
were inspected. `phy_wifi_enable_set` remains open only because
`PhyWifiBbControl` has no physical target implementation in this repository.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_wifi_enable_set,phy_bb_cbw_chan_cfg,phy_vht_support,phy_csidump_force_lltf_cfg,phy_hemu_ru26_good_res,phy_freq_band_reg_set,phy_sifs_reg_init,phy_bbtx_outfilter,phy_bb_wdt_rst_enable,phy_bb_wdt_int_enable,phy_bb_wdt_timeout_clear,phy_bb_wdt_get_status,phy_bb_dcmem_clr,phy_i2c_txrate_init,phy_lltf_mask_en \
  _oracles/esp32s31_rev0_rom.elf
```

## `phy_wifi_enable_set`

ROM freshly reads `0x20109c18` and either sets bit one for every nonzero input
or clears it for input zero. Rust preserves the two input equivalence classes
through `PhyWifiBbControl::set_wifi_baseband_enabled(bool)`. The external
target backend still prevents proof that the call performs exactly one fresh
RMW of this physical word.

## `phy_bb_cbw_chan_cfg`

The function derives four values from the full input word.

When `input >> 4` is nonzero, it subtracts one and zero-extends the low byte.
That byte becomes both the TX-offset source and the low two-bit control
source. When `input >> 4` is zero, it uses the low nibble: values below two
normalize to zero, other values subtract two. Two additional Boolean fields
record whether the low nibble and `input & 0x0e` are nonzero.

ROM then makes four RMWs:

1. clear bits 3:0 of `0x20104400` and OR the full normalized byte;
2. replace bits 1:0 of `0x20107ce0`;
3. replace bits 4:2 of `0x20107ce4`;
4. freshly replace bits 1:0 of `0x20107ce4`.

Rust `channel_cbw_fields(u8)` matches every byte input and retains four
separate PAC operations. The standalone ROM accepts a full word. Its high
path can derive a byte above `0x0f`, and the first OR is not masked back to
the cleared nibble, so bits 7:4 can be set. Rust both narrows the input to
`u8` and writes through a four-bit field. Complete-domain status is
MISMATCH.

## VHT, CSI, HE and frequency-band controls

`phy_vht_support` replaces bit 5 of `0x20107ce4` with `input & 1`.

`phy_csidump_force_lltf_cfg` replaces bit 15 of `0x201070a4` with
`input & 1`.

`phy_hemu_ru26_good_res` freshly sets bit 24 of `0x20107890`, then freshly
clears bit 25.

`phy_freq_band_reg_set` replaces bit 5 of `0x20107030` with
`!(input & 1)`, then tail-calls `phy_vht_support(input)` to publish the
non-inverted low bit at the second register.

No Rust PHY feature owner implements these ROM operations. MAC-side VHT/HE
configuration uses different peripherals and is not transaction evidence for
these PHY addresses.

## `phy_sifs_reg_init`

ROM performs:

1. at `0x20104c54`, AND `0x801fffff`, OR `0x1d400000`, write;
2. at `0x20104c58`, AND `0xffe003ff`, OR `0x000ee000`, write;
3. freshly read `0x20104c58`, clear bits 9:0, OR `0x0f0`, write.

No Rust leaf owns this fixed initialization trace.

## `phy_bbtx_outfilter`

ROM makes three fresh RMWs of `0x20107440`, replacing bit 5 from input zero,
bit 6 from input one and bit 4 from input two. Each source is reduced to its
low bit by the shift/mask pair. The complete three-input operation is absent.

## Baseband watchdog controls

All four functions use the watchdog block:

- `phy_bb_wdt_rst_enable` freshly replaces bit 31 of `0x20107c40` with the
  low input bit;
- `phy_bb_wdt_int_enable` freshly replaces bit 30 of the same word;
- `phy_bb_wdt_timeout_clear` freshly sets bit 29 of that word;
- `phy_bb_wdt_get_status` returns one full read of `0x20107c08`.

Rust `configure_baseband_watchdog` sets bit 31 while applying the cold fixed
configuration, so it matches only the nonzero branch of
`phy_bb_wdt_rst_enable`. It exposes no clear branch, interrupt control,
timeout-clear edge or standalone status read.

## `phy_bb_dcmem_clr`

ROM freshly reads `0x2010703c`, sets bit 20 and writes, then freshly reads
again, clears bit 20 and writes. Rust `clear_agc_dc_memory` preserves the two
distinct pulse edges exactly.

## `phy_i2c_txrate_init`

ROM freshly updates `0x2010448c` twice:

1. clear bits 25:18 with `0xfc03ffff`, OR `0x01540000`;
2. freshly clear bits 1:0, set them to `0b10`.

It then loads callback-table slot `0x30` and tail-calls it with input one.
Archive `phy_get_romfunc_addr` replaces this slot with
`phy_txgain_comp_pacfg_new`, whose four-RMW restore bytes are
`[00,fa,ff,00]`.

Rust `configure_i2c_tx_rate` performs the same two rate RMWs and immediately
executes the already matched archive gain-restore trace. It does not call the
older ROM `phy_txgain_comp_pacfg_`, whose different bytes are documented
separately.

## `phy_lltf_mask_en`

ROM makes two fresh RMWs of `0x2010790c`: first replace bit 13 from the low
bit of input zero, then freshly replace bit 12 from the low bit of input one.
No Rust PHY leaf implements these controls.
