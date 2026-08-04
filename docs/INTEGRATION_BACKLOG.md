# Integration backlog

Verified against `hil/esp32s31/runtime/src/radio_hil.rs` on 2026-08-04.

The HIL workspace owns board clocks and boot, PSRAM/flash placement, the
executor, concrete `embassy-net` scenarios, credentials, traffic generation
and reporting. Reusable radio behavior belongs in a driver, protocol or
integration crate.

The production path now has reusable owners for generated PAC register leaves,
MAC IRQ dispatch, RX/DMA frontier and recycle, ordinary connected TX,
pre-connected management/EAPOL TX,
connected BlockAck control, the single `WifiRunner` event loop,
Authentication/Association through `StaJoinRunner`, and WPA2 Message 1/3
response handling through `Wpa2HandshakeRunner`. `Wpa2KeyInstallRunner` owns
request validation, PTK/GTK publication ordering, Message 4 completion and
rollback. Referenced HT/HE A-MPDU, partial BlockAck retry, one-MPDU HT retry
handoff and beacon-loss timing are now owned by that same runner. The HIL
consumes those owners; it no longer contains parallel
Authentication, Association, WPA2 response-deadline or key-install state
machines. Completed transfer history is archived in the
[2026-07-31 integration report](archive/integration/2026-07-31-esp32s31-rust-integration-audit.md).

## 1. Completed: concrete WPA2 TX backend

`Wpa2HandshakeRunner` now accepts the still-live RX ring through a finite
backend, enforces the exact absolute Message 1 and Message 3 deadlines,
services RX before simultaneous timeout, transmits Message 2 only in response
to a peer Message 1, stops RX and returns a typed `Wpa2PendingKeyInstall`.
The HIL has deleted its former Message 3 receive/deadline loop.

`Wpa2KeyInstallRunner` now consumes `Wpa2PendingKeyInstall`, validates the
station PTK/GTK request, publishes both keys atomically through its backend,
builds and transmits the exact Message 4, rolls both keys back on every later
failure, and returns typed installed-key ownership for `WifiRunner`. Its host
tests include the complete successful M1/M2/M3/key/M4 transition, atomic
install failure and Message 4 rollback.

The HIL implementation now calls the shared management/EAPOL owner from
section 2. Credentials, diagnostics and the protected ARP end-to-end assertion
remain HIL policy. No RTOS event queue or vendor supplicant context is retained.

## 2. Completed: management/EAPOL TX transaction owner

`ordinary_tx.rs` is now the single owner of the pinned ordinary descriptor,
EDCA/retry state, calibrated power, entropy and per-publication deadline.
`control_tx.rs` provides typed Probe, Authentication, Association, unprotected
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

The production HIL constructs this owner and runs it through the same
`WifiRunner`; both debug and optimized RISC-V images link successfully. Host
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
explicit opt-in. `WifiRunner` supplies one coherent `WifiControlContext`
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

The WPA2/connected transition no longer exposes 23–28 positional arguments.
After extracting key/M4 orchestration, its entry points take three or four
coherent owners: `StaConnectedLink` for peer/BSSID/AID/PHY facts,
`StaConnectedSession` for network/rate/sequence state, and
`RadioHilConnectedFixture` for the concrete board-owned resources. Keep this
rule while shrinking other HIL paths: create a type only for one ownership or
domain invariant, never merely to hide an arbitrary argument list.

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

- `WifiRunner::run` returns the typed `WifiRunnerExit::Disconnected` after it
  publishes link-down instead of hiding link loss as `Ok(())`;
- `WifiRunner::into_parts` and `Esp32s31WifiBackend::into_parts` return the
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
  HIL now keeps `WifiRunner` in its parent STA future, clears PTK/GTK through
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
  can therefore operate on the `CooperativeTxHardware` returned by a completed
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
  and `CooperativeTxHardware` implement the same contracts, in addition to
  their existing MMIO/RX-DMA/TX/CCMP traits; no finite Association operation
  now intrinsically requires the cold PAC-owner type;
- the HIL now consumes `RadioHilReconnectReady` instead of parking it. It runs
  the same production Association, peer programming, WPA2 handshake/key
  install and connected `WifiRunner` on the cooperative hardware, halted RX,
  persistent network stack, A-MPDU arena and control mailbox returned by the
  first epoch. A second connected teardown recreates the same typed frontier
  once more, proving at compile time that no reconnect phase needs another
  PAC singleton or static allocation;
- `open-esp-radio-wifi-lifecycle` now owns the executor- and chip-independent
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
  cover all three cleanup edges. The HIL cold port carries
  `ColdRadioRegisters` by value together with the production
  `Esp32s31ScanPhy`, `Esp32s31ColdScanTx`, DMA and observation ownership
  through all 13 channel transactions and candidate selection.
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
  cold port has board evidence. The controlled reconnect fixture now also
  assembles the persistent PHY, production running RX/TX sub-owners and its
  scan table into a concrete running `Esp32s31StaScanPort`. The same backend
  completed the full 6, 1--5, 7--13 channel plan, selected the target network,
  returned every RX/TX owner and transferred the selected `ScanRecord` into a
  fresh Open Authentication transaction on `CooperativeTxHardware`. The
  resulting target then completed the second Association/WPA2 epoch. This
  proves the running transaction and candidate transfer. The concrete
  `Esp32s31RunningScanPort` which binds PHY retune, cooperative hardware,
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
  [join-port qualification](hil/2026-08-04-esp32s31-sta-join-port.md).
  `RadioHilStaLifecycleOwner::RunningScan` is now a distinct outer owner:
  generation 1 entered it only with `refresh_candidate=1`, and the successful
  transaction produced the separate `Reconnect` owner.

This is now a bounded controlled rescan and re-authentication implementation
with board evidence.
`WifiRunner::run_until` observes an outer stop only at a transaction boundary,
waits for an active TX to release hardware, publishes link-down and returns the
distinct `Stopped` outcome. HIL protocol v4 advertises this capability and
`cargo hil station reconnect` requests it without calling the stop a beacon
loss. The 2026-08-04 ESP32-S31 run completed the first teardown, a second
Association, WPA2 M1--M4 and entry into the second connected epoch; see the
[qualification report](hil/2026-08-04-esp32s31-station-reconnect.md).

This evidence deliberately has a narrower meaning than automatic recovery
from an unavailable AP. The controlled cycle now performs and observes a real
multi-channel rescan and feeds the selected candidate through fresh Open
Authentication. The trigger is still a host-requested healthy runner stop, and
the selected AP remains available throughout. It therefore does not prove AP
disappearance, a beacon-loss-triggered rescan or retry/backoff recovery.
The next slices, in order, are:

1. extract the remaining WPA2/key-install/connected-session composition from
   HIL. The reusable Embassy layer now owns the disconnected/running-scan/
   reconnected resource transition and the complete Authentication/
   Association port, while the board still sequences peer programming, WPA2
   and connected entry;
2. route real beacon loss through candidate selection and Authentication, and
   preserve the complete retry owner across each bounded failure/backoff edge;
3. qualify real AP loss/recovery and one injected TX/RX failure before
   resuming feature expansion.

Network stack/report lifetime, per-epoch benchmark lifetime and connected
static-resource lifetime are separated correctly. The outer lifecycle gap is
closed for running scan: `refresh_candidate` selects a distinct owner, and the
scan result crosses fresh Authentication before Association. The remaining
architectural gap is now the security/session composition: WPA2 RX/TX/key
installation, peer programming and connected-entry sequencing still live in
`radio_hil.rs` and must move behind narrower ESP32-S31 Embassy ports.
Cold scan still precedes the outer station service, while running scan is now a
real service phase. `Authenticate`, `Join`, `RunningScan` and `Reconnect`
deliberately retain different owner types instead of sharing a mutable
vendor-style context.

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
