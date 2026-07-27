# ESP32-S31 radio register source

`esp32s31-radio.svd` is the editable machine-readable source for the recovered
radio clock, reset, power, PHY-PBus and PHY-I2C PAC. Run:

```console
tools/generate-esp32s31-radio-pac.py
```

The generated file is
`crates/open-esp-radio-pac-esp32s31/src/power.rs`. The source-only audit runs
the generator with `--check`, so a direct edit of generated Rust fails CI.

## Evidence policy

Descriptions use `SOURCE[...]` and `CONFIDENCE[...]` tags. Confidence has the
following meaning:

- `exact-s31-layout`: field name, offset and width come from an ESP32-S31
  register structure or SVD;
- `instruction-exact-semantics-unknown`: the complete ROM/blob instruction
  body proves the address, mask and operation, but not the hardware meaning;
- `register-exact-fields-unknown`: the S31 structure names the register while
  its internal bit layout is still unknown;
- `mixed-per-field`: individual fields in the register have different evidence.

Unknown and reserved fields are omitted. An `OPAQUE` or `UNKNOWN` name is
intentional: it records usable instruction-level evidence without converting a
neighboring-chip similarity into an S31 fact.

The generator supports CMSIS-SVD `dim`/`dimIncrement` register arrays. The
45-word `PHY_I2C_COMMAND_RAM.COMMAND_MEMORY%s` definition is therefore kept
compact in the source while still generating bounded Rust register
identities for every entry.

## Recovered sources

| Source ID | Basis |
|---|---|
| `S31_MODEM_SYSCON_STRUCT` | Pinned `esp-wifi-sys` commit `2585f278`, S31 `modem_syscon_struct.h`, SHA-256 recorded in the SVD |
| `S31_MODEM_LPCON_STRUCT` | Same commit, S31 `modem_lpcon_struct.h`, SHA-256 recorded in the SVD |
| `S31_PMU_HEADERS` | Official ESP-IDF S31 `pmu_reg.h` pinned to the commit recorded in the SVD, plus local hashed copies in `esp-wifi-sys` |
| `S31_ESP_PACS_SVD` | Local `esp-pacs` commit `f823dd9d`, ESP32-S31 generated SVD |
| `ROM_REV0_PHY_OPEN_FE_BB_CLK` | Complete 0x38-byte rev0 ROM body at `0x2f823ec0` |
| `ROM_REV0_PHY_FE_REG_INIT` | Complete 0xf6-byte rev0 ROM `phy_fe_reg_init` body at `0x2f827740` |
| `ROM_REV0_PHY_PBUS` | Complete PBus mode/force bodies and read address/shift jump tables |
| `ROM_REV0_PHY_SET_PBUS_MEM` | Complete 0x180-byte `phy_set_pbus_mem` parent at `0x2f82479e` |
| `ROM_REV0_PHY_WRITE_PBUS_MEM` | Complete 0x16a-byte `phy_write_pbus_mem` body at `0x2f824634` |
| `ROM_REV0_PHY_SAVE_PBUS_REG` | Complete 0x32-byte `phy_save_pbus_reg` body at `0x2f824602` |
| `ROM_REV0_PHY_WRITE_GAIN_MEM` | Complete 0x2a-byte `phy_write_gain_mem` body at `0x2f8274f0` |
| `ROM_REV0_PHY_CLOCK_FORCE` | Complete force-TX/RX and RX/TX clock-pair bodies |
| `ROM_REV0_PHY_I2C` | Complete clock-select, master-init/reset and host read/write bodies |
| `BLOB_LIBPHY_PHY_I2C` | Complete S31 PHY-I2C callbacks and command-RAM initializer |
| `BLOB_LIBPHY_PHY_SET_TX_CFR_MEM` | Complete 0x76-byte `phy_set_tx_cfr_mem` body from `phy_tx_gain.o` |
| `BLOB_LIBPHY_PHY_SET_TX_GAIN_MEM_NEW` | Complete `phy_set_tx_gain_mem_new` body from `phy_tx_gain.o` and its complete ROM write leaf |
| `BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW` | Complete PMU-based analog-I2C power/reset body |
| `BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK` | Complete 0x20-byte `libphy.a[phy_init.o]` body |
| `BLOB_LIBPHY_PHY_BB_INIT` | Complete 0x16a-byte `phy_bb_init` body and relocation graph |

The public ESP-IDF ESP32-C5/C61 register headers are only cross-chip
validation. They are not accepted as the sole basis for an S31 address or bit.
