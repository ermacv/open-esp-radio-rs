# Revision-zero ROM AGC, CCA and channel-control leaf audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to thirteen revision-zero ROM
functions. Addresses refer to `_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_disable_agc` | `0x2f827460` | `0x10` | MATCHED | Exact one-RMW disable trace |
| `phy_enable_agc` | `0x2f827470` | `0x28` | MATCHED | Exact disable-bit clear followed by two-edge pulse |
| `phy_disable_cca` | `0x2f827498` | `0x32` | NOT-PORTED | Missing two-RMW forced CCA image |
| `phy_enable_cca` | `0x2f8274ca` | `0x26` | NOT-PORTED | Missing two-RMW CCA field clears |
| `phy_rx_filter_mode` | `0x2f82751a` | `0x20` | NOT-PORTED | Missing general four-bit RX-filter mode RMW |
| `phy_bb_bss_cbw40_dig` | `0x2f82753a` | `0x16` | BODY-AUDITED | Boolean abstraction is correct; target MMIO backend remains external |
| `phy_mac_tx_chan_offset` | `0x2f827550` | `0x38` | MISMATCH | Three field images match byte inputs, but Rust narrows the standalone full-word domain |
| `phy_i2cmst_reg_init` | `0x2f8276c4` | `0x22` | BODY-AUDITED | Two abstract platform operations exist; target RMW implementation remains external |
| `phy_bt_gain_offset` | `0x2f8276e6` | `0x5a` | NOT-PORTED | Missing four-RMW BT gain-offset publication |
| `phy_mac_enable_bb` | `0x2f827836` | `0x2a` | BODY-AUDITED | Three ordered abstract platform edges exist; target RMW implementation remains external |
| `phy_bb_wdg_cfg` | `0x2f827860` | `0x2c` | MATCHED | Exact two fresh-read watchdog RMWs |
| `phy_fe_txrx_reset` | `0x2f82788c` | `0x24` | NOT-PORTED | Missing clear/set pulse of bits 26:25 |
| `phy_set_rx_comp_` | `0x2f8278b0` | `0x28` | MISMATCH | ROM writes `0xeb`; Rust follows the archive replacement and writes `0xed` |

All thirteen direct bodies, branch targets and MMIO instructions were
inspected. Three bodies are left open only because the repository owns an
operation trait but not its physical target-register implementation.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn --symbolize-operands \
  --disassemble-symbols=phy_disable_agc,phy_enable_agc,phy_disable_cca,phy_enable_cca,phy_rx_filter_mode,phy_bb_bss_cbw40_dig,phy_mac_tx_chan_offset,phy_i2cmst_reg_init,phy_bt_gain_offset,phy_mac_enable_bb,phy_bb_wdg_cfg,phy_fe_txrx_reset,phy_set_rx_comp_ \
  _oracles/esp32s31_rev0_rom.elf
```

## AGC enable and disable

`phy_disable_agc` freshly reads `0x20107030`, sets bit 29 and writes once.
Rust `set_agc_enabled(false)` performs that exact RMW.

`phy_enable_agc` performs three distinct RMWs:

1. freshly read `0x20107030`, clear bit 29, write;
2. freshly read `0x2010702c`, set bit 23, write;
3. freshly read `0x2010702c` again, clear bit 23, write.

Rust `set_agc_enabled(true)` retains all three reads, writes and their order.
There is no delay between the two pulse edges in either implementation.

## CCA controls

Both functions operate on `0x20104c5c` and use a fresh read for each field.

`phy_disable_cca` first replaces bits 31:30 with `0b10`, then freshly replaces
bits 29:28 with `0b10`. `phy_enable_cca` clears bits 31:30, then freshly
clears bits 29:28. The exact preserve masks are `0x3fffffff` and
`0xcfffffff`.

The MAC crate has unrelated CCA policy controls, but no PHY/PAC/HAL operation
owns this two-field ROM sequence at this address. Both functions are
NOT-PORTED.

## `phy_rx_filter_mode`

ROM freshly reads `0x20100430`, preserves it with `0xffc3ffff`, inserts
`(input << 18) & 0x003c0000` into bits 21:18, and writes once. Thus only the
low four bits of an arbitrary input word affect the register.

No Rust PHY action or lower-layer leaf implements this register update. The
function is NOT-PORTED.

## `phy_bb_bss_cbw40_dig`

ROM freshly reads `0x20109c18`, clears bits 3:2 with `0xfffffff3`, inserts
`(input << 2) & 4`, and writes. Only the low input bit is significant, and
bit 3 is always cleared.

The ROM parent `phy_bb_bss_cbw40` calls this leaf with literal zero when its
own input is zero and literal one otherwise. Rust preserves that reached
Boolean contract through `PhyWifiBbControl::set_bss_cbw_40_digital`.
However, the target implementation containing the required physical RMW is
outside this repository, so standalone transaction parity remains open.

## `phy_mac_tx_chan_offset`

ROM always performs one fresh RMW of `0x20104400`, clearing bits 3:0 and
selecting:

| Full input word | Low-nibble image |
| --- | ---: |
| exactly `2` | `2` |
| exactly `3` | `1` |
| every other word | `0` |

Rust `bss_tx_offset(u8)` and `configure_bss_cbw_prefix` reproduce all byte
inputs and the reached ROM parent first truncates its input with `zext.b`.
The standalone ROM function itself does not truncate. A word such as `0x102`
therefore selects zero in ROM but cannot be represented without becoming
byte `2` in the Rust helper, which selects two. Complete-domain status is
MISMATCH even though the reached parent profile matches.

## `phy_i2cmst_reg_init`

The body performs two fresh RMWs of `0x2010f818`:

1. clear bits 10:9, set them to `0b10`;
2. freshly read and set bit 6.

Rust calls `set_phy_i2c_register_mode(2)` and then
`enable_phy_i2c_register_mode`. The operations are kept separate and the
first call is not optimized into its `debug_assert`. As with the BBPLL mode
leaf, `PhyI2cMasterControl` has no target implementation in this repository,
so the physical addresses, masks and fresh reads are not yet proved.

## `phy_bt_gain_offset`

The low byte of the unrestricted input is published twice to each of two
registers:

1. replace bits 31:24 of `0x20102848`;
2. freshly replace bits 23:16 of `0x20102848`;
3. replace bits 31:24 of `0x20102868`;
4. freshly replace bits 23:16 of `0x20102868`.

The preserve masks are `0x00ffffff` and `0xff00ffff`. Rust has no BT
gain-offset owner or equivalent four-RMW trace, so the function is
NOT-PORTED.

## `phy_mac_enable_bb`

ROM performs three separate fresh RMWs of `0x20109c18`:

1. set bit 28;
2. freshly clear bit 1;
3. freshly set bit 1.

Rust `enable_mac_baseband` preserves the three logical edges and order through
three calls on `PhyWifiBbControl`. The target backend is external, so it is
not yet possible to prove that each call makes precisely one fresh RMW at
the required physical word.

## `phy_bb_wdg_cfg`

ROM freshly reads `0x20107c3c`, ANDs with `0xbfff0000`, ORs
`0x400000aa`, and writes. It then freshly reads `0x20107c40`, sets bit 31 and
writes.

Rust `configure_baseband_watchdog` reproduces the first combined field update
in one PAC `modify`, followed by a second PAC `modify` for the enable bit.
Both intermediate register images and order match.

## `phy_fe_txrx_reset`

ROM freshly reads `0x20100440`, clears bits 26:25 with mask `0xf9ffffff`,
and writes. It freshly reads again, sets both bits with `0x06000000`, and
writes. No Rust leaf owns this exact reset pulse, so it is NOT-PORTED.

## `phy_set_rx_comp_`

The older ROM leaf freshly replaces the low byte of `0x2010702c` with
`0xeb`, then freshly replaces the high byte of `0x201070a0` with `0xeb`.

The installed archive replacement
`libphy.a[phy_reg.o]::phy_set_rx_comp_new` uses `0xed` in both positions.
Rust `configure_rx_compensation` correctly follows that archive body and
therefore differs from the standalone ROM function. The two constants are a
versioned vendor-artifact difference, not a proved vendor defect.
