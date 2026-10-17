# ESP32-S31 ESP-HAL radio platform

This crate is the role-neutral owner of the ESP32-S31 modem platform
singletons used by radio lifecycles: `MODEM_SYSCON`, `MODEM_LPCON`,
`HP_SYS_CLKRST`, `PMU`, `LP_AON_CLK_RST`, `LP_PERI`, `LP_TSENS`, and
`I2C_ANA_MST`. Raw peripheral handles stay private. The official
`MODEM_SYSCON` and `MODEM_LPCON` tokens are inert ownership guards: all radio
words carved from those blocks are operated only by the affine custom PAC.
Clients receive narrow affine reservations; all clock dependencies are
reference-counted by the route-owned custom PAC before they reach MMIO. The
platform exposes the effective Bluetooth interface address through ESP-HAL's safe
base-eFuse accessor plus the pinned S31 second-universal-address policy. The
result remains in canonical EUI-48 order; the generic HCI bootstrap type
performs the reviewed conversion to HCI `BD_ADDR` byte order.

`EspHalRadioClocks` implements the HAL `PlatformClockProvider` for the radio
arbiter. Every platform-owned clock stays under ESP-HAL's own reference
counts: the 160 MHz PLL output and the MPLL through their clock-tree nodes
(`clock::ll::request_pll_f160m`, `request_mpll_clk`) and the three
`MODEM_LPCON.CLK_CONF` gates (analog-I2C master, coexistence and low-power
timer) under the lock ESP-HAL's regi2c accesses share, added by the pinned
fork. `acquire` returns a `PlatformClockGuard`, which ends explicitly: its
`release` calls the matching ESP-HAL release, and its `abandon` keeps the
clock on deliberately, as a fail-stop owner does. Dropping a guard otherwise
is a defect that keeps the clock on and asserts in a debug build. The
provider records no abandonment on the chip, because the radio's single
shared owner lives for the whole program. The
provider is a zero-size capability and a
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

A protocol composition reaches these singletons through the one owner of the
shared radio that holds this platform (none exists since #770, tag
`legacy-radio-2026-10-16`; #771 brings it back); the Wi-Fi ESP-HAL adapter
owns only the `WIFI` singleton. No adapter grants a second claim of the
platform resources.

The Bluetooth interrupt binding that lived here (sources, routes and the
three handlers) was removed with the Bluetooth LE orchestration (#770, tag
`legacy-radio-2026-10-16`); the source and route facts it carried are kept in
the [Bluetooth driver](../../../../hardware/esp32s31/driver/bluetooth/README.md#bring-up-and-teardown-order).
