# ESP32-S31 radio register provenance

This register pass separates three questions that were previously mixed in
raw constants:

1. Which MMIO address is accessed?
2. Which bit field exists on ESP32-S31?
3. Which initialization order and value has actually been exercised?

The SVD answers the first two. HAL operations carry the third in comments and
through `PowerOperation::evidence()`.

## Clock and power chain

| Stage | Registers | Layout evidence | Operation evidence |
|---|---|---|---|
| HP-active modem ICG selection | `PMU.HP_ACTIVE_ICG_MODEM`, `IMM_MODEM_ICG`, `IMM_SLEEP_SYSCLK` | S31 SVD and PMU headers | pinned S31 `esp-hal` clock initialization |
| Modem bus and source | `HP_SYS_CLKRST.MODEM_CTRL0`, `MODEM_CONF` | S31 SVD | pinned S31 `esp-hal` clock initialization |
| Domain power-state maps | `MODEM_SYSCON.CLK_CONF_POWER_ST`, `MODEM_LPCON.CLK_CONF_POWER_ST` | exact S31 structures and LL accessors | pinned S31 `esp-hal` values `0x64646400` and `0x66660000`, now composed from named fields |
| Wi-Fi BB reset and PHY gates | `MODEM_RST_CONF`, `CLK_CONF1`, `CLK_CONF`, LPCON `CLK_CONF` | exact S31 structures | pinned S31 `esp-hal` PHY/radio clock sequence |
| Frontend/baseband gate leaf | three `PHY_CLOCK_ORACLE` registers and `PMU.HP_ACTIVE_HP_CK_POWER` | mixed S31 PMU plus opaque instruction evidence | complete rev0 ROM `phy_open_fe_bb_clk` and blob `phy_close_fe_bb_clk` bodies |
| Analog PHY-I2C power/reset | `PMU.RF_PWC`, `IMM_HP_CK_POWER_0`, `ANA_PERI_PWR_CTRL` | official S31 PMU header and S31 SVD | complete `libphy.a[phy_reg.o]::phy_open_i2c_xpd_new` body |

The former constant named `MODEM_LPCON_CLOCK_CONF_ADDRESS` at `0x2070401c`
was incorrect: that address is
`PMU.HP_ACTIVE_HP_CK_POWER`. Its bit 22 is the S31
`HP_ACTIVE_XPD_BB_I2C` field. The same ROM operation also sets bits 3:0; their
individual meanings remain unknown and are represented as
`ROM_OPEN_FE_BB_UNKNOWN_LOW`.

The three addresses previously labeled as MODEM_LPCON in the PHY port are
actually inside the S31 PMU block:

| Address | Recovered identity | Qualified fields |
|---|---|---|
| `0x207040f0` | `PMU.IMM_HP_CK_POWER_0` | all twelve S31 header fields; the open-I2C path uses `TIE_HIGH_XPD_BB_I2C` |
| `0x20704184` | `PMU.RF_PWC` | `XPD_RF_CIRCUIT[31:16]` |
| `0x20704208` | `PMU.ANA_PERI_PWR_CTRL` | `XPD_PERIF_I2C`, `RSTB_PERIF_I2C` |

The PMU header labels fields in `IMM_HP_CK_POWER_0` as `WT`, while the
complete S31 blob body loads the register before masking and storing it. CMSIS
SVD has no write-trigger access class. This recovered SVD therefore models
the register `read-write`, records the conflict in its description, and
preserves the blob's read/modify/write sequence in the HAL.

## PHY PBus and PHY-I2C

`PHY_MEMORY` at `0x20100800` now owns the shared table-memory aperture:

- `COMMAND` at `0x44`, with the widest instruction-evidenced ten-bit command
  in bits 20:11 and the TX-CFR commit pulse in bit 21;
- three mode-dependent data words at `0x48..0x50`;
- six packed PBUS group-boundary words at `0x54..0x68`.

Complete rev0 ROM `phy_set_pbus_mem`, `phy_write_pbus_mem`, and
`phy_save_pbus_reg` prove the full 60-entry sequence and the twelve packed
`first/last` group pairs. Complete ROM `phy_write_gain_mem` and complete blob
`phy_set_tx_cfr_mem` prove that the command and data words are shared with
gain-memory and CFR modes. Their overlapping subfields are therefore modeled
as one mode-dependent `MEMORY_COMMAND`, not as mutually contradictory aliases.

The public ESP32-S31 `esp-pacs` SVD pinned by this repository does not name
`0x20100844..0x20100868`. Public cross-chip PBUS memory-power fields are not
used to invent S31 identities for this internal aperture; complete S31
instructions remain the primary source.

The `phy_memory` HAL reproduces one optional boundary RMW, one data write and
one command RMW in ROM order. It captures the six boundary words through the
same PAC identities. The transition carries only group and entry values and
must borrow `&mut RadioRegisters`; raw addresses and volatile pointers no
longer cross the PBUS-memory binding.

`PHY_PBUS.STATUS_CLOCK_FORCE` at `0x20100890` is now confirmed as one physical
multifunction register rather than conflicting guessed aliases. Independent
complete rev0 ROM bodies establish:

- bits 11:8: the four-bit value replaced by `phy_force_txrx_off`;
- bits 15:14: the pair controlled by `phy_set_rxclk_en`;
- bits 17:16: the pair controlled by `phy_set_txclk_en`;
- bit 31: the busy status sampled by `phy_pbus_force_test`.

The pair-level functions do not distinguish the two constituent clocks, so
the SVD deliberately calls them `RX_CLOCK_ENABLE_PAIR` and
`TX_CLOCK_ENABLE_PAIR`. The four force encodings are exact, but their
electrical meaning is still unknown and remains
`FORCE_TXRX_MODE_UNKNOWN`.

The complete `phy_pbus_rd` address and shift tables expose five packed result
words at `0x20100894..0x201008a4`. Each visible nine-bit window is represented
without assigning an analog meaning. Only selector 1's low window has a
qualified consumer identity: the RX-DCO calibration path.

The PHY-I2C PAC records both host command words, the master control fields,
read-mask callback, opaque 14-bit host map, three clock-selection words, and
all 45 command-memory entries. Command layout is instruction-exact:
block/register/data occupy bytes 0/1/2, followed by write, busy and
start/reset bits 24/25/26.

The corresponding HAL is split by hardware ownership:

- `analog_i2c` owns PMU power/reset sequencing;
- `pbus` owns command publication, completion sampling, packed reads and the
  RX/TX clock pairs;
- `phy_i2c` owns host commands, the six-write clock-selection transform,
  master setup and bounded command RAM.

Every public operation documents both its S31 register-layout source and the
complete ROM/blob body used for operation order. The cold PHY binding accepts
`&mut RadioRegisters` borrowed from `Radio<P, Powered>` and uses these HAL
methods for the newly recovered regions. Reusable RFPLL, RXIQ/TXIQ, DCO,
gain, temperature, saturation, power and power-detector target bindings use
the same borrow; no raw-owner PHY-I2C or PBus force-test leaf remains.

## Cross-chip comparison

Current public ESP-IDF headers for ESP32-C5 and ESP32-C61 independently use the
same MODEM_SYSCON/LPCON naming model and R/W clock fields. This is useful for
detecting transcription errors, but it is deliberately secondary evidence:
only S31-local headers/SVD or complete S31 instruction bodies can create an
S31 PAC field.

Primary public comparison sources:

- Espressif ESP-IDF
  [ESP32-S31 PMU register header](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32s31/register/soc/pmu_reg.h);
- Espressif ESP-IDF
  [ESP32-C5 MODEM_SYSCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c5/include/modem/modem_syscon_reg.h)
  and
  [MODEM_LPCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c5/include/modem/modem_lpcon_reg.h);
- Espressif ESP-IDF
  [ESP32-C61 MODEM_SYSCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c61/include/modem/modem_syscon_reg.h)
  and
  [MODEM_LPCON](https://github.com/espressif/esp-idf/blob/master/components/soc/esp32c61/include/modem/modem_lpcon_reg.h);
- the public
  [esp-rs ESP32-S31 PAC/SVD package](https://github.com/esp-rs/esp-pacs/tree/main/esp32s31).

## Remaining uncertainties

The individual identities inside the RX/TX two-bit clock pairs, the electrical
meaning of the four force-TX/RX encodings, the PBus packed result windows, and
the two clock-selection subfields remain unknown. They are included with
`UNKNOWN` names because their locations and complete operations are useful
and instruction-exact.

Other raw PHY MMIO constants still follow the same migration rule: move an
address into the SVD only when its identity is stable; add named fields only
when S31-local layout or instruction-level masks support them.
