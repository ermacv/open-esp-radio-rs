# ESP32-S31 radio register outputs

The editable source is the schema-2 project model under
`verification/vendor/targets/esp32s31/registers/`. It is split into one TOML
fragment per logical peripheral and keeps provenance in structured `[[review]]`
records. The same model is loaded directly by the vendor validator and by the
production PAC generator through the Workbench `crates/register-model` crate.

`esp32s31-radio.svd` is the generated, portable CMSIS-SVD representation. It
contains hardware names and semantics, but no validator provenance tags,
discovery paths or target code-generation extensions. Do not edit it directly.
Run:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

This materializes the SVD from the project model, validates the reviewed
`registers/api.toml` transaction pack, and generates the internal
`driver/chips/esp32s31/pac-raw/src/lib.rs`, plus the reviewed public domains in
`driver/chips/esp32s31/pac/src/generated.rs`. Workbench `project publish
--check` verifies the SVD, raw PAC, closed-PAC module and binding index and
rejects invalid or stale reviewed API references. A direct edit of a generated
file therefore fails CI. Application and HAL code use the separate closed
`driver/chips/esp32s31/pac` crate and cannot import raw register pointers.

`verification/vendor/targets/esp32s31/registers/api.toml` is target-owned
reviewed policy, not CMSIS-SVD. It describes qualified compound transactions,
constrained register-image helpers and ownership partitions. The adjacent
evidence catalogs retain their provenance. Workbench validates every API
register/field reference against the generic model before emitting Rust, so
generic register/SVD code contains no ESP32-S31 helper semantics.

`esp32s31-platform-radio-deps.svd` is a separate, validator-only catalog for
official-PAC registers reached from the vendor radio call graph. A project
run loads it together with the schema-2 radio model; direct target-spec
invocations load it together with `esp32s31-radio.svd`. It is not an input to
the radio raw PAC, generates no runtime crate, and creates no second peripheral
owner. Its definitions are pinned to the same `esp-pacs` revision as the
workspace lockfile. In particular, it mirrors all 23 contiguous
`MODEM_LPCON` registers at `0x2010_f000..0x2010_f05c` from that PAC, so vendor
clock, reset, retention-memory and wakeup accesses receive stable register
identities instead of merely falling inside a broad MMIO window.

## Evidence policy

Model `[[review]]` records use `sources` and `confidence`; source definitions
and target helper bindings live in `pac-addon.xml`. The generator accepts only
the following confidence vocabulary:

- `block-exact-register-semantics-opaque`: the containing S31 block and word
  address are exact, while the word's hardware semantics remain unnamed;
- `instruction-exact`: the complete instruction body proves the described
  operation and value transformation;
- `instruction-exact-partial`: the instruction evidence is complete for the
  described subset, but the register has additional unmodeled behavior;
- `instruction-exact-semantics-unknown`: the complete ROM/blob instruction
  body proves the address, mask and operation, but not the hardware meaning;
- `hil-observed`: the statement is an observed hardware result;
- `instruction-exact-hil-qualified`: complete instruction evidence is also
  qualified by a matching hardware observation.

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
| `ESP_IDF_ESP32S31_MODEM_REG_BASE` | Official ESP-IDF S31 modem partition map pinned to the commit and SHA-256 recorded in the add-on provenance catalog |
| `HIL_OPEN_HE_RATE_CONTROL_ACK_SNR_2026_07_30` | Open HE20 MCS9/LDPC A-MPDU completion, typed ACK-SNR decode, DHCP and zero-loss ICMP qualification on ESP32-S31 rev0 |
| `S31_MODEM_SYSCON_STRUCT` | Pinned `esp-wifi-sys` commit `2585f278`, S31 `modem_syscon_struct.h`, SHA-256 recorded in the add-on provenance catalog |
| `S31_MODEM_LPCON_STRUCT` | Same commit, S31 `modem_lpcon_struct.h`, SHA-256 recorded in the add-on provenance catalog |
| `S31_PMU_HEADERS` | Official ESP-IDF S31 `pmu_reg.h` pinned to the commit recorded in the add-on provenance catalog, plus local hashed copies in `esp-wifi-sys` |
| `S31_ESP_PACS_SVD` | `ermacv/esp-pacs` commit `d0fb94ef3` (the S31 platform work, evidenced PMU access corrections and qualified radio-field write constraints), ESP32-S31 generated SVD |
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
| `BLOB_LIBPP_HAL_TSF_TIMER` | Complete `hal_tsf.o` target, wakeup and enable/disable leaves; proves the four-entry `0x2010d87c + index*8` control/target bank and its WDEVPWR interrupt-mask transactions |
| `BLOB_LIBPP_HAL_TSF_RUNTIME` | Complete station-TBTT, activity, wakeup-clear, SoftAP/NAN and broadcast-TWT TSF leaves; proves the remaining runtime words at `0x2010d80c`, `0x2010d82c`, `0x2010d834`, `0x2010d840`, `0x2010d85c`, `0x2010d860`, `0x2010d868` and `0x2010d870` |
| `BLOB_LIBPP_HAL_COEX_RUNTIME` | Complete receive-beacon and individual-TWT leaves in `hal_coex.o`; proves `0x2010d854` and the five-word `0x2010d89c..0x2010d8ac` runtime COEX/PTI bank |
| `BLOB_LIBPP_HAL_RUNTIME_MMIO_LEAVES` | Complete beacon-filter/CRC, secondary-interrupt clear, QoS-null translation, baseband-error, RX-end and statistics-clear leaves across `hal_mac*.o` and `hal_sniffer.o`; closes the remaining seven unique unmapped HAL MMIO addresses |
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
