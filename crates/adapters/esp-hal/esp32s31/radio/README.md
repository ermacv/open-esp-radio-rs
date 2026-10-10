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

`EspHalRadioClocks` implements the HAL `PlatformClockProvider` for the radio
arbiter. Every platform-owned clock stays under ESP-HAL's own reference
counts: the 160 MHz PLL output and the MPLL through their clock-tree nodes
(`clock::ll::request_pll_f160m`, `request_mpll_clk`) and the three
`MODEM_LPCON.CLK_CONF` gates (analog-I2C master, coexistence and low-power
timer) under the lock ESP-HAL's regi2c accesses share, added by the pinned
fork. `acquire` returns a `PlatformClockGuard`; dropping it calls the
matching ESP-HAL release. The provider is a zero-size capability and a
guard does not borrow it, because ESP-HAL's counts are global.

`EspHalRadioPlatform` holds the `I2C_ANA_MST` singleton, and that singleton
is the ownership of the analog-I2C bus. The platform hands it out once as the
HAL's `AnalogBusOwnership`, without which `RadioHardware::take` gives no radio
root, so the PHY's analog transactions, split across executor polls, run only
under it. The brownout detector, whose ESP-HAL configuration borrows the
singleton too, is configured once by the platform bootstrap before the
application starts
([platform policy](../../../../../platform/esp32s31/README.md#brownout-detector-policy)),
so no image configures it after the singleton moves here. No lock is shared
with ESP-HAL.

Every protocol composition reaches these singletons through the one shared
radio system that owns this platform; the Wi-Fi ESP-HAL adapter owns only the
`WIFI` singleton. No adapter grants a second claim of the platform resources.

The pinned ESP32-S31 PAC names all three Controller sources as `MODEM_BT_MAC`,
`MODEM_LP_TIMER`, and `MODEM_BT_MAC_INT1`. This adapter routes those typed identities
for the reviewed source-124/source-127/source-133 policies and contains one
same-core, level-three bind/disable set. `bind_routes` borrows the
stable publication and returns one affine
`BoundEspHalBluetoothInterruptEpoch`. The adapter owns the exact three
handlers, public SRAM fns each Bluetooth image's interrupt table names at
level three on core 0; `EspHalBluetoothInterruptRoutes`, made from the three
sources' tokens (another source's token does not compile), reaches the
adapter once per boot through `BluetoothParked::new`, and every epoch enables
and disables the three routes. Integration publishes one full-controller
dispatcher that receives
a fixed `Primary`, `ModemLpTimer`, or `NrtDefault` role. The roles therefore
cannot be exchanged by passing handlers in the wrong order. Full dispatcher
state must be stable before bind. The callback/live marker is installed before
the first route is enabled, so even an immediately pending interrupt observes
the complete dispatcher. Successful same-core disable closes all three routes
and clears the dispatcher while the epoch ends its borrow; dropping the epoch
is fail-stop and cannot globally remint another live route. The three
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

After complete route removal and timer extraction,
`retire_interrupt_registers_after_routes_disabled` returns the actual shared
primary/NRT owner in an opaque post-route state. This operation does not access
registers or prove dynamic-source quiescence. The Bluetooth composition admits
it after idle task handoff and drained timer retirement, then retains the result
through platform join. Rejected extraction leaves the owner in its original
slot. Successful extraction does not release the storage reservation: old
static Controller borrows still exist, so another publication is rejected even
when both slots are empty. Board reset is the only current reservation reset.

`RetiredEspHalBluetoothInterruptRegisters::into_output_owner` hands the
recovered post-route owner to the Controller composition. Output release,
physical cold return and PHY maintenance belong to that composition; this
adapter exposes no route reactivation on the recovered owner.
