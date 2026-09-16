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

After complete route removal and timer extraction,
`retire_interrupt_registers_after_routes_disabled` returns the actual shared
primary/NRT owner in an opaque post-route state. This operation does not access
registers or prove dynamic-source quiescence. The Bluetooth composition admits
it after idle task handoff and drained timer retirement, then retains the result
through platform join. Rejected extraction leaves the owner in its original
slot. Successful extraction does not release the storage reservation: old
static Controller borrows still exist, so another publication is rejected even
when both slots are empty. Board reset is the only current reservation reset.

The recovered post-route owner can join the retired Controller task through
`try_release_controller_output`. Busy scheduler, published heads, an in-flight
time latch or primary faults reject without releasing output. Success masks
dynamic sources, disables RUN, acknowledges its residual sources and performs
the reviewed output-release transaction. `release_physical` then composes the
matching retired timer/task/platform with last-client RF close, temperature
power-down, Bluetooth reset and checked clock restoration. It returns the
actual cold radio, retaining the separate closed software epoch. Storage remains
reserved; physical cold return alone does not authorize a second publication.

`RetiredEspHalBluetoothInterruptRegisters::maintain_phy` lends the actual
unrouted bank to the idle Controller maintenance transition. Both ISR owners
return atomically to the original claimed storage before the same service can
bind routes again. The same atomic restoration supports completed powered
restart. Restoration rejects live routes or occupied slots; it never releases
the reservation or issues another publication lease. See the
[Controller lifecycle](../../../../hardware/esp32s31/driver/bluetooth/README.md#quiescent-phy-maintenance).
