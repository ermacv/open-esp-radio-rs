# ESP32-S31 radio register source

`esp32s31-radio.svd` is the editable machine-readable source for the recovered
radio clock, reset, power, PHY-PBus, PHY-I2C and AGC PAC. Run:

```console
cargo pac-gen
```

The generated crate source is
`crates/open-esp-radio-svd-esp32s31/src/lib.rs`. The source-only audit runs the
Rust generator with `--check`, so a direct edit of that generated file fails
CI. `open-esp-radio-pac-esp32s31/src/power.rs` is the shrinking compatibility
facade for code not yet moved to the generated API; it must not acquire
official system peripherals already delegated to `esp-hal`.

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
identities for every entry. Official ESP-IDF S31 `modem/reg_base.h` now names
the containing `0x2010_fc00` aperture `DR_REG_I2C_ANA_MST_MEM_BASE`; the
complete pinned PHY blob remains the source for the 45-word count and
block/register/data layout.

## Recovered sources

| Source ID | Basis |
|---|---|
| `ESP_IDF_ESP32S31_MODEM_REG_BASE` | Official ESP-IDF S31 modem partition map pinned to the commit and SHA-256 recorded in the SVD |
| `HIL_OPEN_HE_RATE_CONTROL_ACK_SNR_2026_07_30` | Open HE20 MCS9/LDPC A-MPDU completion, typed ACK-SNR decode, DHCP and zero-loss ICMP qualification on ESP32-S31 rev0 |
| `S31_MODEM_SYSCON_STRUCT` | Pinned `esp-wifi-sys` commit `2585f278`, S31 `modem_syscon_struct.h`, SHA-256 recorded in the SVD |
| `S31_MODEM_LPCON_STRUCT` | Same commit, S31 `modem_lpcon_struct.h`, SHA-256 recorded in the SVD |
| `S31_PMU_HEADERS` | Official ESP-IDF S31 `pmu_reg.h` pinned to the commit recorded in the SVD, plus local hashed copies in `esp-wifi-sys` |
| `S31_ESP_PACS_SVD` | `ermacv/esp-pacs` commit `d02f0b719` (upstream `8fddffd1d` plus the S31 platform work and evidenced PMU access corrections), ESP32-S31 generated SVD |
| `ROM_REV0_PHY_OPEN_FE_BB_CLK` | Complete 0x38-byte rev0 ROM body at `0x2f823ec0` |
| `ROM_REV0_PHY_FE_REG_INIT` | Complete 0xf6-byte rev0 ROM `phy_fe_reg_init` body at `0x2f827740` |
| `ROM_REV0_PHY_PBUS` | Complete PBus mode/force bodies and read address/shift jump tables |
| `ROM_REV0_PHY_SET_PBUS_MEM` | Complete 0x180-byte `phy_set_pbus_mem` parent at `0x2f82479e` |
| `ROM_REV0_PHY_WRITE_PBUS_MEM` | Complete 0x16a-byte `phy_write_pbus_mem` body at `0x2f824634` |
| `ROM_REV0_PHY_SAVE_PBUS_REG` | Complete 0x32-byte `phy_save_pbus_reg` body at `0x2f824602` |
| `ROM_REV0_PHY_WRITE_GAIN_MEM` | Complete 0x2a-byte `phy_write_gain_mem` body at `0x2f8274f0` |
| `ROM_REV0_PHY_AGC` | Complete rev0 ROM AGC enable/disable, 11b, register-init, register-update and RF RX saturation bodies |
| `ROM_REV0_PHY_ANT_INIT` | Complete 0x44-byte rev0 ROM `phy_ant_init` body |
| `ROM_REV0_PHY_CLOCK_FORCE` | Complete force-TX/RX and RX/TX clock-pair bodies |
| `ROM_REV0_PHY_I2C` | Complete clock-select, master-init/reset, BBPLL-calibration and host read/write bodies |
| `BLOB_LIBPHY_PHY_I2C` | Complete S31 PHY-I2C callbacks and command-RAM initializer |
| `BLOB_LIBPHY_PHY_SET_TX_CFR_MEM` | Complete 0x76-byte `phy_set_tx_cfr_mem` body from `phy_tx_gain.o` |
| `BLOB_LIBPHY_PHY_SET_TX_GAIN_MEM_NEW` | Complete `phy_set_tx_gain_mem_new` body from `phy_tx_gain.o` and its complete ROM write leaf |
| `BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW` | Complete PMU-based analog-I2C power/reset body |
| `BLOB_LIBPHY_PHY_REG_UPDATE_NEW` | Complete post-initialization register update, FTM tail, and ROM saturation-gain leaf |
| `BLOB_LIBPHY_PHY_SET_RX_GAIN_TABLE` | Complete RX-gain-table body, including its final two register limit writes |
| `BLOB_LIBPHY_PHY_SET_RX_COMP_NEW` | Complete 0x28-byte RX-compensation body from `phy_reg.o` |
| `BLOB_LIBPHY_PHY_DC_MEM_CLR` | Complete 0x1c-byte DC-memory-clear body from `phy_reg.o` |
| `BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK` | Complete 0x20-byte `libphy.a[phy_init.o]` body |
| `BLOB_LIBPHY_PHY_BB_INIT` | Complete 0x16a-byte `phy_bb_init` body and relocation graph |
| `BLOB_LIBPP_HAL_ANTENNA_INIT` | Complete 0x5e-byte `libpp.a[hal_mac_tx.o]::hal_attenna_init` body; 34 ordered RMW edges across eight reverse-stride bank words and one common word |
| `BLOB_LIBPP_HAL_INIT_TAIL` | Complete `hal_init` offsets 0xcc..0x12a plus the complete hardware-beacon reload and RTC timer-update leaves; OSI offsets cross-checked against pinned esp-wifi-sys and esp-hal |
| `BLOB_LIBPP_HAL_INIT_COEX` | Complete `hal_init` COEX tail and all complete RX, default, TB and beamforming PTI setter leaves |
| `BLOB_LIBCOEX_PTI_TABLE` | Complete `libcoexist.a` PTI getter plus its 48-byte cold-default table; provides the four event values queried by MAC init |
| `BLOB_LIBPP_HAL_HE_INIT_PREFIX` | Complete `hal_he_init` prefix, beamforming init/report-rate children and trigger-based TX init through the TX-power boundary |
| `BLOB_LIBPP_HAL_TX_POWER_INIT` | Complete 43-entry MAC power-table parent plus TB, immediate-response and TB-RU register leaves |
| `ROM_REV0_PHY_GET_MAX_PWR` | Complete rev0 ROM target-power chain that produces each two-byte MAC table entry from live PHY state |
| `BLOB_LIBPP_HAL_HE_INIT_SUFFIX` | Complete post-power `hal_he_init` hardware tail and every reached finite register leaf |
| `BLOB_LIBPP_DBG_READ_TX_POWER` | Complete unconditional diagnostic traversal; 25 discarded PHY queries still have observable ROM MMIO edges |
| `BLOB_LIBPP_HAL_SNIFFER_MISC` | Complete mode-dependent promiscuous miscellaneous-packet policy leaf |
| `OPEN_DRIVER_PROMISCUOUS_RX_FRONTIER` | Working open scan/STA promiscuous boundary used to qualify the two non-vendor cold policy edges |

The public ESP-IDF ESP32-C5/C61 register headers are only cross-chip
validation. They are not accepted as the sole basis for an S31 address or bit.
