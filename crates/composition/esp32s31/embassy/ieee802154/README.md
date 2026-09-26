# ESP32-S31 IEEE 802.15.4 system

`oer-esp32s31-ieee802154-system` brings IEEE 802.15.4 up as a client of the
shared radio arbiter (`SharedRadio<ConcurrentPhy>`) and tears it down again.
It is the chip composition of the [HAL lifecycle](../../../../hardware/esp32s31/hal/src/ieee802154/role.rs),
the [PHY client](../../../../hardware/esp32s31/phy/src/ieee802154_client.rs),
the [runtime](../../../../runtime/esp32s31/ieee802154/src/lib.rs) and the
[esp-hal route](../../../../adapters/esp-hal/esp32s31/ieee802154/src/lib.rs).

## Lifecycle

`start` follows ESP-IDF's `esp_ieee802154_enable` on the concurrent split:

1. enter common radio power and enable the IEEE 802.15.4 module clocks;
2. register the shared PHY domain when no client did, or wake its closed RF;
3. join the domain and the shared BTBB baseband and apply the transmit-on
   delay; run PHY tracking inside the client's quiescence window when the
   join makes it due;
4. pulse the MAC reset and configure the masked foundation;
5. activate the interrupt owner, install the runtime (engine `mac_init`) and
   bind modem source 132 at priority one.

`Ieee802154System::stop` reverses the steps: quiesce the route, take the MAC
owners out of the runtime, prove the foundation again, leave the domain and
BTBB (closing RF after the last PHY client), release the clocks and leave
common power. It returns the partition and the engine for a later start.

A step that fails before starting shared hardware work rolls the earlier
steps back and returns the parked partition. A started PHY, clock or power
transaction that fails keeps its owner as fail-stop; the chip must be reset.

## Use

The runtime is a process singleton. After `start`, `Ieee802154System::runtime`
accepts portable `RadioCommand`s and yields `Ieee802154RadioEvent`s. The
engine resolves transmit power through the recovered ESP32-S31 BTBB level set.
The enhanced-ACK generator is not composed; enhanced ACKs are refused.

## Limits

Periodic PHY tracking while the MAC runs is not scheduled, sleep and RF
gating are not composed, and on-air behaviour is qualified only by HIL runs.
