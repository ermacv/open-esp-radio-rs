# ESP32-S31 IEEE 802.15.4 system

`oer-esp32s31-ieee802154-system` brings IEEE 802.15.4 up as a client of the
[shared radio](../../../../runtime/esp32s31/radio/README.md) (`RadioSystem`) and tears it down again.
It is the chip composition of the [HAL lifecycle](../../../../hardware/esp32s31/hal/src/ieee802154/role.rs),
the [PHY client](../../../../hardware/esp32s31/phy/src/ieee802154_client.rs),
the [runtime](../../../../runtime/espressif/ieee802154/src/lib.rs) and the
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

The crate reexports what construction needs, so an application builds the
client through it, or through the `oer` facade's `embassy-ieee802154`
feature at `oer::systems::esp32s31::embassy::ieee802154`, alone:
`EspHalRadioPlatform` is what the facade's `radio::start` takes to create
the `SharedRadio` and spawn its PHY tracking and coexistence schedule;
`Ieee802154Parked::new` takes the radio's IEEE 802.15.4 partition and the
`Ieee802154PibDefaults`, and `start` joins the shared radio with them.
`Ieee802154MacOwners` names the runtime's owners.
`IEEE802154_DEFAULT_TX_POWER_DBM` and `IEEE802154_RECEIVE_SENSITIVITY_DBM`
are the chip figures ESP-IDF reports to an upper stack (the highest BTBB
power level every channel starts at, and `IEEE802154_RX_SENSITIVITY`), and
`Ieee802154CoexConfig` and `Ieee802154CoexLevel` the scene levels
`Ieee802154System::update_coexistence` takes. With the facade's
`openthread` feature, the
[OpenThread radio adapter](../../../../adapters/openthread/ieee802154/README.md)
is at `ieee802154::openthread`; the
[Thread example](../../../../../examples/esp32s31/thread/) uses only this path.

The runtime is a process singleton. After `start`, `Ieee802154System::runtime`
is the client's `Ieee802154RadioPort`: it accepts portable `RadioCommand`s,
yields `Ieee802154RadioEvent`s and applies `RadioSetting`s. The
engine resolves transmit power through the recovered ESP32-S31 BTBB level set.
Enhanced ACKs follow ESP-IDF's OpenThread port: `start` installs no
generator, so 2015 frames are delivered without an ACK, as with the vendor's
default generator. `RadioSetting::EnhancedAck` installs a generator and
`RadioSetting::EnhancedAckHeaderIes` and `EnhancedAckProbing` set its header
IEs and Link Metrics initiators. Each `start` begins without a generator.

Transmit security follows ESP-IDF's OpenThread port, which claims
`OT_RADIO_CAPS_TRANSMIT_SEC`: `RadioSetting::MacKeys` and
`RadioSetting::FrameCounter` install the keys (key index, previous, current
and next key) and the frame counter that `otPlatRadioSetMacKey` and
`otPlatRadioSetMacFrameCounter` set, starting from zeroed keys. With keys the radio secures every attempt of a secured frame: a new
frame counter unless the attempt retransmits the frame, the current key
index and key, and in key identifier mode 1 the extended address as the
nonce source; secured enhanced ACKs take their counter from the same keys.
As in the port, a retransmission keeps its counter and key index; the port
takes a new counter per retry only for a CSL receiver, which is not composed.
A request's `TxSecurity` carries the port's transmit information:
`Retransmission` for the stack's own retransmission (`mIsARetx`) and
`Processed` for a frame the stack already secured (`mIsSecurityProcessed`),
which goes out as given. The transmit completion reports the frame counter
and key index the radio wrote, which the port leaves in the stack's frame,
and each received frame reports the frame-pending bit and the security of
the acknowledgement it was sent. Security the upper layer arms with
`RadioSetting::TransmitSecurity` takes precedence for the next transmission, and
without keys a secured frame goes out as given. Frames in
other key identifier modes reuse the address of the last mode 1
transmission, as the port's shared `s_security_addr` does; unlike the port,
secured enhanced ACKs do not refresh it.

The engine's DMA frames belong to the composition: the MAC DMA reaches
internal SRAM alone, not PSRAM, so they live in the platform's DMA-visible
section whatever the image's data placement, and `Ieee802154Parked::new`
takes them once. `start` refuses frames outside that memory before touching
the hardware (`Ieee802154StartError::BuffersNotDmaVisible`).

`Ieee802154Parked::multipan` parks an engine built with multi-PAN
(`CONFIG_IEEE802154_MULTI_PAN_ENABLE`) and one to four interfaces. Its
radio publishes `MULTI_PAN`: `Configuration::Interface` sets each
interface's PAN ID, addresses, enable bit, pending mode and frame-pending
table, received frames report the interface they matched (`None` for a
broadcast), and a transmission names its interface. As ESP-IDF's
multi-instance OpenThread port keeps a security context per instance,
the MAC key settings name the interface whose keys they change: a transmission is secured with its interface's keys and extended
address, an enhanced ACK with those of the interface the acknowledged frame
matched. Receive and sleep stay radio-wide: the port's
`esp_ieee802154_multipan_receive` is enabling the interface and receiving,
`esp_ieee802154_multipan_sleep` disabling it and sleeping once no
interface is enabled.

CSMA-CA transmissions follow OpenThread `SubMac` over the one-CCA radio: a
random backoff before each CCA attempt, receiving on the transmit channel
when the radio was receiving, and another backoff while the channel is busy.
A transmission with `max_frame_retries` is retried as `SubMac` retries it
with ESP-IDF's OpenThread defaults: after an attempt without channel access
at once, after one without acknowledgement following a random delay whose
exponent grows from 0 to 5. Every attempt arms its transmit security again,
as the port arms it per transmit. The backoff and
retry timers run in the runtime's runner, `Ieee802154System::run` (the
runtime's `Ieee802154Runtime::run`), which the application polls beside the
consumer of the port's events for as long as the client runs; `next_event`
only takes events. The random words come from the hardware generator.
The port's clock (`now`) is the runtime's `embassy-time` clock, running
whether or not a radio is installed; synchronous callers such as OpenThread
read it, and the live RSSI, through the port itself (the OpenThread
adapter's `PortClock` and `PortRssi`).

Source matching is part of the portable contract: `Configuration` sets the
pending mode and adds, removes or resets the sources of interface zero's
frame-pending table (`esp_ieee802154_set_pending_mode`,
`add_pending_addr`, `clear_pending_addr`, `reset_pending_table`); a source
a full half has no room for is refused with `PendingTableFull`. The
vendor's TX/RX statistics (`CONFIG_IEEE802154_TXRX_STATISTIC`) are
collected on request through `Ieee802154SystemRuntime::set_txrx_statistics`:
started transmissions, transmissions refused during a reception, `TX_DONE`
and `RX_DONE` interrupts, TX coexistence breaks and the MAC's diagnostic
counters, which every interrupt drains.

Scheduled operations use the radio clock, the port's `now`
(ESP-HAL's microsecond clock, as `otPlatRadioGetNow` reads `esp_timer`). A
`TxMode::Scheduled` transmission starts at its time through TIMER0 and the
modem ETM, after a CCA when it asks for one (`esp_ieee802154_transmit_at`).
`RadioCommand::ScheduledReceive` opens a receive window through TIMER1
(`otPlatRadioReceiveAt`): the radio sleeps outside it, and
`ScheduledReceiveDone` reports its end - at its end, with its first
received frame as the vendor receive path ends it, or at once for a window
that had already ended. Another operation replaces the window, after which
the radio sleeps. `RadioCommand::Cancel` ends a running transmission
(`TxStatus::Aborted`, or the outcome the MAC had already reached), energy
scan, CCA or scheduled window with its terminal event, stopping the MAC as
`esp_ieee802154_sleep` does and resuming receive mode when the radio was
receiving.

A CSL receiver sets its period and next sample time with
`RadioSetting::Csl` (`otPlatRadioEnableCsl`,
`otPlatRadioUpdateCslSampleTime`). With a period, the radio answers 2015
frames with enhanced ACKs that carry a CSL IE, writes the period and the
phase to the next sample time into the CSL IE of every frame it sends when
the frame's SFD goes out - the engine writes the changed bytes into the
frame the MAC is sending, as ESP-IDF's OpenThread port edits it in place -
and secures retransmissions with a new frame counter. The runtime lends the
radio clock as a function (the port's `clock`) for callers
that read it without the lock.

The shared radio is a software-coexistence build, so the MAC takes part in
coexistence as the vendor driver does with `CONFIG_ESP_COEX_SW_COEXIST_ENABLE`:
`start` reads the arbiter's coexistence table once and the engine publishes
the ACK priority at the middle level and the TX/RX priority of each scene
(idle, low for transmission and reception, middle for timed operations).
`Ieee802154System::coex_config` returns the levels in use
(`esp_ieee802154_get_coex_config`), and
`Ieee802154System::update_coexistence` reads the table again, optionally with
other scene levels (`esp_ieee802154_set_coex_config`); a changed table does
not reach the MAC until it is called. Stopping the client returns both
priorities to the disabled image the foundation proves.

Next to Wi-Fi, as in a Thread border router,
`Ieee802154System::enable_wifi_coexistence` enters IEEE 802.15.4 into
coexistence (`esp_coex_wifi_i154_enable`: `coex_enable` and the schedule
status bit) through `RadioGuard::enable_ieee802154_coex`; the image must
run the radio system's coexistence schedule, as its Wi-Fi composition does.
`disable_wifi_coexistence` leaves it again, and `stop` refuses while it
takes part, since leaving can fail after it changed the coexistence state.

RF stays open for the client's lifetime, as in ESP-IDF builds without
tickless idle and modem retention, where `IEEE802154_RF_ENABLE` and
`IEEE802154_RF_DISABLE` are empty: ESP-IDF closes RF for a sleeping radio
only together with the REGDMA retention of the PHY, which this port does
not implement. The radio's sleep state is kept by the MAC alone. `stop` leaves the shared PHY and
closes RF after the last client, and `start` wakes it, as
`esp_ieee802154_disable` and `esp_ieee802154_enable` do.

## Limits

Under the quiesced admission, tracking that must collect the proofs of
several active clients needs a joint radio supervisor, which is not composed.
Modem retention and light sleep are not composed: closing RF keeps the MAC
and baseband powered. On-air behaviour is qualified only by HIL runs.
