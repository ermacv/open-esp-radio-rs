# ESP32-C5 radio HAL

`oer-esp32c5-hal` owns the ESP32-C5 radio registers above the closed
[PAC](../pac/src/lib.rs). This first stage covers the IEEE 802.15.4 MAC:

| Item | Contents |
| --- | --- |
| `ieee802154::mac` | Task and interrupt owners made from the PAC partition, and the vendor transmit-on delays |
| `ieee802154::ll::Ieee802154MacOwners` | The ESP32-C5 implementation of the chip-neutral [engine](../../ieee802154/engine/README.md)'s low-level interface |
| `ieee802154::{tx_power, coex, IEEE802154_DMA_WINDOW}` | The provider power levels, the coexistence priorities and the memory the MAC DMA reaches |
| `coex` | The vendor coexistence priority table |

The recovered values name their pinned source in their documentation.
Clocks, resets, the PHY and the MAC lifecycle around these owners are not
owned by this crate yet, so no ESP32-C5 composition starts the MAC.
