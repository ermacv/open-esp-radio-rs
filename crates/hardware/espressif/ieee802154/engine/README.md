# IEEE 802.15.4 MAC engine

`oer-espressif-ieee802154-engine` is the MAC state machine and public API layer of the
public ESP-IDF IEEE 802.15.4 driver (`esp_ieee802154_dev.c` and its
helpers), ported once for every supported chip.

| Module | Contents |
| --- | --- |
| `engine` | Operation state machine, interrupt handler, receive ring and DMA frame buffers, statistics |
| `pib` | The driver's PIB and its publication to the MAC |
| `ll` | The `Ieee802154LowLevel` interface (the driver's `ieee802154_ll_*` accessors, modem ETM steps and timer commands) and the hardware helpers generic over it; `ll::model` is a register model for host tests (feature `model`) |
| `capabilities` | The portable capabilities a radio over the engine reports (ESP-IDF's OpenThread port defaults) and the table a chip builds from its transmit-power level set |
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

## Bring-up and teardown order

No runtime holds the engine now; the former ESP32-S31 composition (tag
`legacy-radio-2026-10-16`) brought the radio up in the order of ESP-IDF's
`esp_ieee802154_enable`:

1. the common radio power and the IEEE 802.15.4 module clocks;
2. the shared PHY domain: registered, woken or joined as the
   [first PHY client](../../../esp32s31/phy/README.md#first-and-last-phy-client);
3. the shared BTBB reference and the transmit-on delay;
4. the MAC reset and the masked foundation;
5. the interrupt owner;
6. the engine's `mac_init`, under the runtime's critical section;
7. the CPU route of modem interrupt source 132.

Stop reverses every step. A failure before shared state is touched rolls
back; a started PHY or clock transaction that fails is fail-stop. The
runtime is a process singleton: source 132 has one handler.
