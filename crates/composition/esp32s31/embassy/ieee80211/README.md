# ESP32-S31 Wi-Fi product integration

`oer-esp32s31-ieee80211-system` composes the public radio lifecycle,
static resources, IRQ bindings and selected network adapter. Applications own
board identity, credentials, IP configuration and sockets. The
[network implementation guide](../../../../../docs/network-implementations.md)
explains the external crates, reasons for patches and complete build commands.

## Start here

Applications call the crate's `new` entry point once and receive a
`RadioInstance` plus the sole `SystemRunner`. Splitting `RadioInstance` yields
the hardware-free `WifiControl`, network devices, monitor stream and status
observers. The runner, not `WifiControl`, retains the stopped PHY/MAC owner,
DMA arenas and IRQ route while it is spawned. Internal crates do not depend on
the `oer` facade; the facade reexports their application-facing contracts.
This crate also reexports every input of `new`, `RadioConfig` and
`WatchdogConfig`: `EspHalRadioPeripheral`, `PhyCalibrationIdentity`,
`phy_get_rf_cal_version`, `DeadlineWatchdog`, `DeadlineBudget` and the
`await_stack_boundary!` poll boundary. Applications therefore need no direct
PHY, SoC or esp-hal adapter dependency; board startup and the executor remain
application-owned.

Use the buildable [station](../../../../../examples/esp32s31-station/),
[access-point](../../../../../examples/esp32s31-access-point/) and
[monitor](../../../../../examples/esp32s31-monitor/) applications as the
copyable entry points. They are compiled for the repository's pinned
ESP32-S31 target and exact Cargo feature profiles by the documentation gate.

## Wi-Fi lifecycle

The normal station path crosses these distinct ownership boundaries:

| Boundary | Owner and meaning |
| --- | --- |
| Cold acquisition and PHY registration | Composition claims the PAC/platform roots, selects or produces calibration state, initializes the MAC, and constructs the driver's `WifiStopped`. No application role is active. |
| Request planning | `oer-radio` validates the station request while the actor still holds `WifiStopped`. A rejected request is returned with `WifiControl`; no PAC, DMA or IRQ owner moved. |
| Role materialization | The concrete runner consumes `WifiStopped`. The station epoch takes the register owner and inactive IRQ token, installs its DMA/task graph, and acknowledges start only after that graph is owned. |
| Connected data | Network adapters lend packet storage; the chip datapath owns DMA publication and terminal return. A control response does not release a frame or descriptor. |
| Maintenance | The runner pauses role activity, detaches/admit-checks IRQ access and consumes the logical PHY owner across tracking. It restores MAC state and releases checked maintenance access before resuming publication. |
| Stop and restart | Stop must recover the exact DMA/task owner and IRQ setup token. Only their reunion with the logical owner reconstructs `WifiStopped`; the next role may then be planned. |

AP uses the same single physical owner but different role policy and queues.
Same-channel STA+AP is one combined role epoch, not two independently
restartable radios. A station maintenance path or qualification result does
not automatically establish the corresponding AP or combined-role property.

### Failure and cancellation

- Dropping an application wait does not cancel a command already published to
  the supervisor. The actor continues, and its mailbox drains the stale reply
  before any later internal request. The consumed public typestate is not
  recreated by dropping its future.
- A planning rejection occurs before materialization and returns the original
  request and idle control capability. Errors after owner movement retain the
  physical frontier, either in `Faulted` or through terminal system escalation;
  neither returns a restartable role.
- Maintenance failure after the role is paused does not promise restoration.
  Admission failure, PHY failure and checked-access release failure retain
  different owner frontiers; failure or cancellation after the hardware edge
  requires reset rather than reuse.
- Connected and stopped-role maintenance request system reset for an invalid
  PHY, unconfirmed MAC/RX stop or failed hardware restoration. The composition
  borrows the retained failure until the SoC adapter's diverging reset request;
  it does not await an application response or release DMA/IRQ owners first.
  Admission rejection, peer-notification errors and ownership failures at an
  already-stopped MAC/paused RX boundary keep their rejection/quarantine paths.
  Explicit release/cycling and restart also escalate ambiguous RF close/wake,
  registration without completed cleanup, and failed initial tracking/channel
  execution. Preparation failures, completed registration cleanup, closed-RF
  reunion failures, remaining shared clients and unexecuted pending tracking
  retain their exact non-runnable lifecycle owners without reset.
  Initial cold start uses the same classification; even a non-escalated start
  failure is retained in the lifecycle fault slot rather than dropped when
  reporting `NewError::RadioStart`.
  Radio drivers do not own watchdog/reset peripherals. `RadioConfig` requires
  caller-owned static `WatchdogConfig` storage binding the SoC TIMG1 service
  and explicit startup, maintenance and shutdown budgets. A lease covers the
  full physical operation (including quiescence and restoration); cancellation
  cannot disable it. The shutdown budget also covers a retained close/wake
  cycle. No qualified defaults or measured RF-stop time bound are supplied.
  Cold start arms before the physical driver entry and completes at its ready
  owner. Connected maintenance arms before requesting TX drain; it completes
  only after checked RX/MAC/IRQ restoration and worker handoff, or a retained
  safe rejection. Radio restart uses a shutdown lease through cold release and
  a separate startup lease through owner reconstruction. Retained close/wake
  uses one uninterrupted shutdown lease. Waiting for an automatic maintenance
  demand does not arm a physical lease.
- Releasing a stopped Wi-Fi PHY client while Bluetooth or IEEE 802.15.4 still
  has a client returns a physically powered shared radio. A final client may
  run RF close and cold reunion. Once that async close is polled, it must reach
  a terminal result; dropping it can strand a partially closed RF epoch.
- A retained close/wake cycle is admitted only for the final client and returns
  a new `WifiStopped` only after wake, client reacquisition, tracking and MAC
  receive-policy restoration. It is not connected modem sleep or peer-facing
  Wi-Fi power save.

The item-level contracts are in `oer-radio-embassy`,
`oer-esp32s31-ieee80211::runtime`, and the concrete role modules in this crate's
source. `FEATURES.md` is navigation for scope and limitations; the generated
qualification view remains the readiness authority.

Select exactly one network feature with `default-features = false` when
replacing the default:

| Feature | Product contract |
| --- | --- |
| `owned-network` (default) | Maintained Embassy/Xarxa contract with explicit packet pools |
| `upstream-network` | Original Xarxa driver contract; the application supplies its stack |
| `embassy-network` | Released Embassy/smoltcp token contract |

Both `--network upstream-xarxa` and `--network patched-xarxa` in repository
builders select `upstream-network`. Their difference is the application graph's
Xarxa source, not a second product feature. Source overrides belong to the
consumer workspace; selecting this library feature alone does not apply a patch.

The [packet ownership contract](../../../../../docs/wifi-egress.md) defines
adapter and physical scheduler boundaries. Static dimensions and the one-time
resource claim belong to this crate; reusable adapters supply storage types.
All network selections share the ESP32-S31 hardware dependencies. Example and
HIL support is listed separately in the implementation guide: a library feature
does not establish that every application role has been qualified.

Applications that construct their own stack can consume `WifiDevice`
through `into_upstream()`, `into_embassy()` or `into_owned()`, according to the
selected contract. The owned transfer returns its matching packet allocator
alongside the unique device. This permits application-owned stack composition
and observation without exposing hardware authority or cloning an endpoint.
