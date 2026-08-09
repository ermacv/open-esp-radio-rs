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
- `scan_port` composes PHY, RX, TX, storage and the port dwell timer;
- `scan_target` implements those ports for the runtime RISC-V register owner.

There is no separate reconnect-scan facade: the initial scan and reconnect
scan use the same `Esp32s31ScanPort`. The common cold-to-runtime transition is
completed before either scan starts.

Ordinary retry/deadline ownership, single-MPDU encoding and the pre-connected
control transmitter likewise live in
`open_esp_radio_esp32s31_wifi_sta::{ordinary_tx,single_mpdu_tx,control_tx}`.
The same chip crate implements its join, peer and WPA2 transmit ports. The
adapter supplies only port bindings such as `tx_time::EmbassyWifiTxTimer`;
no TX state machine is re-exported through this crate.

`station::Esp32s31StationRuntimeResources` is the common production
composition frontier used by the standalone station target and HIL. It keeps
three independently returned groups:

- PHY/platform/interrupt authority;
- DMA, TX epoch, scan storage and protocol scratch frames;
- board services such as task spawners, network bindings and observers.

The grouping is an ownership contract, not the complete station service.
`station::connected_start` is now the sole transition from the raw first-join
PAC owner or an exact reconnected epoch into the uniform cooperative-hardware,
live-RX, aggregate-TX and control owner set. Its initial resource factory runs
only once; publication and RX-arm failures return the exact reached frontier.
The chip-independent `open_esp_radio_wifi_embassy::station_network`
transition likewise consumes the network device exactly once, publishes the
connected link edge, and carries the resulting stack plus radio runner across
ESP32-S31 reassociation epochs without making that lifetime chip-specific.
Its sibling `connected_tasks` module owns the reusable parent/task split for
port shutdown. Only the task endpoint can publish the stopped protocol
owner; dropping either half before completion permanently poisons reuse, and
there is no signal-reset escape hatch.
Its task-group combiner also withholds that primary owner until every
auxiliary epoch task acknowledges a separate linear endpoint. HIL traffic can
therefore decorate the production service, but cannot manufacture teardown
completion with an unrelated event bit.
The connected service also owns atomic RX/TX/control graph construction, the
fixed interrupt activation plus durable RX handoff probe, and the ordered
runner -> IRQ -> port-task -> control/RX/TX/key teardown transaction. Both
success and every quarantined failure retain the network, task and exact driver
owners together. The remaining extraction work is board-facing task spawning
and the outer scan/join/WPA2/reconnect composer, leaving examples as board
wiring and HIL as protocol, measurement and fault-injection policy.

RX descriptor and DMA-buffer storage is port-independent and lives in
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
coalesces classified MAC work into RX/TX port wakes,
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
hardware frontier in quarantine. The destructor is not the normal shutdown path:
it makes at most one non-blocking stop observation and retains active owners if
quiescence cannot be proved; it neither spins nor panics. That fault is sticky
in both the control mailbox and static RX arena: a later materialization is
rejected as quarantined instead of erasing the abandoned epoch. The board may
choose reset, but the adapter neither requests it nor exposes a reusable owner.
After a clean acknowledgement, consuming `try_into_stopped` decomposes the
inactive interrupt epoch, discards only the halted RX-ring transaction and
returns `Esp32s31WifiStopped` together with reusable monitor memory, sink,
route and wake runtimes. Active or faulted hardware has no such consuming
transition and the complete task owner is returned unchanged.

WPA2 protocol deadlines and atomic key-publication rollback live in
`open_esp_radio_wpa2::runner`, while the port-independent ESP32-S31
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

`station::join` is the single production composition of that target. It
temporarily borrows PHY/platform, hardware, RX, TX and scratch from the outer
role owner, runs channel selection through WPA2 connected entry, and returns
only the pre-connected RX, station/security state, peer, keys and value-only
report after every hardware borrow has ended. The normal station image and HIL
use this same transaction; their observers can record evidence but cannot
reassemble or replace join policy. All of those inputs now enter as one
`Esp32s31StationJoinResources` owner bundle; neither consumer maintains a
parallel positional join ABI.

`station::scan` is the corresponding single composition for both the cold
candidate scan and the disconnected reconnect scan. It accepts one grouped
PHY/hardware/RX/control-TX/storage owner set, constructs the concrete scan
port, runs exactly one finite channel plan and returns every owner together
with a typed decision, TX summary and bounded telemetry. Production firmware
and HIL use this same transaction for both scan forms; only their observers,
failure policy and role-specific RX handoff differ.
The retry/terminal distinction for a failed production scan is also common:
failed probe publication and RX-stop are fail-closed terminal frontiers, while
the remaining owner-preserving failures may request a candidate refresh.

The RX frontier shared by Authentication, Association and WPA2 is exposed by
the `preconnected_rx` facade. `time` contains only the Embassy settle-delay
adapter, `state` defines the halted/prepared/live/vacant owner vocabulary, and
`lifecycle` performs the finite DMA transitions and connected promotion.

The application-facing station entry points are in `station`:

- `resource_profile::Esp32s31DefaultStationMemory` groups the default RX/TX
  DMA arenas, descriptor-address storage, scan scratch and station control
  domain behind one atomic `claim()`. A board cannot partially acquire these
  large statics or accidentally construct them on an async task stack;

- `station::command` owns the severity-ordered reconnect/disconnect/stop
  mailbox and its single consumer;
- `station::connected_epoch` coalesces those commands with peer loss only at a
  transaction-safe connected-runner boundary;
- `station::connected_transaction` is the single run-to-stopped transaction
  used by firmware and HIL. Observation is statically dispatched, exit policy
  is evaluated while the runner is intact, and no reusable owner is exposed
  before IRQ publication and every attached port task are quiescent. The
  classified exit remains attached through driver teardown;
- `station::backend` owns command priority and the cancellable reconnect
  backoff;
- `station::composer` owns the shared `InitialScan`/`InitialJoin`/`RunningScan`/
  `Reconnected` phase owner and the only production phase dispatcher. The cold
  phase carries only station identity; candidate selection creates the first
  join owner and cannot be bypassed by a caller-proven scan record. Board
  firmware and HIL both use `Esp32s31StationEngine`, including the same owned
  binary SSID and scan policy; hardware ports bind finite owner transactions,
  while a separate read-only observer carries HIL telemetry. Neither can
  replace the candidate-refresh contract or command protocol;
- `station::lifecycle` owns the outer finite scan/join/connected/reconnect
  service and always returns its exact hardware owner. Its public boundary is
  `prepare_esp32s31_station_task`, which returns a hardware-free controller
  and one task-owned lifecycle. The names intentionally match standalone
  monitor materialization: requesting stop is not the same event as the task
  acknowledging quiescence. Cancellation of the station run future publishes
  `Faulted`, while lower DMA/IRQ owners remain fail-closed. The station
  control resource preserves that fault and refuses another split; recovery
  is chosen only by the physical supervisor or board policy;
- chip-neutral `stop_connected_task_group` keeps the epoch in `Stopping` until
  every spawned task returns its exact owner. The optional deadline-bounded
  observation reports only `Pending`; it does not manufacture a reset frontier.

Run the host-side composition example with:

```text
cargo run -p open-esp-radio-esp32s31-wifi-embassy --example station_service
```

The example deliberately supplies a small deterministic attempt runner. A
real RISC-V board application binds the same facade to the common station join
transaction, its static DMA/network storage, its port spawners and the
ESP-HAL interrupt adapter. Production firmware and HIL now share the outer
phase dispatcher, exact owner vocabulary, concrete scan/join transactions and
the persistent network-owner start transition. Connected RX/TX/control and the
network runner now also enter one named assembly transaction; qualification
faults are a statically dispatched service decorator rather than a second
composition. Station shutdown and restart additionally share owner-preserving
runtime reclaim, phase restore and fresh-request rebind transactions. Rebind
discards an old candidate by returning to initial or running scan while
retaining persistent network, RX and aggregate resources. Task preparation
also returns its owner and runner when the control domain is busy or poisoned.
The physical supervisor polls the active role future in its own
local task: the complete radio owner is intentionally `!Send` and must not
cross an port-task boundary through a synchronized channel. The facade now
provides local drivers for both station and monitor futures plus the common
actor shell. A following extraction pass still needs to bind them to a default
production ESP32-S31 resource-profile implementation of the epoch-runner
contract so normal board code does not assemble intermediate owners manually.
Board memory placement and task placement remain explicit policy rather than
global HIL state.
