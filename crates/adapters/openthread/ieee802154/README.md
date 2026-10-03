# IEEE 802.15.4 OpenThread radio

`oer-ieee802154-openthread` implements the `Radio` trait of the
[`openthread`](https://crates.io/crates/openthread) crate over any
[`Ieee802154RadioPort`](../../../protocols/ieee802154/src/port.rs), so the
OpenThread stack that crate binds can run on any backend of the portable
IEEE 802.15.4 radio port. It is portable: it depends on the protocol
package and the trait crate alone. The ESP32-S31 composition's runtime
(`oer-espressif-ieee802154-runtime`) is the port the Thread example and the
HIL target use; the ESP-IDF behaviour described below is that of this
adapter over that runtime.

It depends on `openthread-radio`, the trait crate of the repository's fork
of that crate,
[`ermacv/openthread`](https://github.com/ermacv/openthread/tree/oer/main)
at a pinned revision of its working branch `oer/main`, which merges upstream
`esp-rs/openthread` `main` every one to two weeks, keeps every pinned
revision under a `pin/<sha>` tag and records what each merge took in
[`UPSTREAM.md`](https://github.com/ermacv/openthread/blob/oer/main/UPSTREAM.md);
the fork's `openthread` crate at the same revision re-exports the
trait for the application. The fork extends the trait with what OpenThread hands a radio that
claims `OT_RADIO_CAPS_TRANSMIT_SEC` and reads back from it: the MAC keys and
frame counter (`set_mac_keys`, `set_mac_frame_counter`), each frame's
transmit information (`transmit_frame`: `mIsARetx`, `mIsSecurityProcessed`,
and the frame counter and key index the radio wrote), the acknowledgement
sent for a received frame (`PsduMeta::ack`), the capabilities declared
before the instance is built (`OtResources::set_radio_caps`) and, with its
`csl` feature, Coordinated Sampled Listening: the radio clock
(`Radio::clock`), scheduled receive windows (`Radio::receive_at`), the CSL
receiver state (`Radio::set_csl`), delayed transmission (`TxFrame::tx_at`)
and receive SFD times (`PsduMeta::timestamp`), and, with its
`link-metrics-subject` feature, the enhanced-ACK probing initiators of a
Link Metrics subject (`Radio::set_enh_ack_probing`), and, with its
`time-sync` feature, the Time IE of each frame (`TxFrame::time_sync`).

## Use

Start the IEEE 802.15.4 composition, pass `PortClock::new(port)` and
`PortRssi::new(port)` to the `OpenThread` constructor, which reads the
port's own clock (`otPlatRadioGetNow`, `otPlatTimeGet`) and live RSSI
(`otPlatRadioGetRssi`) through them, then hand the port to
`OpenThreadRadio::new` with the transmit power, CCA threshold and receive
sensitivity to report, and give the radio to the `openthread` crate. Poll
the runtime's runner (`Ieee802154System::run`) beside it: the adapter is the
port's one event consumer and only takes events. The
figures are the composition's: `OpenThreadRadioDefaults::esp_idf` takes the
chip's default transmit power and receive sensitivity (on the ESP32-S31,
`IEEE802154_DEFAULT_TX_POWER_DBM` and `IEEE802154_RECEIVE_SENSITIVITY_DBM`
of the composition) with ESP-IDF's default CCA threshold. The radio enables
the port through its lifecycle and takes the `Enabled` terminal event,
installs zeroed MAC keys and an enhanced-ACK generator through
`RadioSetting`s and serves OpenThread's operations through the port's
commands and events. Declare `OPEN_THREAD_RADIO_CAPABILITIES`
with `OtResources::set_radio_caps` before building the OpenThread instance:
OpenThread's `SubMac` reads the capabilities once, when the instance is
built, and leaves transmit security to the radio only when it saw it there.
When the port reports `EventsLost`, the adapter cancels an operation whose
terminal event may be in the gap (a refusal as not running proves it ended;
a port without cancellation is judged by its state) and reports the lost
frames to OpenThread as one `RxFailed` reception. The
application still supplies what the `openthread` crate asks of a platform:
entropy, settings storage and the C library functions OpenThread links.

After an OpenThread role change, `frames::role_txrx_priority` gives the
coexistence priority of immediate transmission and reception ESP-IDF's
`handle_ot_role_change` sets (low with the receiver on when idle, middle for
a sleepy device); the composition maps it to its radio's coexistence levels,
as the Thread example does with `Ieee802154System::update_coexistence`.

## What the radio reports

The capabilities are the ones the radio keeps under this trait:

- PHY: ACK timeout, energy scan, transmission from sleep, transmit
  security and timed transmission and reception, as ESP-IDF's OpenThread
  port reports them (`otPlatRadioGetCaps`). The CSL accuracy and
  uncertainty are the port's defaults, 50 ppm and 500 microseconds.
- MAC: hardware acknowledgement in both directions, PAN ID, short and
  extended address filtering, promiscuous mode and source matching. A
  disabled source-match table answers every poll with frame pending; an
  enabled one uses the enhanced pending mode ESP-IDF's port selects for
  Thread 1.2 and later. Table changes reach the radio entry by entry, and a
  full table is no error, as in the port.

As over ESP-IDF's radio, OpenThread's `SubMac` runs CSMA-CA backoffs and
retries itself; each of its attempts reaches the radio as one transmission,
with a CCA when OpenThread asks for one, so the radio's own CSMA-CA and
retries stay unused. The CCA uses OpenThread's energy threshold: the radio
starts from the reported default and takes each threshold
`otPlatRadioSetCcaEnergyDetectThreshold` sets before the next CCA, as the
port hands it to `esp_ieee802154_set_cca_threshold`. The radio secures each attempt
as the port's `otPlatRadioTransmit` does: a new frame counter and the
current key index for a first transmission, the frame's own for
`SubMac`'s retransmission, nothing for a frame OpenThread secured itself.
The frame counter and key index it wrote go back into OpenThread's frame,
where `SubMac` reads the used counter. 2015 frames get enhanced ACKs,
secured ones with the same keys and counter; the received frame reports the
ACK's counter and key index and its frame-pending bit, as the port's
receive information does. Unlike the port, which reports the counter after
the one the ACK carried, the radio reports the one the ACK carried. The
radio acknowledges no second short address.

`otPlatRadioGetRssi` reads the composition's live RSSI reader, over the
runtime's `Ieee802154RadioPort::recent_rssi`, as the port reads
`esp_ieee802154_get_recent_rssi`: the low byte of the shared baseband's
current receive information, of the most recent reception whichever
protocol received it; OpenThread's invalid RSSI while no radio is
installed.

The radio clock is the port's `clock` (over the runtime, ESP-HAL's
microsecond clock, where ESP-IDF's port reads `esp_timer`); OpenThread's 32-bit radio times are taken as the nearest
instant of it. A CSL receiver's `otPlatRadioReceiveAt` becomes a scheduled
receive window, outside which the radio sleeps; the first frame of the
window ends it, as in the vendor driver. `otPlatRadioEnableCsl` and
`otPlatRadioUpdateCslSampleTime` set the port's CSL state (`RadioSetting::Csl`): enhanced ACKs
then carry a CSL IE, every CSL IE the radio sends gets the period and the
phase to the next sample time when its SFD goes out, and retransmissions
take a new frame counter, all as the port does. A CSL transmitter's delayed
frame becomes a scheduled transmission, with a CCA when OpenThread asks for
one.

A Link Metrics subject's probing initiators
(`otPlatRadioConfigureEnhAckProbing`) reach the port's enhanced-ACK
generator (`RadioSetting::EnhancedAckProbing`), whose ACKs to an initiator carry the Thread enhanced-ACK
probing IE with the frame's metrics, as the port's generator adds
`otLinkMetricsEnhAckGenData`. Link margins are measured from the receive
sensitivity the radio reports; ESP-IDF's port never sets the noise floor
of its `link_metrics.cpp` utility, so its margins are zero.

With Thread network time synchronization, a frame's Time IE gets the time
sync sequence and the radio clock plus OpenThread's network time offset
when its SFD goes out, as the port's `ot_radio_transmit_sfd_done` writes
them; OpenThread reads the same clock (`otPlatTimeGet`).

Frames that arrive during a transmission or energy scan wait in a bounded
queue for `receive`; a full queue drops the newest. A transmission whose
future OpenThread drops finishes in the backend, and the next operation waits
for its end.

## Build

The adapter needs no OpenThread C library: `openthread-radio` holds the
trait alone, so the adapter builds and is tested on the host, where its tests
drive the trait as OpenThread does over the Espressif runtime and the
engine's register model, and over a host model of the port that has no
runtime, engine or chip behind it. The application's `openthread` crate compiles OpenThread from source
for the chip target through `openthread-sys`, which needs system-installed
CMake, Clang and libclang; host builds of the workspace do not compile it.

## Limits

The HIL cell `ieee802154-thread-exchange` attaches an end device over the
adapter to a network ESP-IDF's OpenThread leads and exchanges UDP both ways;
CSL, time synchronization and secured enhanced ACKs are not exercised on air.
The
[Thread example](../../../../examples/esp32s31/thread/README.md) runs an
OpenThread end device over it.
