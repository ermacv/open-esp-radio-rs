# ESP32-S31 shared radio system

`oer-esp32s31-radio-runtime` owns the shared ESP32-S31 radio on the chip, as
ESP-IDF's `esp_phy` component does: the [radio arbiter](../../../hardware/esp32s31/hal/src/shared_radio.rs)
with its [shared PHY domain](../../../hardware/esp32s31/phy/src/concurrent.rs),
the platform token the PHY target port borrows and the platform sources of
the modem clocks.

## Use

`RadioSystem::new` splits the radio for concurrent clients and returns the
Wi-Fi, Bluetooth and IEEE 802.15.4 partitions for their compositions. The
protocol compositions are clients of the system:

- `RadioSystem::lock` takes the arbiter lease together with the platform
  resources, waiting while another holder owns it, as ESP-IDF waits for its
  PHY lock. `RadioGuard::parts` lends the lease, the platform token and the
  clock sources for one radio transaction.
- `RadioGuard::prepare_phy` is the first-client half of `esp_phy_enable`: it
  registers the shared PHY domain once, with the calibration identity given
  to `new`, or wakes its closed RF.
- `RadioGuard::close_phy_if_idle` is the last-client half of
  `esp_phy_disable`: it closes RF when no client remains.
- `RadioSystem::run_tracking` is the vendor periodic `phy_track_pll` timer.
  Every tracking period it runs one tick (`RadioSystem::track`) under the
  domain's admission policy. The default vendor admission tracks with
  protocols running; the tracking graph brackets its RF-sensitive regions
  with the grant-protect request. Run it for the lifetime of the radio;
  `RadioSystem::run_tracking_observed` also reports every tick's result, and
  `RadioSystem::run_tracking_until` ends when a stop future completes, which
  it polls only between ticks so a started tick always finishes.

- `RadioGuard::suspend_ieee802154` and `RadioGuard::resume_ieee802154` are
  the vendor `ieee802154_rf_disable`/`ieee802154_rf_enable` pair around
  IEEE 802.15.4 sleep: the operational MAC route leaves the PHY client set
  while keeping its BTBB reference, RF closes when no client remains, and a
  wake reopens RF before the client re-enters. The vendor compiles this
  sleep only with modem retention and tickless idle, so a composition
  enables it explicitly.

The Wi-Fi, Bluetooth and IEEE 802.15.4 compositions are all clients of the
system.

## Limits

The system keeps no calibration cache across registrations or resets, and
does not compose coexistence policy. There is no modem retention or light
sleep: RF close keeps the registration and calibration, nothing else. A tracking failure
leaves the domain poisoned; the chip must be reset.
