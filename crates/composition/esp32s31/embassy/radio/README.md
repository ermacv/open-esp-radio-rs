# ESP32-S31 shared radio system

`oer-esp32s31-radio-system` brings the shared ESP32-S31 radio up once for an
Embassy application. Every protocol composition (Wi-Fi, Bluetooth LE,
IEEE 802.15.4) joins the same `SharedRadio`: the radio arbiter over the
esp-hal radio platform and clock sources.

```rust,ignore
let platform = EspHalRadioPlatform::new(/* the eight radio peripherals */);
let (radio, partitions) = start(spawner, platform, RadioStart::new())?;
```

`start` claims the radio hardware, creates the radio in static storage and
spawns the two tasks the radio needs for its whole lifetime:

- periodic PHY tracking, as ESP-IDF's `phy_track_pll` timer runs it. With the
  default `Tracking::FailStop` a failed tick stops the program, because the
  PHY state after it is not known. `Tracking::Caller` leaves tracking to the
  caller, for example a HIL task that suspends it during a measurement;
- the coexistence schedule's phase timer.

`RadioStart::with_calibration_cache` replays a retained calibration at the
first PHY registration instead of calibrating. The returned
`ConcurrentPartitions` hold one partition per protocol; hand each to its
composition. A second call returns `RadioStartError::AlreadyStarted`.

The `oer` facade exposes the crate as `oer::systems::esp32s31::embassy::radio`
with the `owned-xarxa` or `embassy-ieee802154` feature.
