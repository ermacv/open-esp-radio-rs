# Line audit: revision-zero ROM TX-gain leaves

Artifact:
`_oracles/esp32s31_rev0_rom.elf`. The audited `.text` bytes are identical in
the local and canonical containers recorded by the PHY README.

This page closes seven ROM functions used by the archive TX-gain member.

## Pure functions

The following complete bodies have no MMIO and no register-relevant child:

| Function | Address | Size | Status | Complete behavior |
| --- | ---: | ---: | --- | --- |
| `phy_set_chan_cal_interp` | `0x2f825fac` | `0x4c` | NO-REGISTER-EFFECT | Signed three-point channel interpolation with two five-channel segments and a final `third + 2` branch |
| `phy_get_data_sat` | `0x2f826024` | `0x10` | NO-REGISTER-EFFECT | Signed clamp selected by two comparisons |
| `phy_txbbgain_to_index` | `0x2f826ac8` | `0x32` | NO-REGISTER-EFFECT | Maps `0x80,0x100,0x20,0xa0` to `1,2,3,4`, all other halfwords to zero |
| `phy_get_tx_gain_value` | `0x2f826e3c` | `0x6c` | NO-REGISTER-EFFECT | Bounded bidirectional table-index search and three caller-buffer halfword stores |
| `phy_bt_get_tx_gain` | `0x2f826ea8` | `0x150` | NO-REGISTER-EFFECT | Sixteen-entry BT gain generation; optional child is diagnostic `ets_printf` only |
| `phy_wifi_get_tx_gain` | `0x2f826ff8` | `0x102` | NO-REGISTER-EFFECT | 32-entry Wi-Fi gain generation; optional child is diagnostic `ets_printf` only |

`phy_bt_get_tx_gain` calls only the pure saturation and table-selection
helpers above. `phy_wifi_get_tx_gain` calls only the pure channel
interpolator and table-selection helper. Their loop counts, signed
truncations, byte/halfword output widths and diagnostic branches were
included in the audit.

Rust `calculate_wifi_tx_gain` matches the Wi-Fi arithmetic and fixed tables.
There is no Rust BT equivalent. These statuses report register effect, not
the availability of every pure vendor API.

## `phy_write_gain_mem`

Address `0x2f8274f0`, size `0x2a`. Strict status: **MATCHED**.

The function performs exactly:

1. full 32-bit store of input word zero to `0x20100848`;
2. full 32-bit store of input word one to `0x2010084c`;
3. full 32-bit store of input word two to `0x20100850`;
4. fresh read of `0x20100844`;
5. preserve bits 31:20, clear bits 19:0, insert the byte memory index in
   bits 18:11 and set bit 19;
6. write the resulting command word to `0x20100844`.

`RadioRegisters::program_gain_memory_entry` preserves the three full stores,
single command-register read, mask, index, gain-write bit and final store.
