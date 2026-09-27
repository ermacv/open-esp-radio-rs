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
  to `new` and the retained cache given to `RadioSystem::with_calibration_cache`,
  or wakes its closed RF. It reports `RadioPhyPrepared::Registered` with the
  registration's calibration path and fresh cache, `Woken` or `AlreadyOpen`.
  The domain stays registered across RF close and wake, as ESP-IDF's
  calibrated PHY does, so only the first preparation registers.
- `RadioGuard::calibration_cache` captures the registered domain's current
  state as the cache for the next cold registration.
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

- Coexistence: `RadioGuard::enable_coex`, `disable_coex`,
  `request_wifi_coex`, `request_bluetooth_coex` and `release_coex` are the
  vendor `coex_enable`/`coex_disable` and request/release calls on the
  arbiter's timer bank. `RadioGuard::set_coex_status_bits`,
  `clear_coex_status_bits`, `set_coex_interval` and `restart_coex_phases`
  drive the recovered time-slice schedule (`RadioGuard::coex_schedule`).
  `RadioSystem::run_coex_schedule` is its phase timer: run it for the
  lifetime of the radio. Each phase change publishes the phase to
  `RadioSystem::wifi_coex_phase` and `bluetooth_coex_phase`, which keep only
  the latest unread phase.
  `enable_coex` and `disable_coex` count the enabled radios as `coex_enable`
  does: the second radio starts coexistence (`RadioSystem::wifi_coex_started`,
  `bluetooth_coex_started` with `true`), falling back to one radio stops it
  for Bluetooth, and the last disable withdraws every request.
  `set_coex_wifi_channel` records the Wi-Fi channel for Bluetooth
  (`RadioSystem::bluetooth_wifi_channel`, `RadioGuard::coex_wifi_channel`),
  and `end_bluetooth_preemption` reports the end of a Bluetooth preemption
  to Wi-Fi (`RadioSystem::wifi_coex_preemption_end`). The schedule's period,
  interval, flexible period and phases are read through
  `RadioGuard::coex_schedule`.

The Wi-Fi, Bluetooth and IEEE 802.15.4 compositions are all clients of the
system.

## Limits

Persisting the calibration cache across resets is the caller's policy. No
protocol publishes its coexistence status or reacts to phases yet, so the
schedule stays at its all-default scheme. There is no modem retention or light
sleep: RF close keeps the registration and calibration, nothing else. A tracking failure
leaves the domain poisoned; the chip must be reset.
