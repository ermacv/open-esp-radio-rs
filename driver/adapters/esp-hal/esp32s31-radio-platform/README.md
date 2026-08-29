# ESP32-S31 ESP-HAL radio platform

This crate is the role-neutral owner of the ESP32-S31 modem platform
singletons used by radio lifecycles: `MODEM_SYSCON`, `MODEM_LPCON`,
`HP_SYS_CLKRST`, `PMU`, `LP_AON_CLK_RST`, `LP_PERI`, `LP_TSENS`, and
`I2C_ANA_MST`. Raw peripheral handles stay private. The official
`MODEM_SYSCON` and `MODEM_LPCON` tokens are inert ownership guards: all radio
words carved from those blocks are operated only by the affine custom PAC.
Clients receive narrow affine reservations; all clock dependencies are
reference-counted by the route-owned custom PAC before they reach MMIO. The
Bluetooth platform lease exposes no operation family or singleton token. It
does expose the effective Bluetooth interface address through ESP-HAL's safe
base-eFuse accessor plus the pinned S31 second-universal-address policy. The
result remains in canonical EUI-48 order; the generic HCI bootstrap type
performs the reviewed conversion to HCI `BD_ADDR` byte order.

Bluetooth is the first client. The existing Wi-Fi ESP-HAL adapter still owns
the same singleton types independently, so safe Wi-Fi + Bluetooth composition
is intentionally impossible today. Wi-Fi must migrate to this coordinator
before coexistence can be enabled.

The pinned ESP32-S31 PAC names all three Controller sources as `BT_MAC`,
`MODEM_LP_TIMER`, and `BT_MAC_INT1`. This adapter compile-checks those
identities against the reviewed source-124/source-127/source-133 policies and
contains one same-core, level-three bind/disable set. The primitives remain
crate-private: stable scheduler-event publication and scheduler-list drain
must be composed before a public live interrupt epoch can safely enable any route. Both unique
register owners are already published atomically in stable process-wide
slots. The published lease can run the finite primary classifier, opaque
default NRT acknowledgement and timer register disposition while retaining
their owners in those slots. It also lets Controller task code take a
software-pending timer owner and return only its fully rearmed successor. The
chip crate owns the bounded modem-timer queue, epoch and backpressured event
handoff, so this adapter does not duplicate Controller policy.

The Bluetooth clock/reset sequence is pinned to the reviewed ESP-IDF source:

- [`bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c)
- [`btdm_lp.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c)
- [`modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c)
- [`Kconfig.mac`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/port/esp32s31/Kconfig.mac)
