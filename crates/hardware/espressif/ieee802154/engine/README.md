# IEEE 802.15.4 MAC engine

`oer-espressif-ieee802154-engine` is the MAC state machine and public API layer of the
public ESP-IDF IEEE 802.15.4 driver (`esp_ieee802154_dev.c` and its
helpers), ported once for every supported chip.

| Module | Contents |
| --- | --- |
| `engine` | Operation state machine, interrupt handler, receive ring and DMA frame buffers, statistics |
| `pib` | The driver's PIB and its publication to the MAC |
| `ll` | The `Ieee802154LowLevel` interface (the driver's `ieee802154_ll_*` accessors, modem ETM steps and timer commands) and the hardware helpers generic over it; `ll::model` is a register model for host tests (feature `model`) |
| `types`, `channel`, `coex`, `tx_power` | The chip-neutral values the interface exchanges, the 2.4 GHz channel/frequency-code map, the software-coexistence priorities and the vendor transmit-power resolution |

Each chip's HAL implements `Ieee802154LowLevel` over its own PAC and converts
between these values and its register types; the ESP32-S31 implementation is
`oer_esp32s31_hal::ieee802154::ll::Ieee802154MacOwners`. What differs between
chips stays with the chip: register geometry, the transmit-on delays and the
rest of the MAC lifecycle, the transmit-power level set, the coexistence
table, the diagnostic counters a chip's LL adds and the memory the MAC DMA
reaches, which the caller passes to
`Ieee802154Engine::buffers_dma_visible` before any hardware is touched.

The engine names no executor, interrupt route or chip. A runtime holds it
with the chip's MAC owners in one critical section and calls its operation
entries and interrupt handler. The
[ESP32-S31 host stand](../../../../../verification/esp32s31/host/ieee802154/README.md)
compares its accessor sequence with the compiled vendor driver.
