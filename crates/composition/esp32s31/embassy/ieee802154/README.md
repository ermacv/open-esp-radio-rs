# ESP32-S31 IEEE 802.15.4 system

`oer-esp32s31-ieee802154-system` brings IEEE 802.15.4 up as a client of the
[shared radio](../radio/README.md) (`RadioSystem`) and tears it down again.
It is the chip composition of the [HAL lifecycle](../../../../hardware/esp32s31/hal/src/ieee802154/role.rs),
the [PHY client](../../../../hardware/esp32s31/phy/src/ieee802154_client.rs),
the [runtime](../../../../runtime/esp32s31/ieee802154/src/lib.rs) and the
[esp-hal route](../../../../adapters/esp-hal/esp32s31/ieee802154/src/lib.rs).

## Lifecycle

`start` follows ESP-IDF's `esp_ieee802154_enable` on the concurrent split:

1. enter common radio power and enable the IEEE 802.15.4 module clocks;
2. prepare the shared PHY through the radio system: register the domain when
   no client did, or wake its closed RF;
3. join the domain and the shared BTBB baseband and apply the transmit-on
   delay; run PHY tracking inside the client's quiescence window when the
   join makes it due;
4. pulse the MAC reset and configure the masked foundation;
5. activate the interrupt owner, install the runtime (engine `mac_init`) and
   bind modem source 132 at priority one.

`Ieee802154System::stop` reverses the steps: quiesce the route, take the MAC
owners out of the runtime, prove the foundation again, leave the domain and
BTBB (the radio system closes RF after the last PHY client), release the
clocks and leave
common power. It returns the partition and the engine for a later start.

Periodic shared PHY tracking belongs to the radio system
(`RadioSystem::run_tracking`), as ESP-IDF's `phy_track_pll` timer belongs to
`esp_phy`. `Ieee802154System::maintain_phy` runs one tracking attempt on
demand. Under the default vendor admission it is one tracking tick with IEEE
802.15.4 receiving. Under the stricter quiesced admission, when IEEE
802.15.4 is the only active client, the call pauses the runtime (leaving
receive mode), closes the CPU route, issues the client's quiescence proof,
tracks within that window, then resumes receive mode and binds the route
again; `maintain_phy_until` repeats it once per tracking period. With
another client active it reports `AwaitingOtherClients`; a running
transmission, scan or CCA reports `Busy`.

A step that fails before starting shared hardware work rolls the earlier
steps back and returns the parked partition. A started PHY, clock or power
transaction that fails keeps its owner as fail-stop; the chip must be reset.

## Use

The runtime is a process singleton. After `start`, `Ieee802154System::runtime`
accepts portable `RadioCommand`s and yields `Ieee802154RadioEvent`s. The
engine resolves transmit power through the recovered ESP32-S31 BTBB level set.
Enhanced ACKs follow ESP-IDF's OpenThread port: `start` installs no
generator, so 2015 frames are delivered without an ACK, as with the vendor's
default generator. `Ieee802154SystemRuntime::with_enhanced_ack` installs an
`Ieee802154EnhancedAckGenerator`, sets its header IEs and gives it the MAC
keys and frame counter that secure the ACK of a secured frame; the stack
takes the frame counters of its own secured transmissions from the same
`MacKeys`. Each `start` begins without a generator.

CSMA-CA transmissions follow OpenThread `SubMac` over the one-CCA radio: a
random backoff before each CCA attempt, receiving on the transmit channel
when the radio was receiving, and another backoff while the channel is busy.
A transmission with `max_frame_retries` is retried as `SubMac` retries it
with ESP-IDF's OpenThread defaults: after an attempt without channel access
at once, after one without acknowledgement following a random delay whose
exponent grows from 0 to 5. Each attempt after the first arms the transmit
security the upper layer set again, as the port arms it per transmit; a
retransmitted secured frame keeps its frame counter, while `SubMac`
re-secures one that carries header IEs with a new counter. The backoff and
retry timers run inside `Ieee802154SystemRuntime::next_event`, so the
consumer must await events while a transmission waits; the random words
come from the hardware generator.

The shared radio is a software-coexistence build, so the MAC takes part in
coexistence as the vendor driver does with `CONFIG_ESP_COEX_SW_COEXIST_ENABLE`:
`start` reads the arbiter's coexistence table once and the engine publishes
the ACK priority at the middle level and the TX/RX priority of each scene
(idle, low for transmission and reception, middle for timed operations).
`Ieee802154System::update_coexistence` reads the table again, optionally with
other scene levels (`esp_ieee802154_set_coex_config`); a changed table does
not reach the MAC until it is called. Stopping the client returns both
priorities to the disabled image the foundation proves.

## Limits

Under the quiesced admission, tracking that must collect the proofs of
several active clients needs a joint radio supervisor, which is not composed. Sleep and RF gating are not
composed, and on-air behaviour is qualified only by HIL runs.
