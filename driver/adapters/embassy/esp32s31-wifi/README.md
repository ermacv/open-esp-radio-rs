# ESP32-S31 Wi-Fi / Embassy adapter

This crate owns the reusable async composition between ESP32-S31 Wi-Fi
hardware owners and a bounded Embassy network adapter. It does not own board
startup, task placement, credentials, sockets, traffic generation or HIL
reporting.

The connected RX path exposes an optional typed `RxPipelineObserver`. This
crate defines when semantic pipeline events occur, but does not choose
counters, sampling, report formats or IRQ correlation. Those concrete
qualification policies belong to the consuming HIL.

Associated-peer policy is not an Embassy concern. Candidate preparation and
WMM/HT/HE/rate-control programming live in
`open_esp_radio_esp32s31_wifi_sta::peer`; this crate contains only the private binding
from its narrow transmit trait to the async control-TX owner.

Likewise, STA TX entropy, calibrated-power and timer contracts plus their
resource bundle live in `open_esp_radio_esp32s31_wifi_sta::tx`. This crate
supplies only `tx_time::EmbassyWifiTxTimer`; it does not make Embassy time part
of station policy or Association power derivation.

The task-cooperative register facade also belongs to the chip composition,
not to Embassy. `open_esp_radio_esp32s31_wifi_sta::cooperative_hardware` owns
the RX/TX/key/peer/BlockAck trait implementations over the single
`RadioRegisters` cell. Embassy compositions consume that chip type directly;
the adapter does not mirror it through a compatibility module.

The station-wide TX epoch itself lives in
`open_esp_radio_esp32s31_wifi_sta::tx_epoch`. The local `sta_tx_epoch` module
is only an extension that creates and restores this owner using the concrete
async control transmitter.

The persistent PHY/channel owner is
`open_esp_radio_esp32s31_wifi_sta::channel::Esp32s31ScanPhy`. Local scan and
attempt modules only adapt it to their async transaction traits; they do not
own its state or channel-switch policy.

The finite scan transaction, its failure taxonomy and primitive port are in
`open_esp_radio_esp32s31_wifi_sta::scan`. This crate retains the concrete
PHY/RX-DMA/probe-TX composition and maps one neutral dwell tick to Embassy
time.

The role-neutral `rx_ring_owner` owns prepared/live/halted DMA-ring
transitions. The concrete scan adapter then has three non-overlapping pieces:

- `scan_rx` owns management-frame observation over that ring owner;
- `open_esp_radio_esp32s31_wifi_sta::scan_tx` owns active Probe Request
  publication and passive fallback;
- `scan_port` composes PHY, RX, TX, storage and the executor dwell timer;
- `scan_target` implements those ports for cold and cooperative RISC-V owners.

There is no separate `running_scan` facade: cold scan and reconnect scan use
the same `Esp32s31ScanPort` with different typed resource owners.

Ordinary retry/deadline ownership, single-MPDU encoding and the pre-connected
control transmitter likewise live in
`open_esp_radio_esp32s31_wifi_sta::{ordinary_tx,single_mpdu_tx,control_tx}`.
The same chip crate implements its join, peer and WPA2 transmit ports. The
adapter supplies only executor bindings such as `tx_time::EmbassyWifiTxTimer`;
no TX state machine is re-exported through this crate.

RX descriptor and DMA-buffer storage is executor-independent and lives in
`open_esp_radio_esp32s31_wifi_dma::rx_storage`. The local `rx_dma_service`
chooses the production large-RX dimensions and combines that arena with
reload waits, staging leases and optional observations. Network publication
is isolated in `network_rx`; the bounded borrowed-RX to owned-control handoff
is isolated in `control_mailbox`.

The two adapter-side RX actors now have responsibility names and private
phase modules:

- `rx_dma_service::{lifecycle,service}` owns stopped/prepared/live ring
  transitions and one finite DMA-to-staging service epoch;
- `connected_rx_protocol::owner` owns the staged queue, scratch and shutdown;
- `connected_rx_protocol::scheduler` owns Embassy queue/command/deadline
  arbitration;
- `connected_rx_protocol::reorder` owns RX BlockAck buffering and release;
- `connected_rx_protocol::dispatch` owns ordinary MSDU and A-MSDU publication.

The first actor is hardware-facing and never parses 802.11. The second has no
PAC access and cannot extend the DMA completion frontier. Their only shared
currency is an owned staging lease.

Connected aggregate TX is likewise separated from its optional measurements:
`aggregate_tx` owns pinned network leases, A-MPDU publication and completion;
`aggregate_tx_observer` defines only typed value events and the object-safe
observer contract. Concrete counters, histograms and interval snapshots live
in the HIL telemetry crate. Observer state cannot affect retry, queue or DMA
decisions.

`aggregate_tx` is a facade rather than one transaction-sized source file. Its
private modules follow the lifetime of one connected TX exchange:

- `owner` constructs the unique owner and returns its resources at station
  teardown;
- `publication` admits network frames, encodes the batch and publishes it;
- `completion` normalizes completion, BlockAck retry, collision and timeout;
- `resources` owns release, quarantine and the fail-closed `Drop` path;
- `adapters` binds that owner to the connected-control and network-service
  runtime contracts;
- `tests` contains the host transaction fixture and ownership regressions.

This is still an Embassy integration component because it retains pinned
`embassy-net` leases. Register programming and descriptor state remain in the
chip MAC/DMA crates; the module split does not introduce another MAC layer.

Interrupt integration has the same boundary. `embassy_irq::mac_runtime`
coalesces classified MAC work into RX/TX executor wakes,
`power_runtime` preserves an opaque acknowledged power-event image, and
`epoch` owns activation/quiescence of the platform route plus stale-wake
draining. Hardware status reads, acknowledgement and classification remain in
the chip MAC crate.

Connected-station control has a similarly explicit integration seam.
`open_esp_radio_esp32s31_wifi_sta::connected_control` owns the complete
association-scoped BlockAck, beacon-loss and power-save state machine plus its
runtime-neutral TX and reorder-command contracts. Its fields remain private;
one synchronous `service_step` consumes at most one delivered event and
performs one bounded transition. The adjacent
`connected_control_hardware` module owns the chip-specific TSF, RX BlockAck
and HE-TID contract. This adapter's `connected_control` module owns only the
Embassy receiver, deadline `select` and bounded reorder sender, while
preserving the former application-facing type names.

Connected-epoch composition is a separate phase from execution.
`connected_sta_port::plan` validates VIF, rate, BlockAck and beacon policy
before moving an owner. `composition` binds the already validated RX, TX and
control resources, while `resources` contains the named handoff vocabulary.

`connected_runner` then owns execution only. Its private `owner` module
constructs and returns the network/service graph, `service` owns bounded RX/TX
progress, and `arbitration` owns Embassy priority, deadlines and
cancellation-safe stopping. `connected_services` remains the finite RX,
control and network-TX capability graph consumed by that loop.

The scan path follows the same ownership boundary. `rx_ring_owner` owns the
typed DMA phase machine, while `scan_rx::running` only retains the surrounding
connected-epoch resources across a finite rescan. The peer `monitor_rx`
publishes normalized borrowed views without scan semantics. Independent
capture storage and its async consumer live in the chip-neutral
`open-esp-radio-wifi-embassy` crate. `scan_port::owner` holds and returns the
composed owners, `service` implements scan sequencing, and `bindings` adapts
the concrete RX/TX owners to that service contract.

Standalone monitor execution is exposed through the public `monitor` module.
Its private service owns `RxDma`, normalized RX, the non-blocking capture sink and one
`Esp32s31MacInterruptEpoch`. The run future distinguishes synthetic handoff
wakes from actual ISR posts, quiesces the route before stopping the DMA walker
and retains every owner. The application sees only a controller mailbox;
dropping that handle cannot affect hardware owned by the long-lived monitor
task. `controller.stop().await` publishes a request and waits for a separate
terminal acknowledgement. That acknowledgement is `Stopped` only after the
IRQ route and DMA walker both confirm quiescence. A transient RX-DMA `Busy`
therefore yields cooperatively rather than becoming an error. A broken route or
ring invariant is reported as `Faulted` while the task retains the complete
hardware frontier for reset. The destructor is not the normal shutdown path:
it makes at most one non-blocking stop observation and retains active owners if
quiescence cannot be proved; it neither spins nor panics. That fault is sticky
in both the control mailbox and static RX arena: a later materialization is
rejected with `ResetRequired` instead of erasing the abandoned epoch.
After a clean acknowledgement, consuming `try_into_stopped` decomposes the
inactive interrupt epoch, discards only the halted RX-ring transaction and
returns `Esp32s31WifiStopped` together with reusable monitor memory, sink,
route and wake runtimes. Active or faulted hardware has no such consuming
transition and the complete task owner is returned unchanged.

WPA2 protocol deadlines and atomic key-publication rollback live in
`open_esp_radio_wpa2::runner`, while the executor-independent ESP32-S31
handshake/key ports live in `open_esp_radio_esp32s31_wifi_sta::wpa2`. The local
private `wpa2_time` and `wpa2_port` modules now provide only Embassy
time, retained DMA RX and control-TX bindings.

Open Authentication/Association deadlines and retry sequencing live in
`open_esp_radio_wifi_sta::join`. Its ESP32-S31 RX, control-TX, observer and
error contracts live in `open_esp_radio_esp32s31_wifi_sta::join`; the local
private `sta_join_port` only binds them to retained DMA RX and the concrete
control transmitter. Internally `rx` owns the pre-connected DMA adapter,
`resources` names borrowed radio/storage/station policy, `owner` owns their
return boundary, and `service` implements only `StaJoinBackend` sequencing.
Likewise the complete finite S31 attempt transaction and
its value-only input/report types live in
`open_esp_radio_esp32s31_wifi_sta::attempt`; private `join_time` plus the
concrete target facade retain only Embassy time and the DMA/TX owner graph.
Inside `sta_attempt_target`, `channel` binds channel switching, `resources`
names the caller-supplied owners, `owner` retains mutable attempt state, `port`
is the stateless trait handle, and `service` implements the finite attempt
phases.

The RX frontier shared by Authentication, Association and WPA2 is exposed by
the `preconnected_rx` facade. `time` contains only the Embassy settle-delay
adapter, `state` defines the halted/prepared/live/vacant owner vocabulary, and
`lifecycle` performs the finite DMA transitions and connected promotion.

The application-facing station entry points are in `station`:

- `station::command` owns the severity-ordered reconnect/disconnect/stop
  mailbox and its single consumer;
- `station::connected_epoch` coalesces those commands with peer loss only at a
  transaction-safe connected-runner boundary;
- `station::backend` owns command priority and the cancellable reconnect
  backoff. Board firmware and HIL now implement only the concrete finite
  scan/join/connected attempt through `Esp32s31StationAttemptRunner`; neither
  may replace the common controller protocol or report `Stopped` before its
  DMA and interrupt owners are quiescent;
- `station::lifecycle` owns the outer finite scan/join/connected/reconnect
  service and always returns its exact hardware owner. Its public boundary is
  `prepare_esp32s31_station_task`, which returns a hardware-free controller
  and one task-owned lifecycle. The names intentionally match standalone
  monitor materialization: requesting stop is not the same event as the task
  acknowledging quiescence. Cancellation of the station run future publishes
  `Faulted`, while lower DMA/IRQ owners remain fail-closed for reset. The
  station control resource preserves that poison and refuses another split;
- `stop_esp32s31_connected_task_group` returns all spawned-task ownership or a
  distinct reset-required outcome under one deadline.

Run the host-side composition example with:

```text
cargo run -p open-esp-radio-esp32s31-wifi-embassy --example station_service
```

The example deliberately supplies a small deterministic attempt runner. A
real RISC-V board application binds the same facade to
`Esp32s31StaAttemptTargetPort`, its static DMA/network storage, its executor
spawners and the ESP-HAL interrupt adapter. A following materialization pass
still needs to group that concrete scan/join/connected backend so normal board
code does not assemble its intermediate phase owners manually. Board memory
placement and task placement remain explicit policy rather than global HIL
state.
