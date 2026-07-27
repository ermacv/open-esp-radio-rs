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

The former constant named `MODEM_LPCON_CLOCK_CONF_ADDRESS` at `0x2070401c`
was incorrect: that address is
`PMU.HP_ACTIVE_HP_CK_POWER`. Its bit 22 is the S31
`HP_ACTIVE_XPD_BB_I2C` field. The same ROM operation also sets bits 3:0; their
individual meanings remain unknown and are represented as
`ROM_OPEN_FE_BB_UNKNOWN_LOW`.

## Cross-chip comparison

Current public ESP-IDF headers for ESP32-C5 and ESP32-C61 independently use the
same MODEM_SYSCON/LPCON naming model and R/W clock fields. This is useful for
detecting transcription errors, but it is deliberately secondary evidence:
only S31-local headers/SVD or complete S31 instruction bodies can create an
S31 PAC field.

Primary public comparison sources:

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

## Deferred conflicts

`0x20100890` is still used under multiple recovered semantic names in the PHY
port (PBus status, clock control and force-TX/RX related paths). No public S31
register description or complete instruction comparison yet separates those
interpretations. It is therefore excluded from this SVD pass rather than
being assigned a convenient name.

The remaining raw PHY MMIO constants follow the same migration rule: move an
address into the SVD only when its register identity is stable; add named
fields only when S31-local layout or instruction-level masks support them.
