# Integration backlog

Verified against `hil/targets/esp32s31/runtime/src/radio_hil.rs` on 2026-08-04.

Composition roots own board clocks and boot, memory placement, the executor,
credentials and concrete `embassy-net` services. The normal root is
`examples/esp32s31-station`; HIL additionally owns qualification traffic,
fault injection and reporting. Reusable radio behavior belongs in a driver,
protocol or integration crate.

The production path now has reusable owners for generated PAC register leaves,
MAC IRQ dispatch, RX/DMA frontier and recycle, ordinary connected TX,
pre-connected management/EAPOL TX,
connected BlockAck control, the single `ConnectedRunner` event loop,
Authentication/Association through `StaJoinRunner`, and WPA2 Message 1/3
response handling through `Wpa2HandshakeRunner`. `Wpa2KeyInstallRunner` owns
request validation, PTK/GTK publication ordering, Message 4 completion and
rollback. Referenced HT/HE A-MPDU, partial BlockAck retry, one-MPDU HT retry
handoff and beacon-loss timing are now owned by that same runner. The HIL
consumes those owners; it no longer contains parallel
Authentication, Association, WPA2 response-deadline or key-install state
machines. Completed transfer history is archived in the
[2026-07-31 integration report](archive/integration/2026-07-31-esp32s31-rust-integration-audit.md).

Connected entry now has the same production boundary. The reusable
`Esp32s31ConnectedStaPort` validates one coherent peer/configuration plan,
selects ordinary and aggregate rates, constructs staged RX, ordinary/A-MPDU
TX and BlockAck/beacon control, and assembles the production backend. HIL only
supplies storage, network/executor adapters and scenario policy at this edge.
`Esp32s31ConnectedStaTeardownPort` now owns the reverse transition: control
shutdown, RX-DMA stop, idle TX/resource recovery and pairwise/group key clear.
`Esp32s31StaTxEpoch` retains the control construction policy while that owner
is lent to connected TX, and the pre-connected RX owner consumes its own
live-frontier promotion. HIL only maps typed failures to qualification output.
`Esp32s31MacInterruptEpoch` now owns the connected interrupt phase as well:
the ESP-HAL route lends stable MAC/power PAC owners to the hard handlers,
recovers them before draining Embassy wakes and returns the inactive setup
token for running scan. HIL retains handler observations and executor policy.
`Esp32s31ScanPort` now owns the complete cold and running scan transaction;
HIL supplies only the different typed hardware epochs, storage, policy and a
non-retaining evidence observer.

## 1. Completed: concrete WPA2 TX backend

`Wpa2HandshakeRunner` now accepts the still-live RX ring through a finite
backend, enforces the exact absolute Message 1 and Message 3 deadlines,
services RX before simultaneous timeout, transmits Message 2 only in response
to a peer Message 1, stops RX and returns a typed `Wpa2PendingKeyInstall`.
The HIL has deleted its former Message 3 receive/deadline loop.

`Wpa2KeyInstallRunner` now consumes `Wpa2PendingKeyInstall`, validates the
station PTK/GTK request, publishes both keys atomically through its backend,
builds and transmits the exact Message 4, rolls both keys back on every later
failure, and returns typed installed-key ownership for `ConnectedRunner`. Its host
tests include the complete successful M1/M2/M3/key/M4 transition, atomic
install failure and Message 4 rollback.

The HIL implementation now calls the shared management/EAPOL owner from
section 2. Credentials, diagnostics and the protected ARP end-to-end assertion
remain HIL policy. No RTOS event queue or vendor supplicant context is retained.

## 2. Completed: management/EAPOL TX transaction owner

`chips/esp32s31/wifi/sta::ordinary_tx` is now the single owner of the pinned ordinary descriptor,
EDCA/retry state, calibrated power, entropy and per-publication deadline.
`chips/esp32s31/wifi/sta::control_tx` provides typed Probe, Authentication, Association, unprotected
EAPOL and protected-data transactions before connection. It transfers the
same owner directly into `Esp32s31SingleMpduTx` after WPA2 Message 4.

The HIL-local `transmit_encoded_frame` and
`transmit_encoded_unicast_with_retry` state machines are deleted. `TxStorage`
is only a board fixture containing the movable production owner. Register
snapshots, synthetic ARP payloads and PASS/FAIL reporting remain HIL policy.

Host tests cover successful management publication, exact management versus
Voice-data PTI, retry of the same sequence with the Retry bit, fail-closed
deadline quarantine and preservation of association policy across the
control-to-connected ownership transition. A rejected handoff returns the TX
owner, key token, sequence owner and connected config together, so cancelling
a pre-connected TX future cannot silently lose a live DMA or crypto resource.

## 3. Completed production aggregate ownership; optional HE workloads remain

`Esp32s31ConnectedTx` now owns the ordinary descriptor and the descriptor-only
referenced A-MPDU arena together. It claims pinned `embassy-net` leases only
after ADDBA becomes operational, retains them through detach/BlockAck and
aggregate retry, and releases them at the first safe ownership edge. A single
missing HT MPDU moves into the private ordinary descriptor without allocating
a second sequence number or CCMP PN. The connected control owner mirrors
ADDBA response, rejection, timeout and DELBA state directly into this TX
scheduler.

`Esp32s31ConnectedStaPort` constructs this owner and the HIL runs it through
the same `ConnectedRunner`; both debug and optimized RISC-V images link
successfully. Host
tests cover full BlockAck release, partial-BlockAck compaction/republication
and the individual retry handoff. The former raw-MAC, A-MSDU and HE matrix
traffic scenarios remain disabled until their workload/reporting layer is
reattached to this owner and real-board results are repeated. Historical HIL
evidence does not by itself qualify the new runtime owner.

Connected RX now publishes typed Beacon/TIM observations. An executor-clock
monitor uses the association beacon interval and a finite miss limit; a beacon
received on the exact deadline wins because RX is serviced first. Expiry
stops TX BlockAck sessions, publishes `embassy-net` link-down and returns the
runner.

The first finite power-save hardware slice is now closed. Ten generated PAC
leaves and a focused exact-effects baseline qualify the connected modem
beacon-miss counters, sleep-limit wake gates, wake-protect lead time and TBTT
auto-period transaction. Two additional generated leaves qualify WDEVPWR
STATUS/CLEAR, and the hard ISR now retains the complete acknowledged image in
an opaque Embassy handoff. HIL explicitly writes a zero WDEVPWR enable mask
and remains always awake.

The chip-independent STA path now encodes an exact legacy Null Data PM frame.
The production connected transmitter publishes it through the same pinned
descriptor, bounded retry owner and final ACK outcome as ordinary/control
traffic. A pure Embassy policy refuses a doze permit until PM=1 was
acknowledged, fails closed for missing or inconsistent TIM/DTIM, buffered or
pending traffic, and expresses the wake deadline in the STA TSF clock domain.
The coherent `hal_get_sta_tsf` ROM transaction is represented in SVD/PAC and
passes a four-case compiled profile covering both optional output pointers.

The policy is now integrated into `Esp32s31ConnectedControl` behind an
explicit opt-in. `ConnectedRunner` supplies one coherent `WifiControlContext`
instead of a growing positional argument list, and re-runs control while
holding a newly arrived pinned network lease. Consequently a station that has
advertised PM=1 must complete an acknowledged PM=0 transaction before the
lease can reach MAC/DMA, including when the frame arrived inside the
executor's `select`. A final PM=0 failure publishes link-down rather than
transmitting data under a split AP/station power-state assumption. The
controller can produce a single-use `StaDozePermit`, but no production caller
consumes it yet.

Actual modem sleep remains deliberately disabled. The next slice must qualify
individual WDEVPWR cause bits and the RF/PHY and platform-clock sleep/restore
transaction, then add a platform sleep owner that revalidates the permit in
the live STA-TSF domain immediately before touching hardware. It must not port
vendor semaphores, notifications, NVS reads or private PM/interface layout.
The non-symmetric two-register
`set_station_tsf_wakeup` bool domain is now a separate passing scenario gate;
it does not broaden the still-missing RF/PHY lifecycle claim.

## 4. Shrink the HIL surface

Continue reducing `radio_hil.rs` toward only:

- board/resource selection and linker-profile hooks;
- task spawning, interrupt entry points and logging;
- credentials, peer addresses and traffic configuration;
- UDP/iperf and explicitly selected qualification workloads;
- synthetic packets, qualification markers and diagnostic snapshots.

Stable diagnostic register meanings move to SVD/PAC with provenance. Raw
reads may remain only when they are explicit comparison evidence and cannot
affect runtime transitions.

PHY observer callbacks and RF/TXDC/authentication register snapshots now live
in `radio_hil/phy_diagnostics.rs`. This is intentionally still HIL code: it
observes production transitions but cannot initiate a channel change, scan,
join, key install or connected epoch. Both stopped-MAC and restart-aware
channel changes now cross `Esp32s31ScanPhy`; HIL only supplies its diagnostic
observer. The main facade fell from 6,850 to 6,501 lines without moving raw
diagnostic addresses into a driver crate. Runtime CRC32 `882f93b8` completed
the cold scan and three controlled reconnect cycles after the source split.

The next responsibility split moved executor-poll residence, MAC IRQ
classification, RX PHY/aggregation evidence and UDP/MAC order correlation
into focused modules of `hil/targets/esp32s31/telemetry`. The runtime keeps
only explicitly placed static instances and observation call sites. Pure
IPv4/UDP parsing and sequence-interval evidence now live in
`radio_hil/connected_traffic.rs`; they own neither sockets nor radio state.
This reduced the facade to 5,471 lines without hiding benchmark dependencies
behind a wildcard parent-module import.

The WPA2/connected transition no longer exposes 23–28 positional arguments.
After extracting key/M4 and peer orchestration, its entry points take coherent
owners: production `Esp32s31ConnectedStaPeer` for peer/BSSID/AID/PHY and
initialized rate-control state, a small HIL session for network/security state,
and `RadioHilConnectedFixture` for concrete board resources. Keep this rule
while shrinking other HIL paths: create a type only for one ownership or domain
invariant, never merely to hide an arbitrary argument list.

The same rule now covers the earlier join boundary. `authenticate_target`
accepts a `RadioHilJoinFixture`, `StaJoinTarget` and the non-QoS sequence
owner; `associate_target` accepts the connected fixture, target and
`StaAssociationSecurity`. Production TX construction likewise uses
`WifiTxResources` plus a phase configuration or `ConnectedTxHandoff`, rather
than exposing slot/policy/power/entropy/timer/key/sequence fields as unrelated
positional arguments.

## 5. Make executable evidence local to each probe

Completed for driver adapters. Their canonical proof now binds the exact raw
inventory symbol (bytes plus relative relocation schema), the linked vendor
root with its local implementation helpers, and the compiled Rust probe with
its local implementation helpers. Calls to another global definition remain
named ABI boundaries; companion code is included only when reached as a local
helper. Whole ELF/archive hashes remain descriptive caller-owned provenance
and no longer enter adapter baselines. Direct JAL and AUIPC/JALR targets are
address-normalized, so unrelated linked layout does not change the proof.

The backend tests require an unrelated linked function mutation to preserve
the closure identity, while mutations of the root, a local callee, or the raw
symbol/relocation identity change it. Keep accepting baseline refreshes only
after comparing every effect row and confirming zero mismatch, incomplete and
orphan probes; verifier-source changes intentionally remain evidence changes.

## 6. Current priority: close the production STA lifecycle

Do not add AP, sniffer, another PHY mode or another HIL traffic scenario before
the connected station can return its resources to an outer owner. Throughput
qualification has already proved enough of the connected data path; the
remaining architectural blocker is lifecycle ownership, not another register
leaf.

The first reconnect seam is now production-owned:

- `ConnectedRunner::run` returns the typed `ConnectedRunnerExit::Disconnected` after it
  publishes link-down instead of hiding link loss as `Ok(())`;
- `ConnectedRunner::into_parts` and `Esp32s31ConnectedServices::into_parts` return the
  network, hardware, RX, TX and control owners to their caller;
- an idle connected ordinary or aggregate transmitter can return its pinned
  descriptor resources, A-MPDU storage, pairwise-key token and sequence state;
  an active transaction rejects that transition without losing ownership;
- `Esp32s31ConnectedRxProtocol::run_until` prioritizes a caller-supplied stop
  edge, discards queued input instead of blocking on the network sink, and
  returns counts after every hot/cold reorder lease and command is released.
  The HIL now signals that edge when the production radio runner exits and
  waits for an explicit protocol-stop acknowledgement before reusing queues;
- `RxRingLive::try_stop` confirms the walker-disable edge before producing an
  `RxRingHalted`, and `Esp32s31ConnectedRx::try_stop` preserves the complete
  static RX resource bundle on both success and failure. The HIL consumes this
  transition after its radio runner exits;
- `Esp32s31ConnectedTx::try_into_teardown_parts` returns descriptor resources,
  A-MPDU storage, PTK token and sequence state as one driver invariant. The
  HIL now keeps `ConnectedRunner` in its parent STA future, clears PTK/GTK through
  the cooperative hardware owner and reconstructs its pre-connected control
  TX owner instead of stranding the connected runner in task storage.
- the connected interrupt transaction is reversible: the platform first
  disables both CPU routes on their binding core, PAC then masks and clears
  the MAC/WDEVPWR banks and returns `MacInterruptSetup`, and the Embassy IRQ
  adapter drains RX, staging-capacity and TX wakes before another epoch can
  activate the same stable ISR storage. The HIL consumes this transition
  before stopping RX DMA.
- `Esp32s31ConnectedControl::shutdown` clears an in-flight or committed RX
  BlockAck bank, all TX BlockAck sessions and every enabled HE TID, discards
  late association-scoped events and returns exact cleanup counts. The HIL
  calls it only after the staged protocol stop acknowledgement, so no later
  ADDBA publication can repopulate a closed control epoch.
- halted RX now owns a second type-state transition through
  `Esp32s31PreparedRx` back to `Esp32s31ConnectedRx`. Descriptor rebuild and
  walker enable return the complete halted/prepared owner on failure. Host
  coverage exercises a rejected enable followed by a successful retry, and
  HIL creates and confirms a second RX DMA epoch using the same static arena;
- the HIL benchmark now has a stop acknowledgement and cannot retain the PAC
  register cell after disconnect. `run_connected_network` returns a coherent
  production `Esp32s31DisconnectedStaEpoch` containing the persistent network runner,
  register-backed hardware, halted RX and A-MPDU storage. `embassy-net` and
  its report task are created only for `Unstarted`; the returned `Running`
  state keeps that stack alive with link-down rather than calling
  `StackResources::init` again;
- connected-epoch construction now distinguishes the board-only
  `RadioHilConnectedEpochResources::Initial` edge from the production
  `Esp32s31ReconnectedStaEpoch`. Only the
  initial variant can promote raw `RadioRegisters` into the cooperative cell
  and initialize A-MPDU/control storage. The reconnect variant accepts only
  the hardware, halted RX, pinned A-MPDU arena and control mailbox returned by
  `Esp32s31DisconnectedStaEpoch`, so a second connected epoch cannot compile a
  repeated `StaticCell::init` path;
- `ConnectedControlResources` now supports sequential endpoint recreation on
  the same static Embassy channel. Its host test closes one publisher/consumer
  scope, opens a second and proves FIFO delivery again. The HIL does not open
  the next scope until the RX protocol stop acknowledgement and connected
  control shutdown have completed;
- pre-connected RX now has one explicit production type-state owner,
  `Esp32s31PreconnectedRx`:
  `Halted → Prepared → Live → Halted`. Authentication returns its halted
  frontier to Association; Association transfers the live frontier into WPA2;
  WPA2 restart/stop and the protected ARP probe preserve the same owner; and
  the initial connected epoch consumes it instead of reconstructing a ring
  from static addresses. Failed prepare/start/stop transitions retain the
  last hardware-valid owner for fail-closed handling. The owner now lives in
  the ESP32-S31 Embassy integration crate; HIL supplies only its static DMA
  buffer recycle closure. Its host test moves the live frontier between
  protocol phases and returns it to halted, and runtime CRC32 `165ac77c`
  repeated the complete scan/Authentication/Association/WPA2 transition three
  times on hardware;
- network frame queues and their pinned TX pool are now initialized by one
  explicit `initialize_sta_network` edge outside Association. Association
  consumes either that `Unstarted` owner or, in a later reconnect composition,
  the existing `Running` owner; it can no longer hide another static network
  allocation inside each protocol attempt;
- the finite Authentication/Association and WPA2 HIL backends now depend only
  on their actual `Mmio`, `RxDma`, `TxHardware` and `CcmpKeyHardware`
  capabilities. They are no longer tied to the cold `RadioRegisters` type and
  can therefore operate on the `CooperativeRadioHardware` returned by a completed
  connected epoch;
- every returning Association/WPA2 error now produces one
  `RadioHilJoinFailure` with observable progress and a `RadioHilJoinRetry`
  owner. That owner contains the exact board fixture, RX type-state frontier,
  network queues/stack state, PMK, nonce and sequence counters; key-install
  rollback and protected-data failure no longer terminate by dropping those
  capabilities inside a nested function;
- stopped production RX can now be split into a peer-specific
  `RxRingHalted` frontier and peer-independent `Esp32s31RxEpochResources`, then
  reassembled with either a halted or live frontier. The reconnected HIL epoch
  carries the same `RadioHilJoinRx` type state used by Association while
  retaining its staging pool, queue sender, reload delay and telemetry owner;
  the next connected epoch consumes both halves instead of rebuilding those
  resources from globals;
- the complete disconnected/reconnected station epoch now lives in
  `station_epoch.rs`. A running scan consumes a named split containing only
  hardware and stopped RX while the network, A-MPDU arena and control mailbox
  remain inaccessible in `Esp32s31RunningScanRetained`. Restoring the scan
  owner produces `Esp32s31DisconnectedStaEpoch`; `prepare_reconnect` then
  consumes that value once and yields `Esp32s31ReconnectedStaEpoch` with a
  pre-connected RX frontier plus its persistent staging resources. HIL keeps
  only the genuinely board-specific initial `StaticCell` promotion. Host
  coverage proves the split/restore/prepare transition preserves every
  owner, and runtime CRC32 `165ac77c` completed three hardware cycles with the
  same descriptor base;
- `Esp32s31PreconnectedRx::service_completed` now owns the common finite
  descriptor walk used by Authentication, Association and WPA2. It is the
  only layer in those paths which turns a completed descriptor into a DMA
  buffer reference or rearms a completed half. Its higher-ranked observer
  lifetime prevents a buffer reference from escaping across recycle, and a
  terminal observation deliberately keeps the descriptor in the live ring
  for the next protocol phase. The HIL management/EAPOL adapters now only
  parse copied frames and select `Continue`/`Stop`. Host coverage synthesizes
  a terminal completion; CRC32 `165ac77c` repeated the real management and
  EAPOL paths three times. Returning the existing future directly avoids an
  extra async state machine and keeps the image at 1,203,712 bytes;
- `Esp32s31RunningScanRx` now keeps that halted ring together with the exact
  staging pool, queue sender, reload delay and telemetry binding throughout a
  running scan. Its `Halted -> Prepared -> Live -> Halted` transition uses the
  same platform-specific 5 us walker-enable settle edge as connected RX. A
  host ownership test proves an exact round trip back to
  `Esp32s31StoppedRx`, and the controlled reconnect HIL executes this
  production owner when it qualifies the second RX DMA epoch;
- `Esp32s31RunningScanTx` now consumes the ordinary descriptor returned by
  connected teardown only after both CPU and peripheral IRQ routes are
  quiesced; its public constructor requires a borrow of the returned
  `MacInterruptSetup` and retains that borrow for the complete TX-owner
  lifetime, so an active IRQ epoch cannot accidentally select the polling
  contract. It shares the cold path's fail-closed Probe Request
  classifier, returns the exact `Esp32s31ControlTx` for Association, and
  disables further active attempts after a safe passive-fallback edge. Host
  tests cover owner return and fallback. HIL published a real same-channel
  Probe Request with TX status zero between connected epochs, then returned
  the descriptor and completed the second Association/WPA2 epoch;
- connected protocol shutdown now returns a typed stopped owner rather than
  only diagnostic counters. The spawned protocol task releases its receiver,
  sink and reorder bindings and returns the exact MPDU/Ethernet scratch
  buffers before the connected runner publishes `RadioHilReconnectReady`;
  that owner also retains the board fixture, target, persistent network,
  cooperative hardware epoch, PMK/nonce and updated sequence counters;
- the remaining synchronous Association hardware leaves now expose narrow
  reusable traits: link RX policy, noise-floor observation, HE20 peer
  programming and beamforming report-rate programming. Both `RadioRegisters`
  and `CooperativeRadioHardware` implement the same contracts, in addition to
  their existing MMIO/RX-DMA/TX/CCMP traits; no finite Association operation
  now intrinsically requires the cold PAC-owner type;
- the HIL now consumes `RadioHilReconnectReady` instead of parking it. It runs
  the same production Association, peer programming, WPA2 handshake/key
  install and connected `ConnectedRunner` on the cooperative hardware, halted RX,
  persistent network stack, A-MPDU arena and control mailbox returned by the
  first epoch. A second connected teardown recreates the same typed frontier
  once more, proving at compile time that no reconnect phase needs another
  PAC singleton or static allocation;
- `open-esp-radio-wifi-sta` now owns the executor- and chip-independent
  outer attempt loop. `StaLifecycleService` consumes one caller-defined owner
  across attempt, retry/backoff, disconnect and stop edges, applies bounded
  exponential retry policy, and returns that exact owner on stop, exhaustion
  or terminal hardware failure. Candidate refresh is explicit: the HIL starts
  its proven same-peer frontier with `Reuse`, so it cannot claim an
  unperformed scan. The ESP32-S31 HIL implements only the concrete attempt and
  Embassy timer adapter; retry policy no longer lives in PASS/FAIL code.
- `StaCandidateScanService` now owns the chip- and executor-independent scan
  transaction: one explicit preparation edge, a finite caller-supplied channel
  plan, and a distinct candidate-selection edge. Every channel failure, stop,
  empty plan and no-candidate result returns the exact caller owner with
  bounded progress. `Esp32s31StaScanBackend` now owns the production chip
  transaction order shared by both hardware modes: channel switch, RX start,
  optional active probe with passive fallback, bounded drain ticks, mandatory
  RX stop and next-ring preparation. A drain failure still attempts the stop;
  a stop failure takes precedence because DMA ownership is then uncertain. An
  active-probe failure may fall back to passive scan only when `ControlTxError`
  proves that the descriptor owner is quiescent; a busy, unclassified or
  reset-required TX owner closes RX and returns a fatal scan error. Host tests
  cover all three cleanup edges. The shared production `Esp32s31ScanPort`
  carries `ColdRadioRegisters` by value together with
  `Esp32s31ScanPhy`, `Esp32s31ColdScanTx`, DMA and observation ownership
  through all 13 cold channel transactions and candidate selection.
  `Esp32s31ScanPhy` now owns persistent PHY state, platform control, target
  delay and observer outside HIL. `Esp32s31ColdScanTx` owns the exact
  polling-only control descriptor, TSF/interrupt preparation, passive-fallback
  classification and terminal telemetry; HIL no longer contains a Probe
  Request transmission helper or raw queue cleanup. The
  production `Esp32s31ScanRx` now carries the descriptor authority through
  `Prepared -> Live -> Halted`, promptly recycles each completed prefix during
  dwell and hands that exact `RxRingHalted` to Authentication. The former
  `RadioHilJoinRx::Initial` reconstruction and two raw-address HIL recovery
  loops have been removed. The returned PAC owner alone crosses the one-way
  `into_running` transition. The board fixture now also retains the unique
  `PhyColdState` after Authentication and throughout both connected epochs;
  it is no longer dropped at the first Association boundary. This concrete
  cold port has board evidence. The former HIL `Esp32s31StaScanPort`
  implementation and `RadioHilColdScanOwner` are now deleted. Runtime CRC32
  `d4b41d11` completed the initial scan with 13 successful Probe Requests,
  then three controlled reconnect cycles; see the
  [shared scan-port qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-cold-scan-port.md).
  The controlled reconnect fixture also
  assembles the persistent PHY, production running RX/TX sub-owners and its
  scan table into a concrete running `Esp32s31StaScanPort`. The same backend
  completed the full 6, 1--5, 7--13 channel plan, selected the target network,
  returned every RX/TX owner and transferred the selected `ScanRecord` into a
  fresh Open Authentication transaction on `CooperativeRadioHardware`. The
  resulting target then completed the second Association/WPA2 epoch. This
  proves the running transaction and candidate transfer. The concrete
  `Esp32s31ScanPort` which binds PHY retune, cooperative hardware,
  stopped RX, polling control TX, Embassy dwell timing, scan storage and SSID
  selection now lives in the ESP32-S31 Embassy integration crate. HIL supplies
  only its returned epoch owners, fixed storage, station policy and diagnostic
  frame observer; it no longer implements `Esp32s31StaScanPort` for running
  scan.
  Runtime CRC32 `7a076726` then completed three sequential controlled
  running-scan/reconnect cycles on ESP32-S31 with the same descriptor base,
  empty returned RX queues, 13/13 Probe TX in each generation and a fresh
  connected task topology after every WPA2 handshake.
  Authentication and Association now cross the same boundary. The concrete
  `Esp32s31StaJoinPort` binds the retained RX frontier/DMA storage, management
  extraction, peer RX-filter setup, control TX, PHY preference and calibrated
  HE power capabilities. HIL supplies the candidate and diagnostics only; its
  former `RadioHilStaJoinBackend` and auth/association TX helpers are removed.
  Runtime CRC32 `080db958` subsequently completed three controlled reconnect
  cycles through this port; see the
  [join-port qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-sta-join-port.md).
  WPA2 now follows the same production boundary. `Esp32s31Wpa2HandshakePort`
  owns retained-RX EAPOL extraction/restart plus M2, while
  `Esp32s31Wpa2KeyPort` owns atomic PTK/GTK installation, M4 and rollback. HIL
  no longer contains either backend or any EAPOL/key-slot helper. Its former
  direct protected-ARP frame/RX loop was deleted instead of promoted because
  protected traffic already belongs to the production connected runner.
  Runtime CRC32 `9e080a3b` completed three cycles without that artificial
  gate; see the
  [WPA2-port qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-wpa2-port.md).
  Association-time peer programming now follows the same boundary.
  `Esp32s31StaPeerPort` installs scan-time HT/WMM/HE policy, consumes an opaque
  prepared token plus the successful Association Response, programs HE
  peer/AID/BSR and rate-control hardware, then returns a production
  `Esp32s31ConnectedStaPeer`. Both initial and reconnect HIL paths consume this
  owner; the duplicate policy/programming blocks and private HIL connected-link
  type are gone. Runtime CRC32 `bf8e8ead` completed three cycles with a stable
  descriptor base and empty returned queues; see the
  [peer-port qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-sta-peer-port.md).
  `RadioHilStaLifecycleOwner::RunningScan` is now a distinct outer owner:
  generation 1 entered it only with `refresh_candidate=1`, and the successful
  transaction produced the separate `Reconnect` owner.

This is now a bounded controlled rescan and re-authentication implementation
with board evidence.
`ConnectedRunner::run_until` observes an outer stop only at a transaction boundary,
waits for an active TX to release hardware, publishes link-down and returns the
distinct `Stopped` outcome. HIL protocol v7 advertises this capability and
`cargo hil station reconnect` requests it without calling the stop a beacon
loss. The 2026-08-04 ESP32-S31 run completed the first teardown, a second
Association, WPA2 M1--M4 and entry into the second connected epoch; see the
[qualification report](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-station-reconnect.md).

Controlled reconnect deliberately remains distinct from automatic peer-loss
recovery. The separate `cargo hil station ap-loss` cell now removes the local
HE20 AP, observes protocol-v6 `BeaconLoss`, returns the complete connected
epoch, performs a multi-channel rescan and feeds the restored candidate
through fresh Authentication, Association and WPA2 into generation one. See
the [AP-loss qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-station-ap-loss.md).
The production `Esp32s31StaAttempt` and application-facing
`Esp32s31Station` facade now own the shared pre-connected transaction and
outer lifecycle. The prolonged-absence cell closes the bounded no-peer policy:
protocol v7 observed `BeaconLoss`, three exact `NoCandidate` attempts and
`RetryExhausted` in 17,658 ms. Every 13-channel scan returned the same DMA
owner with an empty queue and zero Probe TX failures. Lifecycle publication
now waits until the exact typed edge has been serialized, rather than merely
admitted to the UART queue. See the
[AP-absence qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-station-ap-absence.md).
The next slices, in order, are:

1. add the complementary deterministic RX failure cell without conflating a
   dropped/corrupt frame with a reset-required DMA owner. RX A-MPDU
   containment is now closed without inventing a common hardware bit. The direct HT-SIG
   Aggregation bit is qualified on an actual HT20/MCS7/SGI downlink:
   78,127 benchmark observations were A-MPDU with zero unavailable
   provenance; see the
   [HT RX aggregation record](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-ht-rx-aggregation-metadata.md). The prior
   tentative inversion of `cur_single_mpdu` was rejected: Espressif defines it
   as IEEE S-MPDU status, and real HE20 data, ARP and Beacon management frames
   all carried a clear value. Its exact propagation is now board-qualified,
   while HE PPDU format now supplies `ProtocolValidated(true)` containment
   rather than pretending to be another hardware field; see the
   [S-MPDU record](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-rx-s-mpdu-metadata.md) and
   [HE containment record](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-he-rx-ampdu-containment.md);
2. split the remaining HIL facade by fixture responsibility (station
   qualification, connected traffic, diagnostics and board bootstrap) while
   keeping only scenario policy, task placement, static storage and reporting.

The connected-TX fault frontier is now qualified. HIL protocol v8 arms one
fault only after the production backend has published a real network TX
descriptor. Its contradictory completion/timeout edge crosses the ordinary
or aggregate `require_reset` path; typed evidence is published only after the
runner, executor tasks and RX DMA have returned while the TX owner remains
quarantined. The host then cold-resets the same image and proves a fresh
network-ready epoch. See the
[TX fault qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-station-tx-fault.md). This
does not close the separate in-place platform reset or RX fault gaps.

The RX failure classes are no longer conceptually merged. Host coverage now
feeds the production service an over-capacity completed unit, observes one
typed discard, and then proves that the immediately following valid descriptor
is staged while the same ring remains live. A real RX HIL fault cell must
inject or receive such a completed descriptor before production staging; a
decorator that merely returns an error after `service_rx()` would test only
the runner and is not acceptable evidence. Errors after ownership becomes
ambiguous (reload failure, corrupt ring, source/descriptor disagreement) stay
reset-required candidates and need their own typed frontier rather than the
drop-and-continue result.

The first capability-driven HMAC/LMAC contract is now source owned.
`open-esp-radio-wifi-softmac` represents granular operation ownership and resource
limits without chip conditionals, while
`ESP32S31_MAC_SERVICE_CAPABILITIES` derives BA and aggregate limits from the
owners that enforce them. `Esp32s31ConnectedStaPort` publishes that profile.
Ordinary TX also distinguishes the raw completion of one S31 hardware attempt
from `MacTxStatus` for the complete retried exchange; the latter retains total
attempts, final typed rate, ACK meaning and ACK SNR. Aggregate TX now publishes
one `MacAmpduTxStatus` only after BlockAck retry policy and any detached
ordinary retry have both terminated; it preserves their separate rates and
publication counts. Do not add a broad
`hardware_retry`, `hardware_crypto` or `hardware_ampdu` flag: each of those is
currently split across hardware and software.

The executor stop deadline is now production API:
`stop_esp32s31_connected_task_group` returns all task-owned scratch or a
distinct reset-required outcome under one group deadline. The HIL task group
contains only its benchmark/protocol signals. A complete platform radio-reset
implementation from that terminal frontier and image-size classification
remain deferred.

Network stack/report lifetime, per-epoch benchmark lifetime and connected
static-resource lifetime are separated correctly. The outer lifecycle gap is
closed for running scan: `refresh_candidate` selects a distinct owner, and the
scan result crosses fresh Authentication before Association. Peer programming
and rate-control activation now live behind `Esp32s31StaPeerPort`.
`Esp32s31ConnectedStaPort` now owns rate selection, RX protocol,
ordinary/aggregate TX, control/BlockAck and final backend assembly; runtime
CRC32 `c7a6b50b` completed three controlled reconnect cycles through that
port. RX live promotion and ordered control/RX/TX/key teardown now follow the
same boundary through `Esp32s31ConnectedStaTeardownPort`; runtime CRC32
`c51449b4` completed another three cycles. `Esp32s31MacInterruptEpoch` and
`EspHalMacInterruptRoute` now own stable ISR storage, CPU route activation,
finite hard-handler service, route quiescence and stale wake drain; runtime
CRC32 `02cbd34c` completed three more cycles without increasing the encoded
image. The bounded executor acknowledgement is now shared production behavior
and has both host and repeated-board evidence. The remaining recovery gap is
the platform action after `ResetRequired`, not another hardware-driver
transition.
Cold scan still precedes the outer station service, while running scan is now a
real service phase. `Authenticate`, `Join`, `RunningScan` and `Reconnect`
deliberately retain different owner types instead of sharing a mutable
vendor-style context.

## Standalone production composition

`examples/esp32s31-station` is now a real non-HIL consumer of the public
driver graph. On ESP32-S31 hardware it completed cold PHY/MAC, active scan,
Open Authentication, HE20 Association, WPA2 M1--M4, interrupt-epoch
activation and DHCP. Its application-owned UDP echo service then returned
200/200 sequential packets, including payloads above one KiB. The dependency
tree contains no HIL protocol, runner, benchmark or telemetry crate.

This target exposed a real board-memory failure: a 64-slot hot RX pool with
32/32 network queues plus the formerly unconditional 64-slot cold reorder
backing raised `.bss` to roughly 405 KiB and made pre-connected WPA2 progress
unreliable. The internal-SRAM baseline now uses 16 stage slots, 8/8 network
queues and an 8-slot typed reorder backing. Before the finite application
reconnect state was added that reduced `.bss` to roughly 176 KiB; the current
complete same-candidate lifecycle is roughly 182 KiB. Large throughput
profiles require explicit PSRAM/internal-SRAM placement and a linked-image
budget.

Connected teardown and candidate-refresh reconnect are now part of the
standalone production application. `run_connected` stops at a safe runner boundary,
quiesces and drains the interrupt epoch, obtains the stopped staged-RX owner,
then uses `Esp32s31ConnectedStaTeardownPort` to stop DMA, return TX resources
and sequences and clear both CCMP slots. Network stack/socket tasks remain
alive while only the radio endpoint crosses link-down/link-up. A temporary
application-controller trigger (removed after the run) proved a complete
13-channel running scan in 5,389 ms, 13 successful Probe transmissions with
zero TX failures, and a without-reset second
Authentication/Association/WPA2/ConnectedEntry sequence. UDP echo after the
second epoch returned 100/100 packets in 847 ms.

The cold candidate loop now also preserves its descriptor frontier across a
complete `NoCandidate` result. `Esp32s31ScanRx::prepare_initial_or_retry`
accepts only the original prepared ring or the halted ring returned by the
last channel; a live frontier still fails closed. With a deliberately absent
SSID, hardware completed three consecutive 13-channel scans and retries using
the same owner instead of the previous second-pass `Prepared/Halted` panic.

That ownership path raises the final linked `.bss` baseline to roughly 182
KiB. `prepare_with_storage` additionally rejects a configured RX Block Ack
window larger than the board-selected `RxReorderFrameStorage` slot count, so
the compact 8-slot profile is no longer held together by convention.

The application candidate-refresh composition gap is closed. Its typed owner
now remains disconnected until `Esp32s31ScanPort` returns hardware, RX,
ordinary TX, scan scratch and a fresh candidate; only then may
`prepare_reconnect` split the stopped RX resources for the next join. The smoke
test rediscovered the same AP, so cross-BSSID and changed-channel recovery are
not yet separately qualified. The next bounded qualification debt remains the
deterministic injected RX-fault HIL cell.

## Completion gate

For each extracted slice:

1. add host tests for finite state, exact deadlines and ownership rules;
2. compile the production owner in the vendor semantic probe where applicable;
3. change HIL to consume the API and delete its duplicate logic;
4. run formatting, host tests, embedded debug/release checks and focused
   evidence baselines;
5. repeat the hardware cells named by
   [the feature ledger](ESP32S31_WIFI_FEATURE_STATUS.md) when DMA lifetime,
   interrupt ordering, target placement or protocol behavior changes.
