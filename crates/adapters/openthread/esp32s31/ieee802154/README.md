# ESP32-S31 IEEE 802.15.4 OpenThread radio

`oer-esp32s31-ieee802154-openthread` implements the `Radio` trait of the
[`openthread`](https://crates.io/crates/openthread) crate over the ESP32-S31
IEEE 802.15.4 runtime, so the OpenThread stack that crate binds can run on
the composed client.

It depends on the repository's fork of that crate,
[`ermacv/openthread`](https://github.com/ermacv/openthread/tree/oer/radio-security)
at a pinned revision on branch `oer/radio-security`, based on the 0.4.0
release. The fork extends the trait with what OpenThread hands a radio that
claims `OT_RADIO_CAPS_TRANSMIT_SEC` and reads back from it: the MAC keys and
frame counter (`set_mac_keys`, `set_mac_frame_counter`), each frame's
transmit information (`transmit_frame`: `mIsARetx`, `mIsSecurityProcessed`,
and the frame counter and key index the radio wrote), the acknowledgement
sent for a received frame (`PsduMeta::ack`), the capabilities declared
before the instance is built (`OtResources::set_radio_caps`) and, with its
`csl` feature, Coordinated Sampled Listening: the radio clock
(`Radio::clock`), scheduled receive windows (`Radio::receive_at`), the CSL
receiver state (`Radio::set_csl`), delayed transmission (`TxFrame::tx_at`)
and receive SFD times (`PsduMeta::timestamp`).

## Use

Start the IEEE 802.15.4 composition, then hand its runtime to
`OpenThreadRadio::new` with the transmit power, CCA threshold and receive
sensitivity to report, and give the radio to the `openthread` crate. The
radio enables the runtime, installs the port's zeroed MAC keys and an
enhanced-ACK generator and serves OpenThread's operations through the
runtime's commands and events. Declare `OPEN_THREAD_RADIO_CAPABILITIES`
with `OtResources::set_radio_caps` before building the OpenThread instance:
OpenThread's `SubMac` reads the capabilities once, when the instance is
built, and leaves transmit security to the radio only when it saw it there. The
application still supplies what the `openthread` crate asks of a platform:
entropy, settings storage and the C library functions OpenThread links.

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

The radio clock is the runtime's (ESP-HAL's microsecond clock, the port's
`esp_timer`); OpenThread's 32-bit radio times are taken as the nearest
instant of it. A CSL receiver's `otPlatRadioReceiveAt` becomes a scheduled
receive window, outside which the radio sleeps; the first frame of the
window ends it, as in the vendor driver. `otPlatRadioEnableCsl` and
`otPlatRadioUpdateCslSampleTime` set the runtime's CSL state: enhanced ACKs
then carry a CSL IE, every CSL IE the radio sends gets the period and the
phase to the next sample time when its SFD goes out, and retransmissions
take a new frame counter, all as the port does. A CSL transmitter's delayed
frame becomes a scheduled transmission, with a CCA when OpenThread asks for
one.

Frames that arrive during a transmission or energy scan wait in a bounded
queue for `receive`; a full queue drops the newest. A transmission whose
future OpenThread drops finishes in the runtime, and the next operation waits
for its end.

## Build

OpenThread is compiled from source for the chip target by `openthread-sys`,
which needs system-installed CMake, Clang and libclang; host builds of the
workspace do not compile it.

## Limits

The adapter is not qualified on air. The
[Thread example](../../../../../examples/esp32s31/thread/README.md) runs an
OpenThread end device over it.
