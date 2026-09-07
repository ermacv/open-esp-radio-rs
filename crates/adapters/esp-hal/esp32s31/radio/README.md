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

Bluetooth uses this coordinator. The Wi-Fi ESP-HAL adapter owns the same
singleton types independently, so the production APIs cannot safely compose
simultaneous Wi-Fi and Bluetooth. Neither adapter grants a second claim of
those platform resources.

The pinned ESP32-S31 PAC names all three Controller sources as `BT_MAC`,
`MODEM_LP_TIMER`, and `BT_MAC_INT1`. This adapter routes those typed identities
for the reviewed source-124/source-127/source-133 policies and contains one
same-core, level-three bind/disable set. `bind_routes` borrows the
stable publication and returns one affine
`BoundEspHalBluetoothInterruptEpoch`. The adapter owns the exact three ESP-HAL
handlers; integration publishes one full-controller dispatcher that receives
a fixed `Primary`, `ModemLpTimer`, or `NrtDefault` role. The roles therefore
cannot be exchanged by passing handlers in the wrong order. Full dispatcher
state must be stable before bind. The callback/live marker is installed before
the first route is enabled, so even an immediately pending interrupt observes
the complete dispatcher. Successful same-core disable closes all three routes
and clears the dispatcher while the epoch ends its borrow; dropping the epoch
is fail-stop and cannot globally remint another live route. The three private
bound-service entries first check the live epoch marker and otherwise perform
no register access. They run only the finite primary classifier, opaque
default NRT acknowledgement or timer register disposition. The chip Controller
consumes those semantic results and durably updates its scheduler/task cells.
A fatal stable-storage result quarantines the asserted CPU route before the
adapter-owned hard handler returns.
It also lets Controller task code take a software-pending timer owner and
return only its fully rearmed successor. The chip crate owns executor
notification, the bounded modem-timer queue and backpressured expiration
handoff, so this adapter does not duplicate Controller policy.

The Bluetooth clock/reset sequence is pinned to the reviewed ESP-IDF source:

- [`bt.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/controller/esp32s31/bt.c)
- [`btdm_lp.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/bt/porting_btdm/controller/btdm_common/src/btdm_lp.c)
- [`modem_clock_impl.c`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/modem/port/esp32s31/modem_clock_impl.c)
- [`Kconfig.mac`](https://github.com/espressif/esp-idf/blob/aeab6dcfbeb44aba4b1f8ed102e3086172833153/components/esp_hw_support/port/esp32s31/Kconfig.mac)
