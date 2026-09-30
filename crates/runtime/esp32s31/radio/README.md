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
  PHY lock. The wait sleeps on a waker: each dropped `RadioGuard` releases
  its lease and then wakes every waiting task, which race for the lease
  again, so no release is lost and nothing polls the arbiter.
  `RadioGuard::parts` lends the lease, the platform token and the clock
  sources for one radio transaction.
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

- `RadioGuard::suspend_wifi` and `RadioGuard::resume_wifi` are the vendor
  modem sleep's `wifi_rf_phy_disable`/`wifi_rf_phy_enable`: the Wi-Fi
  membership becomes a suspended token while Wi-Fi leaves the PHY client set,
  RF closes when no client remains, and a wake reopens RF before Wi-Fi
  re-enters. The registration and calibration stay across the sleep.

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
  `RadioGuard::coex_schedule`; `RadioGuard::coex_active_for` answers
  `coex_status_get` for one radio. `RadioSystem::wifi_coex_view` is a cell
  Wi-Fi reads without the lease: every dropped guard refreshes its copy of
  Wi-Fi's coexistence status, the schedule's periods, interval and first
  phase share, and the priority of event 0. `enable_ieee802154_coex` and
  `disable_ieee802154_coex` are ESP-IDF's `esp_coex_wifi_i154_enable` and its
  reverse: IEEE 802.15.4 enables coexistence and publishes its schedule
  status; its MAC priorities follow its own operation scenes.

The Wi-Fi, Bluetooth and IEEE 802.15.4 compositions are all clients of the
system. Under Embassy, [`oer-esp32s31-radio-system`](../../../composition/esp32s31/embassy/radio/README.md)
creates it once and spawns `run_tracking` and `run_coex_schedule`.

## Joining the shared radio

Each protocol is one portable `RadioClient` of
[`oer-radio-coex`](../../../radio/coex/README.md) and joins the shared radio
the same way, under one `RadioGuard`:

1. `RadioGuard::prepare_phy` makes the shared PHY ready (first-client half of
   `esp_phy_enable`).
2. `join_<client>` of `oer-esp32s31-phy` enters the PHY client set and
   returns the client's affine `<Client>PhyMembership` with its
   `ConcurrentAcquire` (whether tracking is due before RF use). Holding the
   membership is the proof of membership; `leave_<client>` consumes it, and
   after the last client left `RadioGuard::close_phy_if_idle` closes RF.
3. The client enables coexistence and publishes its status word
   (`CoexStatusType::from(client)`); its operation priorities are portable
   `CoexPriority` values that the backend maps onto its events and PTI.

| `RadioClient` | Join | Membership | Leave | Coexistence |
| --- | --- | --- | --- | --- |
| `Wifi` | Wi-Fi cold start calls `join_wifi` after `prepare_phy` | `WifiPhyMembership`; `suspend_wifi` turns it into `WifiPhySuspended` for modem sleep and `resume_wifi` back | `leave_wifi`, `leave_suspended_wifi` | `enable_coex`, status word `Wifi`, `request_wifi_coex` |
| `Bluetooth` | The Controller's `join_phy` calls `join_bluetooth` after `prepare_phy` | `BluetoothPhyMembership`, held by the joined Controller epoch | `leave_bluetooth` | `enable_coex`, status word `Ble`, `request_bluetooth_coex`; event priorities from `CoexistenceLevel` (`Baseline`, `Elevated`, `Critical` = `CoexPriority::Normal`, `Elevated`, `Critical`) |
| `Ieee802154` | `RadioGuard::join_ieee802154` runs `prepare_phy` and `join_ieee802154` | `Ieee802154PhyMembership` | `RadioGuard::leave_ieee802154` | `enable_ieee802154_coex`, status word `Ieee802154`; MAC PTI per scene from `Ieee802154CoexLevel` (`Idle`, `Low`, `Middle`, `High` = `CoexPriority::Idle`, `Normal`, `Elevated`, `Critical`) |

The three compositions still sequence these steps themselves; the table is
the shape a portable join/leave port would take, not yet a shared trait.

## Limits

Persisting the calibration cache across resets is the caller's policy. Wi-Fi
publishes its coexistence status and reacts to its phases. Bluetooth LE
publishes its status (advertising, scanning, connection) and enables
coexistence for each Controller epoch, but does not react to phases: the
vendor BLE Controller only logs them. IEEE 802.15.4 publishes its enabled
status and follows its own operation scenes, not the phases. There is no modem retention or light
sleep: RF close keeps the registration and calibration, nothing else. A tracking failure
leaves the domain poisoned; the chip must be reset.
