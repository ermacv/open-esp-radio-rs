# ESP32-S31 ESP-HAL radio platform

This crate is the role-neutral owner of the ESP32-S31 modem platform
singletons used by radio lifecycles: `MODEM_SYSCON`, `MODEM_LPCON`,
`HP_SYS_CLKRST`, `PMU`, `LP_AON_CLK_RST`, `LP_PERI`, `LP_TSENS`, and
`I2C_ANA_MST`. Raw peripheral handles stay private. Clients receive narrow
semantic leases, and shared clock dependencies are reference-counted before
they reach MMIO. The Bluetooth lease also implements the six platform
operation families required by the common `register_chipv7_phy` transition;
it does not expose any of the underlying singleton tokens.

Bluetooth is the first client. The existing Wi-Fi ESP-HAL adapter still owns
the same singleton types independently, so safe Wi-Fi + Bluetooth composition
is intentionally impossible today. Wi-Fi must migrate to this coordinator
before coexistence can be enabled; no duplicate or compatibility owner exists.

The Bluetooth clock/reset sequence is pinned to the reviewed ESP-IDF source:

- [`bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c)
- [`btdm_lp.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c)
- [`modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c)
