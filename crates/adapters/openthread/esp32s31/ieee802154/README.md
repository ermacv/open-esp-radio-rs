# ESP32-S31 IEEE 802.15.4 OpenThread radio

`oer-esp32s31-ieee802154-openthread` implements the `Radio` trait of the
[`openthread`](https://crates.io/crates/openthread) crate (0.4) over the
ESP32-S31 IEEE 802.15.4 runtime, so the OpenThread stack that crate binds can
run on the composed client.

## Use

Start the IEEE 802.15.4 composition, then hand its runtime to
`OpenThreadRadio::new` with the transmit power, CCA threshold and receive
sensitivity to report, and give the radio to the `openthread` crate. The
radio enables the runtime, installs an enhanced-ACK generator and serves
OpenThread's operations through the runtime's commands and events. The
application still supplies what the `openthread` crate asks of a platform:
entropy, settings storage and the C library functions OpenThread links.

## What the radio reports

The capabilities are the ones the radio keeps under this trait:

- PHY: ACK timeout, energy scan and transmission from sleep, as ESP-IDF's
  OpenThread port reports them (`otPlatRadioGetCaps`).
- MAC: hardware acknowledgement in both directions, PAN ID, short and
  extended address filtering, promiscuous mode and source matching. A
  disabled source-match table answers every poll with frame pending; an
  enabled one uses the enhanced pending mode ESP-IDF's port selects for
  Thread 1.2 and later.

The trait passes no MAC keys, frame counters, per-frame CSMA-CA or retry
parameters and no enhanced-ACK content. OpenThread's `SubMac` therefore runs
CSMA-CA backoffs and retries itself, as it does over ESP-IDF's radio, and
secures frames in software; each of its attempts reaches the radio as one
transmission, with a CCA at the radio's own threshold when OpenThread asks
for one. The radio's transmit security, CSMA-CA and retries stay unused. 2015
frames get unsecured enhanced ACKs; secured 2015 frames get none, and the
radio acknowledges no second short address.

Frames that arrive during a transmission or energy scan wait in a bounded
queue for `receive`; a full queue drops the newest. A transmission whose
future OpenThread drops finishes in the runtime, and the next operation waits
for its end.

## Build

OpenThread is compiled from source for the chip target by `openthread-sys`,
which needs system-installed CMake, Clang and libclang; host builds of the
workspace do not compile it.

## Limits

The adapter is not qualified on air, and no composition runs an OpenThread
instance yet.
