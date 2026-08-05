# ESP32-S31 hardware-in-the-loop platform

This is a private embedded workspace for qualifying the public driver crates.
It is deliberately excluded from the root host workspace: normal
`cargo test --workspace --all-targets` must not build target-only binaries.

This directory is the test harness, not another radio-driver layer. It owns
the concrete board clock tree, boot flow, flash and PSRAM placement, Embassy
executor, and the full `embassy-net`/smoltcp application used for traffic
tests. Reusable network-driver and ESP32-S31 platform bindings live under
`../../../driver/adapters`; PAC/HAL/PHY/Wi-Fi behavior lives under
`../../../driver/esp32s31` and `../../../driver/wifi`.

The authoritative performance profile is `psram-code-psram-data`. Its image
has two stages:

1. a Flash/SRAM bootstrap initializes and verifies external memory;
2. a separately linked runtime executes code and ordinary data from PSRAM,
   while its stack, DMA objects and interrupt closure remain in internal SRAM.

Board electrical settings, credentials, traffic generation and PASS/FAIL
reporting belong here. PHY/MAC/STA behavior belongs in `../../../driver` and must
be moved there before a new behavior is entered in the canonical feature
ledger.

HIL-only PHY observation is isolated in
`runtime/src/radio_hil/phy_diagnostics.rs`. It owns raw comparison snapshots,
observer breadcrumbs and UART evidence callbacks, but no radio transition.
The main `radio_hil.rs` file consumes that observer while keeping the station
ownership flow separate from diagnostic register inventories.

The `telemetry` member similarly owns the concrete atomic counters, IRQ-time
correlation and interval snapshots for the typed RX observations published by
the reusable Embassy adapter. It is chip-specific HIL policy, not a generic
driver dependency.

The closed vendor oracle is never a dependency of this workspace. Its isolated
verification workspace lives at
`../../../verification/vendor/targets/esp32s31/oracle-firmware` and is
reachable only through `cargo hil oracle ...`.

Compiled comparison probes are verification artifacts, not HIL members. Their
isolated workspace lives under
`verification/vendor/targets/esp32s31/probes`; it is never linked into board
firmware.

Common commands from the repository root:

```text
cargo hil scenarios
cargo hil doctor
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
cargo hil station ap-loss --serial /dev/ttyACM0
cargo hil station reconnect --serial /dev/ttyACM0
cargo hil build udp-tx
cargo hil traffic trigger <monitor-interface> --transmitter <bssid> --aid <aid>
cargo hil oracle build
cargo hil oracle flash --port /dev/ttyACM0
```

HIL builds use the git revisions recorded in `Cargo.lock`, even when a sibling
`../esp-hal` checkout exists. Set `ESP_HAL_ROOT` explicitly only while
co-developing against a local esp-hal tree. The runner validates the required
package directories and restores the embedded lock file byte-for-byte after
that opt-in build, so a local dependency experiment cannot dirty the radio
repository.

## Firmware scenarios

`cargo hil scenarios` prints the authoritative list. Each scenario has its
own directory below
`target/hil/esp32s31/psram-code-psram-data-<artifact-name>` and a
`scenario.txt` manifest containing the non-secret compile-time mode selection.
The runner removes inherited scenario-selection variables before applying the
named mode, so an old shell export cannot silently combine two HIL workloads.

| Scenario | Firmware workload | Host-side follow-up |
| --- | --- | --- |
| `boot-smoke` | bootstrap, Flash/PSRAM and stage-two runtime | inspect UART PASS marker |
| `radio` | production `ConnectedRunner` PHY/MAC/STA/WPA2 path | scenario-specific UART inspection |
| `udp-tx` | `embassy-net` device-to-host UDP throughput | provide the configured UDP receiver |
| `tcp-rx` | runtime-configured host-to-device TCP stream | `cargo hil traffic tcp-rx <device-ipv4>` |

The `radio` path uses one production `Esp32s31StaAttempt` for both the initial
join and every reconnect. Its concrete target owner composes
`StaJoinRunner`/`Esp32s31StaJoinPort` for Open Authentication and Association,
`Esp32s31StaPeerPort` for the accepted peer, `Wpa2HandshakeRunner` for the WPA2
Message 1/3 exchange, and `Wpa2KeyInstallRunner` for typed PTK/GTK publication,
Message 4 and rollback. A failed finite phase returns the exact RX, TX,
sequence and security owner to `StaLifecycleService`; HIL no longer carries a
second phase sequencer or a reconnect-only implementation of those steps.
`Esp32s31ControlTx` owns Probe, Authentication, Association, EAPOL and the
protected bootstrap publication until it transfers the same pinned descriptor,
EDCA state, calibrated power and executor adapters to `Esp32s31SingleMpduTx`.
The join port binds the PAC, retained RX frontier, fixed DMA/scratch storage
and control TX in the production Embassy adapter. HIL supplies station
policy and diagnostic observer callbacks only; it no longer parses join RX,
builds HE power fields or publishes Authentication/Association frames.
`Esp32s31StaPeerPort` then owns both sides of the association-time peer
transition: it installs scan-advertised HT/WMM/HE policy before the request,
consumes the accepted response to program HE peer/AID/BSR and rate-control
hardware, and returns `Esp32s31ConnectedStaPeer` by value. Initial and
reconnected HIL paths consume that same owner and only report its value-only
diagnostics; they do not recreate peer policy or a private connected-link
model.
`Esp32s31ConnectedStaPort` consumes that peer and one coherent connected
configuration. It owns rate selection, staged RX dispatcher/protocol policy,
the control-to-ordinary/A-MPDU TX handoff, TX/RX BlockAck and beacon control,
and final `Esp32s31ConnectedServices` assembly. HIL provides fixed resources,
`embassy-net` endpoints, task placement and observations; it does not recreate
those driver constructors. `Esp32s31StaTxEpoch` now retains the ordinary TX
policy while connected service owns its descriptor. Pre-connected RX consumes
its own live promotion, while `Esp32s31ConnectedStaTeardownPort` orders control
shutdown, RX-DMA stop, TX return and PTK/GTK clear. HIL only reports the typed
result and restores its network fixture. `Esp32s31MacInterruptEpoch` plus the
ESP-HAL route now own stable PAC storage, CPU route activation/quiescence,
hard-handler service and Embassy wake drain. HIL retains only handler
observations, task placement and stop signals. Protocol v7 correlates each HIL
cycle request with a reliable completion covering runner return, scan-owner
return, fresh join and replacement connected-runner startup. The task-stop
deadline is no longer HIL policy:
`stop_esp32s31_connected_task_group` requests the fixture-defined task group,
returns its exact stopped owner, and otherwise reports `ResetRequired`. HIL
implements only the benchmark/protocol signals and maps that terminal outcome
to evidence. Executing a complete platform radio reset from the returned
reset-required frontier remains recovery work.
Absolute Embassy deadlines, retry state, RX-before-timeout ordering and
live-ring/TX ownership are shared driver behavior rather than parallel
test-only loops. `Esp32s31Wpa2HandshakePort` and `Esp32s31Wpa2KeyPort` likewise
own EAPOL extraction, M2/M4 publication and atomic PTK/GTK slot ownership.
After M4, HIL transfers those owners directly to the connected runner; it no
longer runs a separate protected-ARP TX/RX loop. The connected handoff wraps
the ordinary TX owner in
`Esp32s31ConnectedTx`: referenced A-MPDU leases, BlockAck retry and the
beacon-loss deadline now run on this one production runner. The opt-in
connected power planner also ACK-gates PM=1 and restores PM=0 before queued
network data, but the HIL does not enable it or consume its doze permit. Actual
modem sleep remains disabled until its complete PAC sleep/wakeup transaction
is qualified.

`cargo hil station reconnect` provisions credentials over the current HIL
protocol and
requests one or more hardware-safe `ConnectedRunner` stops. `--cycles 3`, for
example, requires three request-correlated typed completions covering the
runner/teardown stop, returned scan owners, fresh Open Authentication and
Association/WPA2, and the replacement connected runner on selected same-SSID
candidates. Text output is diagnostic-only. The stop has its own `Stopped`
outcome and is converted by the
qualification adapter into the distinct `CycleRequested` edge; it is never
reported as beacon loss. The outer lifecycle then enters its `RunningScan`
owner with `refresh_candidate=1`. This qualifies repeated resource reuse and
controlled rescan/re-authentication, not recovery after an AP disappears.
`cargo hil station ap-loss` is the separate real peer-loss cell. It controls
the repository HE20 hostapd fixture, requires reliable generation-zero
`Connected` and `BeaconLoss` events, restores the AP, and accepts only a new
generation-one `Connected` event. A guard restores the host interface to
managed mode on every normal or error return.
`cargo hil station ap-absence` keeps the same controlled AP down after the
typed `BeaconLoss` edge. It then requires all three generation-one
`AttemptFailed(CandidateSelection, NoCandidate)` events followed by the exact
typed `RetryExhausted` edge. This tests the bounded outer policy rather than
merely waiting for a timeout or matching UART text.
`cargo hil station tx-fault` uses the dedicated
`cargo hil flash station-tx-fault` image. It arms a one-shot fault only after a
real connected network TX has published its descriptor, requires the
production reset-required error plus the returned runner/task/RX frontier,
and confirms that the TX slot remains quarantined. It then cold-resets the
board and requires a fresh network-ready epoch against the same controlled AP.
This does not claim that an in-place platform radio reset exists.
The finite PHY/RX/TX/dwell/candidate transaction is composed by the reusable
`Esp32s31ScanPort` for both the initial cold scan and later running rescans.
The cold binding carries `ColdRadioRegisters`; the running binding carries the
cooperative connected-epoch owners and quiesced interrupt token. HIL retains
only the board epoch bundle, station policy, fixed storage and UART evidence
observer; it no longer implements a private cold scan port.
`Esp32s31PreconnectedRx` likewise owns the halted/prepared/live RX frontier
across Authentication, Association and WPA2; HIL provides its fixed buffer
storage but no longer carries a parallel RX type-state machine or descriptor
walk/recycle implementation.
`Esp32s31DisconnectedStaEpoch` and `Esp32s31ReconnectedStaEpoch` own the
complete reusable resource transition around running scan. HIL retains only
the initial static-cell promotion and the board evidence/policy adapters.

Build and flash a named scenario with the same identifier:

```text
cargo hil build radio
cargo hil flash radio --port /dev/ttyACM0
```

The former raw-MAC, A-MPDU/A-MSDU and HE matrix scenarios depended on the
second connected event loop that predated the production backend. They are
intentionally absent from `cargo hil scenarios`; selecting their old build
environment fails at compile time. Reintroduce each workload only after its
aggregate/HE transaction is owned by the production backend and scheduled by
`ConnectedRunner`, so a HIL result cannot accidentally qualify a parallel driver.

SSID, passphrase, peer address, channel and bounded rate/MCS overrides remain
external HIL configuration. They are intentionally not written to
`scenario.txt`, because that file must not capture credentials. A scenario
name selects the workload and artifact identity; it does not claim that the
connected AP can satisfy every cell in that workload.

Set `OPEN_RADIO_HIL_STARTUP_ARTIFACT` to a caller-owned file when qualifying
retained PHY calibration. A missing file selects full calibration and is
created from the record returned by the target. A subsequent reset uploads the
same record before network provisioning, allowing the HIL report to distinguish
`FullForRecord`, `FullAfterRejectedRecord` and `PartialRestored` and compare
their elapsed time. The target stores no copy in NVS or flash; eFuse identity
is read by the ESP32-S31 HIL adapter through the PAC and remains outside the
PHY state machine.

STA connection scans all ESP32-S31 2.4-GHz channels automatically.
`OPEN_RADIO_STA_CHANNEL` is only a preferred first-channel hint for controlled
setups; a stale hint cannot pin the connection to the wrong channel.

The root-owned AP/monitor helper and its narrow sudo policy live in
`hil/host/linux-net`. Reinstall it from this repository after changing the
helper or moving the checkout:

```text
cargo build -p open-esp-radio-hil-runner
sudo hil/host/linux-net/install.sh
```
