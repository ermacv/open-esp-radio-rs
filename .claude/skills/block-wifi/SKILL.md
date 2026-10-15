---
name: block-wifi
description: Use when a task changes or investigates the IEEE 802.11 (Wi-Fi) block of this repository — 802.11 protocols, the lower-MAC port, upper MAC, station or access-point services and roles, RSN, the ESP32-S31 Wi-Fi driver, runtime or composition, or Wi-Fi HIL scenarios. Maps the block's packages across layers, its shared contracts, traps and checks.
---

# Wi-Fi block

Layers and edges: [layer dependencies](../../../docs/architecture.md#layer-dependencies),
[sans-IO and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[radio ports](../../../docs/architecture.md#radio-ports),
[ownership and refusals](../../../docs/architecture.md#ownership-and-refusals).

## Packages (path → role)

- Protocols, portable sans-IO: `crates/protocols/ieee80211/` — `mac` (frames, scan, fragmentation), `lower-mac` (the port), `upper-mac`, `softmac`, `sta`, `ap`, `datapath`, `security/rsn`, `trace`.
- Family policy: `crates/protocols/espressif/ieee80211/policy` (Espressif TX policy data).
- Services, portable: `crates/services/ieee80211/rsn`; the station service and lower-MAC owner glue live in `crates/runtime/ieee80211`.
- ESP32-S31: driver `crates/hardware/esp32s31/driver/ieee80211/{mac,dma}`, roles `crates/roles/esp32s31/ieee80211/{sta,ap}`, runtime `crates/runtime/esp32s31/ieee80211`, composition `crates/composition/esp32s31/embassy/ieee80211`, esp-hal adapter `crates/adapters/esp-hal/esp32s31/ieee80211`.
- Facade: `crates/oer` features `wifi`, `esp32s31-wifi`. Applications: `examples/esp32s31/{station,access-point,monitor}`. HIL: `hil/scenarios/ieee80211/`, workloads in `hil/host/family/ieee80211` (fixtures `-fixture`, air analysis `-evidence`).

## Port

The port is on the owner shape: `LowerMacBackend` and its parts in `crates/protocols/ieee80211/lower-mac/src/owner.rs`, its lifecycle (`run_port`, `Owner`, `PortBackend`) in the shared `oer-radio-port` (`crates/radio/port`). The S31 backend is `Esp32s31Backend` over the static runtime `Esp32s31LowerMac` (`crates/runtime/esp32s31/ieee80211/src/lower_mac.rs`); it has no A-MPDU, beacon-timing or monitor operation yet, and the product datapath still drives the hardware directly (`DatapathRunner`) until #709 step 2 moves it onto the port. The portable port services (upper-mac service, ap, sta-ap, the sta port station) were deleted in #709 step 1; git history keeps them for the rebuild on the portable stack. The station is the portable sans-IO `oer_ieee80211_sta::machine` beside the channel arbiter (`oer-ieee80211-arbiter`); the S31 roles' orchestration (station, AP, monitor, ESP-NOW, STA+AP) was removed and returns on the port in #563's step 2 (tag `s31-station-old-path` keeps the old path).

## Shared contracts (change only with their owners)

`crates/radio/port` (`RadioPort`, `PortResult`, `Poisoned`, `ClockInfo`, `ClockRelation`, `RelationSample`, `Generation`), `crates/time` (`Instant`, `RadioInstant`), `crates/radio/coex`, the shared radio system `crates/runtime/esp32s31/radio` and `crates/composition/esp32s31/embassy/radio`, the HAL split `crates/hardware/esp32s31/hal/src/root.rs`, PAC and `registers/`.

## Traps

- Wi-Fi uses the common lifecycle directly: construct `Installed`, return
  `PortBody::Output` through `Ended`, and inspect `PortEnd` including its
  quarantine outcome. The model's branded work stays in `Held`; its one
  `DriverLoan` is consumed only on retirement or published poison.
  `EventEnd::Drained` requires every expected terminal identity to be back;
  a missing ticket/command permission is a contract violation. Quarantined
  ends immediately through fail-stop, without a second disable/drain. The
  model retains poisoned owners even after the quarantine receipt drops.

- S31 RX positions retain `RingIndex<COUNT>` from address validation through
  descriptor traversal and append. Extract `get()` only at an array or
  diagnostic boundary. A position from another ring capacity cannot enter
  the walk, and an empty ring has no valid position. RX arena retention and
  detach authority remain separate from this geometry proof.

- S31 ordinary queue places belong to `CorePools`: the branded authority
  constructs one core, and each `FreeQueue` moves through submit and its
  terminal. A failed gate opening withdraws every unpublished owner and
  retains the published prefix in `GateOpenFailed`, which only drains.
  Host model DMA bus addresses can alias between slots; ownership tests
  must compare the pinned Rust owners, not just their model bus addresses.

- S31 `LowerMacCore::new` returns `DisabledCore`. Lifecycle and gate
  transitions consume their owners: only `EnabledCore` submits, only
  `DisabledCore` retunes/dismantles, and `DisablingCore::finish` emits the
  single disabled terminal. Keep the gate type across disable/retune; a
  closed gate must not silently reopen on enable. `Configuring` owns the
  FIFO barrier until its role's publications end, without delaying for the
  other interface's frames. `GateOpenFailed` retains the published prefix.
- `CorePools<N>` issues the core authority and exactly `N` TX tickets.
  Both open and protected attempts consume a ticket. `KeyHandle` is affine;
  removal consumes it and `KeyUse` retains each already admitted use until
  its MPDU or A-MPDU terminal. Publication takes the selector directly from
  that right. Never clear or reprogram the slot while a right exists;
  `KeyRemoved` follows the last TX terminal without polling. Poisoned keeps
  DMA, queue and key rights in sealed quarantine. `LowerMacCoreParts` keeps
  the key ledger across a disabled restart; extracting only the transmitter
  would lose the outstanding rights and is not exposed.
  Station GTK installation returns `ReceiveKeyHandle`, excluded from TX
  admission and PN allocation. The private `KeyUseCount<N>` only changes by
  consuming a ticket or its issued right; its width is checked in the same
  `CorePools<N>` const evaluation, including the full 256-ticket bound.

- A stopped station service restarts only by consuming `StoppedService` with
  its matching disabled port owner. Reuse its returned banks: reminting loses
  epoch, lease and data-order frontiers. Retirement returns idle channel
  places separately from the station bank; the stopped arbiter collects all
  returns and issues the complete `StationPlaces` required by the infallible
  station restart. Startup reclaims old keys and receive
  settings before dequeuing application commands. After a terminal violates
  the contract, `TerminalCustody` retains that return and all later terminals
  without feeding machines; a full sink returns the next owner to poisoned
  port teardown. Station TX bodies are `Infallible`, including in the model
  composition.

- Keep each package's `clippy.toml` as a regular source file. HIL source
  snapshots retain lint configuration and reject build-input symlinks; a
  symlink to a shared configuration prevents image capture before compilation.

- A backend declares its complete `PortBackend::Pools` layout and consumes `Mint::into_pools(self)` once. An arbiter-backed installation uses `ArbiterPool`, yielding `GateOpen`, the backend's `GateIssuer` and the unique `ArbiterAuthority`. An independent gate uses `GatePool` without arbiter construction authority. A gate returned from a cycle must never mint another arbiter or lease places. Each tuple component has a sealed pool path; retain it through gate/issuer, the phase's `GatePath`, station `StaTokens::GatePath`, leases, replies and pending retunes. Do not erase ticket, key, buffer or connection-source paths in wrappers. The source's path also stays in station/data epochs and contexts; two sources in one installation cannot produce interchangeable epochs.

- Time follows [clocks, stamps and alarms](../../../docs/architecture.md#clocks-stamps-and-alarms).

- A phase loan alone does not identify a rate's installation: independent
  enabled owners can have the same inferred loan lifetime. Keep all three
  parameters of `PhaseRate<'p, 'id, Caps>` through NoAck/A-MPDU rates,
  retry plans and prepared frames. The invariant install brand prevents a
  2.4 GHz proof from bypassing a second same-Caps installation's 5 GHz check;
  the phase loan prevents reuse after disable and retune.

- Admission closes TX context, ticket and MPDU in `InFlight`; queues move
  that whole owner through the terminal. `ExecutionPayload` projections must
  lend only execution storage, never a public `&mut` to an MPDU owner or an
  extraction/replacement operation. Aggregate subframes own their bodies;
  cancellation iterates them without creating an admitted aggregate. Check
  custody before teardown when injecting a publication fault: counting all
  destructors only after `run_port` ends cannot detect an early owner drop.
  Preflight an aggregate's whole fresh-PN range before changing any subframe;
  retries consume no new PN. A poisoned submission still owns the transferred
  context, ticket and MPDUs until teardown, even when its call reports poison.
  Single/Aggregate cardinality does not determine the response: a single BAR
  solicits Block Ack, while Group and IndividualNoAck complete without an ACK.
  Preserve the checked frame's response policy through Sent retries and barriers.

- Task-group shutdown observations never transfer resources into the waiting
  future. Each controller keeps its returned owner until consuming `try_finish`;
  an auxiliary group retains either child's early result across cancellation
  and deadline expiry. Readiness is repeatable and does not release storage.
  Before publishing Idle, consuming retirement clears the old stop latch:
  a repeated stop after completion must never stop the next task epoch.

- CCMP replay preparation returns an exclusive borrow of the selected
  frontier. Commit the token itself before publication, or drop it to cancel.
  Never replace this with a transferable PN/revision pair: resetting a key
  with the same wire ID also resets its revision, letting an old candidate
  corrupt the new frontier. A new material epoch starts at KeyRSC; keeping
  identical material means keeping the original replay owner.

- AP peer storage is lent as `AccessPointPeers`, which moves into the service
  and back into its retirement bank without an empty placeholder. Its drop
  clears peer secrets and the PMKSA cache; the identity generation frontier
  remains in the caller-placed table and survives a restart.
- A beacon publication retains `ApBeaconTxContext`, including the exact DTIM
  group prefix from preparation. Only confirmed publication releases that
  prefix. Later queue arrivals and a later TIM phase cannot enlarge it;
  failed, withdrawn, indeterminate and unsent beacons leave it buffered.
- AP EAPOL publication retains `ApEapolTxContext` from frame construction to
  terminal delivery. RX can advance to M3 before the M1 TX terminal, or finish
  M4 before the M3 terminal. Compare the retained association/message/replay
  identity before arming a retry: deriving the message from the current RSN
  phase lets a late terminal arm another exchange. A different peer's table
  mutation must not invalidate the original peer's association identity.

- Pending key actions and installed GTK records retain `CcmpKey`, whose
  Debug is redacted and whose destructor zeroizes its bytes. Compare through
  borrowed material; a raw `[u8; 16]` in a derived Debug token exposes GTK.
  This owner discipline does not prove erasure of compiler temporaries.

- Authentication's `NeedCredit` owns only the retry runtime. An active
  `StaAuthenticationExchange` is created after ticket/buffer reservation and
  is consumed by response or timeout; only Retry returns the runtime. Do not
  call an active-exchange RX parser while waiting for credit or restore
  separate `active`/`terminal` flags beside the machine's owned stage.

- Association polling and RX consume their runtime owner. Only a continuing
  result returns it; terminal results do not leave a second usable handle.
  A comeback owns its future epoch and retains RX accounting across the wait;
  it restarts the request ordinal, but not the station's sequence counter.

- A home-channel background dwell starts after Configure completes. Check
  overflow both before widening and on its delayed terminal; a failure after
  widening must restore member rules before returning the scan request.
  SA Query's final timeout is checked before it starts; later retries outside
  that window are absent, while the final timeout remains a wake source.

- A terminal port failure cannot drain ordinary `Station::poll_action` and
  discard non-notice actions: doing so consumes Configure/key owners and waits
  for replies the failed port cannot produce. Finish Station/Data into their
  terminal banks after disable's resource returns. `StationService::run` consumes
  the service; `FailedInstallation` is Disabled and can uninstall normally,
  but cannot enable. Recovery needs new service and installation authority.

- `Station::new_open` and `new_personal` separate construction. The backend
  token type carries `GROUP_SLOTS`; resources must match it, and personal mode
  requires a nonzero capacity at compile time. Personal credentials carry SAE
  password and PSK PMK. Join follows shared AKM selection and uses owned SAE
  commit/confirm or the matching PMKSA cache entry; never silently force PSK.

- Connected M3 authenticates both exact and larger replay counters through
  `on_message3`. PTK stays installed; only GTK/IGTK work is returned. In the
  initial supplicant, `RsnStaRetryInstall` cannot name a pairwise install.
  M4 keeps its M3's MAC protection through credit waits and key completion.
  The pinned `wpa.c` M4 builder handles EAPOL flags/MIC, not the 802.11
  Protected bit. Do not confuse EAPOL Secure with MAC protection. Group M1
  requires protected, integrity-verified and replay-admitted RX.
  Deferred M3 and GM1 share the EAPOL-Key replay frontier: process the
  smaller counter first when both cells are occupied. Prioritizing M3 by
  kind advances that frontier past a valid older GM1 and drops it.

- GTK records survive M4/GM2 delivery. Only eviction or Leave removes them.
  Evict the oldest non-current installation when all physical slots are
  occupied; use that same algorithm at capacity one. Selecting retained
  material preserves both its installation age and every replay lane.

- Management disconnect admission is shared by all portable join/link stages:
  `parse_sta_disconnect` takes OwnAddress/Bssid and accepts our unicast or
  broadcast from that BSS. It never parses Protected ciphertext as a reason;
  PMF requires its separate authenticated-management owner, not RX metadata.

- `Bssid` proves both individual and nonzero. Vacant scan cells are `None`,
  and `ScanRecord::new` takes a real BSSID/channel/RSSI observation; its
  constructor is not an empty-record factory. A parked connected RX arena
  likewise has no dispatcher until its association identity is available.

- Station commands enter through `Station::command`, consuming `StaRequest`.
  Each command kind reserves its own terminal cell; completion returns that
  request and refusal returns it synchronously. Never replace those cells
  with one optional notice: a refused background absence and a Disconnect can
  both owe results before the next action poll. Keep the exact AbsenceRefusal.

- Every port method but `next_event` is synchronous (`port.rs`, the trait's documentation).
- A port has one event stream in occurrence order (#266 D8): the S31 runtime numbers every event under the core's lock and merges its four queues by that number (`runtime/esp32s31/ieee80211/src/lower_mac.rs`, `Queues::take`). An extension event names its kind and interface (`LowerMacEvent::Extension { kind, vif }`), and the router routes it by that interface, never by role.
- Interrupt model ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals), #643): the MAC and power interrupts only read and clear their status and post the events, through `IrqConsumer::publish` (`hardware/esp32s31/driver/ieee80211/mac/src/irq/consumer.rs`); the port's owner task runs `LowerMacCore::service`. No Wi-Fi decision has a software deadline below 1 ms (ACK and Block Ack turnaround are hardware). The port backend `Esp32s31LowerMac` still runs `service` inside `on_interrupt` under the core's blocking lock; #563 removes it, so never add interrupt-side logic there.
- Pinning an executor scope does not extend external borrows inside its
  `Context`: forgetting an owning pin leaks the allocation but ends the
  borrow visible to its caller. A persistent callback must neither access
  nor drop that context after forget. Keep the context on the caller side;
  the registered kernel may touch only independently retained hardware,
  payload storage and context-free queue records. A forgotten stack
  `Pin<&mut Scope>` still runs the backing scope's destructor at block exit,
  which must detach synchronously before releasing its task memory.
- `RxEvidence` keeps provenance: never derive a value and report it `HardwareObserved` (`RxEvidence`, `lower-mac/src/rx.rs`). `RxMeta::channel` is configuration, not a per-frame observation (same file).
- A BSS frame without a channel element uses its captured `RxMeta::channel`
  for scanning. The current lease/absence may already have changed before RX
  delivery; looking up that newer lease either mislabels the observation or
  invents channel zero when the lease is gone.
- The IRQ handoff belongs to the MAC driver, with no Embassy or runtime dependency. `take_batch` captures one finite RX-before-TX batch; work published during its processing stays for the next step. `Moderated` requires the platform's restore function at construction. Drain only after route quiescence; a failed quiesce retains facts and moderation. `irq-counters` counts hardware RX publications, including coalesced ones; diagnostics reads it outside the ISR. Software handoff and returned capacity do not increment it. Lane waits and batch processing have one driver consumer and must not run concurrently.
- The S31 port's radio clock is the MAC local time, affine to the monotonic clock (`MAC_CLOCK_RELATION`, `runtime/esp32s31/ieee80211/src/mac_clock.rs`): convert a port instant or stamp through that relation with a `clock_sample()` of its generation (`ClockRelation::project`), never by reading its microseconds as monotonic time. A generation comes only from its relation owner's `GenerationSource`, which is neither `Copy` nor `Clone`.
- A received frame carries `RxTimes { handoff, stamp }` (`hardware/esp32s31/driver/ieee80211/mac/src/rx/pool.rs`): the monotonic executor handoff and the raw MAC local-time receive stamp. The RX path never reads a clock; a consumer that needs the reception time converts the stamp: the station's beacon TSF follow in MAC time (`connected_control.rs`, `mac_local_time() - stamp`), the scan through a `MacClockSnapshot` (`runtime/esp32s31/ieee80211/src/mac_clock.rs`). HE Trigger/NDPA deadlines count from the handoff.
- The S31 station TSF has one writer, `StationTsf` (`hardware/esp32s31/driver/ieee80211/src/station_tsf.rs`): the hardware write takes a `StationTsfWrite` only the owner constructs, and the owner records every write in the portable `TsfRelation` (`lower-mac/src/extensions.rs`), whose generation changes at a jump, not at a beacon follow's drift. The port core and the role's connected control (one per association) each hold one, created with an epoch from `MacClockHandle::tsf_epoch`, so generations never repeat across reconnections or radio starts; never add a write around it. The access-point TSF has the same shape: `AccessPointTsf` (`hardware/esp32s31/driver/ieee80211/mac/src/ap_tsf.rs`) holds the `AccessPointTsfWrite` for `ApTsfHardware`'s reset and stop, and each restart or stop is a new generation.
- Receive configuration and TSF sampling are affine FIFO commands. Each interface gets one `ConfigurePermission` and one `TsfPermission` from its `InterfaceCommandPool`; their terminals return the same installation/role/path-branded owners. Configuration carries the composition's opaque context, including a station epoch, instead of a separate service correlation table. It returns unpublished attempts of that role before changing receive rules and waits only for published attempts; waiting for a held queue deadlocks background scanning before PM Null. Include both interface pools in command/terminal capacity formulas. EDCA, BSS color, TBTT plans, monitor and Block Ack desired state remain tokenless settings.
- The TSF is never modular: a set across 2^64, or one after the counter passed 2^64, is a jump (`TsfRelation::set`), and TWT planning refuses a window past 2^64 (`BeyondTsfRange`). A TWT wake plan carries the station TSF generation it was computed in (`StationTwtWakePlan`); a plan of another generation is stale and must never be armed. No hardware TWT wake exists yet (#85).
- A beacon whose TSF the station cannot follow is malformed, and the station takes nothing from it: its TSF, TBTT schedule, TIM and liveness come from one frame. `connected_rx` rejects a beacon that a 32-bit MAC-time elapsed could carry past 2^64 (`beacon_tsf_followable`, `ConnectedRxError::UnfollowableBeaconTsf`), so control's follow never overflows. A join beacon that cannot be advanced to the start of power management ends the association as a disconnect (`ConnectedDisconnectReason::JoinTsfUnrepresentable`, candidate refresh), not a control error, which would stop the station terminally. Never drop only the TSF write: power management would then place TBTT in a TSF the station never took.
- `ConnectedControl` remains in `roles/esp32s31/ieee80211/sta/src/connected_control.rs`; its former runtime placement slot was removed with station orchestration. The new S31 Driver/State form must own its placement before product integration. A `.psram.bss` `ZeroedStatic<MaybeUninit<_>>` needs an explicit initializer at one-time claim; zero bytes do not stand for arbitrary Rust states ([statics in zeroed regions](../../../docs/architecture.md#statics-in-zeroed-regions)).
- The RX descriptor-to-buffer binding (`dma/src/rx_storage.rs`) stores each descriptor's offset to its buffer, so the arena's zero bytes are the identity binding the host `prepare_ring` path relies on; `dma_layout_rotated` writes a rotation.
- The station's power-save default is `SleepType::None` (`ConnectedPower::new`, `roles/esp32s31/ieee80211/sta/src/connected_control/power.rs`): RF sleeps only for coexistence, so ordinary HIL runs never exercise modem sleep.
- PM coexistence deadlines are monotonic, not wrapping vendor counters. A
  slice shorter than its leave margin produces `Due`; future slice and
  preemption deadlines stay absolute across delayed batch execution.
  Listen-interval scaling stays wide until the S31 checked TBTT projection.
  Clock-input refusal retains the complete PM state and emits no effects.
- The core receives its publication watchdog in `LowerMacConfig::publication_timeout` (`hardware/esp32s31/driver/ieee80211/src/lower_mac.rs`); the old composition supervisor and its 250 ms constant were removed. The core tests supply 250 ms explicitly. A diagnostic-only `NoEffect` must still service a latched event or expired watchdog in the same bounded step. BAR completion uses the independent valid BA observation, not raw status zero alone.
- `cargo verification copies` lists MMIO-differing pairs, not all duplicate symbols. Use `check provenance` to detect ROM/library name ambiguity, and qualify every completion citation by its actual copy.
- Completion dispatch executes the library copy: `libpp[pp.o]::ppTask +0x19c` calls it directly; `libpp[wdev.o]::wdev_funcs_init +0x42c/+0x430/+0x434` installs its address for ROM table calls too. Library status 3 stops at `libpp[lmac.o]::lmacProcessTxComplete +0x252` (`wifi_assert(0)`); it is `InvalidStatus -> Poisoned`, even though the ROM body differs.
- Pinned RTS-error dispatch is not a generic protection failure: `lmacProcessTxRtsError` sends details `1`, `3..5`, `0xa0..0xad` to collision (`+0x124`), and `0xc0` to the key-error end (`+0xe6`). The remaining details only log (`+0x72`) and produce no terminal. Keep that publication until its real result or timeout ([owner clarification](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6158453731)).
- The fourth RTS-handler argument suppresses ending FES for `0xc0` when nonzero (`+0xd2`); do not derive it from the hardware Trigger-flow flag. The discovered ordinary completion callers pass zero: `libpp[lmac.o]::lmacProcessTxComplete +0x19a` and `rom[]::lmacProcessTxComplete +0x152`. `libpp[wdev.o]::wdev_funcs_init +0x900` installs the library handler in the ROM dispatch table.
- Handler queue `10` resolves through `our_tx_eb`, then its descriptor at `+52`, then queue bits `7:4` of descriptor word `+4` (`lmacProcessTxRtsError +0x1a..+0x2c`). It is not ordinary queue ten. Cite `lmacProcessTxseckiderr +0x26` for its FES end; `+0x98` is its call site inside `lmacProcessTxError`.
- `affine_spsc::split` takes `&mut` storage. The endpoints retain that exclusive borrow, so another pair cannot coexist. A new pair may receive values left by its predecessor; the queue owns them until receipt or destruction. Move the receiver itself across lifecycle boundaries; the producer cannot create one.
- Each MSDU queue endpoint has one `AtomicWaker`. Its readiness future borrows the endpoint exclusively until completion or cancellation, registers before inspecting the queue, and owns no value or credit. Keep an outgoing value with its caller until `try_send` publishes it; cancellation of readiness cannot discard it.
- Association's deadline starts at entry, before its first TX credit. A temporary comeback starts the next deadline at the scheduled return, so an occupied credit cannot leave either wait unbounded. Open station construction may have no group slots; protected construction returns its config and resources if the slot is missing.
- Portable station RX enters `Station::receive` once. MAC's `DecodedHeader` and `AdmittedData` locate QoS/HT/CCMP, require integrity evidence and commit replay before EAPOL/MSDU dispatch. The old data receive entry point exists only as a test helper; production cannot commit the same PN through a second API. `FrameControl` lives in MAC's `frame` module, shared by RX and lower-MAC header encoding.
  MSDU admission also checks the payload-bearing subtype before replay:
  Null/QoS Null and reserved subtypes cannot carry EAPOL or network data.
- A-MSDU iteration is created only after rejecting a first destination equal
  to the RFC 1042 LLC/SNAP prefix (`AA:AA:03:00:00:00`). This shared STA/AP
  boundary prevents CVE-2020-24588 before any subframe escapes; checking each
  delivered MSDU later is too late. CCMP integrity alone is not proof that
  SPP A-MSDU authenticated the outer QoS A-MSDU-present bit. Keep the guard
  on the hardware-verified path too (`mac/src/data.rs`, `aggregate_payload`).
- GTK's two-bit wire ID includes zero (IEEE 802.11-2020, 12.7.2 and the pinned ESP-IDF `wpa.c:1064`). Frame addressing distinguishes PTK from GTK; group replay records use `GroupKeyId::select`, separately from the backend's slot pool. S31 STA accepts `GroupKeyId`, including zero, in hardware slot 1. Its AP installation takes `ApGroupKeyId` (1 through 3), validated before hardware. S31 AP's `1 + key_id` layout excludes zero because it would collide with that station slot; do not impose this AP restriction on the portable station.
- Every GTK mutation consumes the installation's one permission for that
  interface; only its FIFO success terminal returns an ordinary permission.
  Never synthesize `GroupKeyInstalled` at submission. The RX boundary includes
  delayed hardware/DMA/ISR/software facts and must finish under continuous
  ingress within `GROUP_COMMAND_BOUND`, with completion/deadline wakeups.
  Exactly one installation per association/wire KeyID decrypts; other IDs may
  stay active. Select a real retained installed owner without material writes
  or counter resets, and exchange its saved replay state only at the terminal.
  Selection does not renew installation age for eviction. A full multi-slot
  pool replaces the oldest non-current key; a single-slot backend replaces
  its sole owner atomically. S31 cannot select an evicted entry from a RAM
  copy: its hardware boundary still needs proof in the new form. Hardware
  uncertainty retains resources for teardown, never an ordinary permission
  ([owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6169527922)).
- Model GTK identities may outlive the port in captured ingress or a test.
  They carry no secret: uninstall/abandon must deactivate and clear the
  installation table and captured RX independently of identity reference
  counts. Ordinary disable/reenable preserves the installed table.
- GTK conformance needs an independent fixture watchdog as well as measured
  admission/completion instants against the backend's `GROUP_COMMAND_BOUND`.
  Keep the backend deadline waker separate from the watchdog's waker. Exercise
  expiry and exact poisoned custody for every mutation, disable before/after
  execution, and AP removal held by a published group transmission.
- Wi-Fi disable delivers `PortBackend::Terminal`, not the phase's event type.
  `LowerMacEvent::Terminal` is the same terminal in the enabled FIFO. The
  backend recycles unsolicited RX during disable and keeps EventsLost for
  re-enable; it never resets the accounting of RX already lent to the body.
- Arbiter failure consumes its working owner. A refused outbound retune
  returns that owner with a home retune; a refused return yields only
  `FailedArbiter`. Keep the working arbiter outside the service's Station/Data
  state while enabling, so failure needs neither a fake replacement nor an
  optional live owner. Late gate/retune returns only drain; recovery requires
  a new installation's construction authority.

The MSDU/command SPSC sender's readiness returns a `SendPermit`. Keep the
permit through publication: its exclusive sender borrow is the capacity
proof, so `send` has no full-queue outcome. Cancelling a wait or dropping a
permit leaves the cursor unchanged. Wake registration precedes the Acquire
capacity check; the consumer can only free additional cells while held.

## Checks

Before each push run check (`--all-targets`), test
(including doctests), and clippy (`--all-targets`) without warnings, with
`--features oer-ieee80211-runtime/test-support`, on:
`oer-ieee80211-{lower-mac,sta,runtime,arbiter,upper-mac,port,mac,rsn}`,
`oer-espressif-ieee80211-policy`, `oer-esp32s31-ieee80211-mac` and
`oer-esp32s31-ieee80211`. The core package's host tests are required too;
checking only its MAC dependency misses its ESP-NOW callers.

Also check the actual S31 target before each push:
`cargo check -p oer-esp32s31-ieee80211-mac -p oer-esp32s31-ieee80211 --target riscv32imafc-unknown-none-elf`.
Host `--all-targets` covers Cargo target kinds, not the RV32 architecture or
its conditional imports and allocation paths.
Run `cargo xtask check tier full --job architecture --checks chip-doctest`
as well: chip API examples need their `no_std` harness, and must not inherit
the allocating MAC model from a host-only dev-dependency. The core enables
that dependency for host unit tests; explicit RV32 model builds still use
the `test-support` feature.
Also compile the same target with explicit models:
`cargo check -p oer-esp32s31-ieee80211-mac -p oer-esp32s31-ieee80211 --target riscv32imafc-unknown-none-elf --features oer-esp32s31-ieee80211/test-support`.
The portable runtime's model also remains `no_std` + `alloc` under its
feature; only unit tests import `std`. Check this library build with
`cargo clippy -p oer-ieee80211-runtime --target riscv32imafc-unknown-none-elf --features test-support`.

Synthetic MAC/DMA completion and detach evidence requires the explicit
`oer-esp32s31-ieee80211-mac/test-support` feature. Host pointer width is not
test authority. Core and role tests enable it through dev-dependencies;
the MAC's integration tests declare it as a required test feature. Also run
`cargo test -p oer-esp32s31-ieee80211-mac --doc` alone without that feature:
its negative contracts verify the production surface without Cargo feature
unification from a test consumer.
RX storage constructors have the same static-storage signature on host and
chip. Tests leak their allocation deliberately, so forgetting a ring cannot
turn a hardware-capable signature into a temporary-memory DMA binding.

- `cargo test -p <package>`, then `cargo xtask check changed`.
- `cargo hil images check --class performance --type-check` for chip-side changes; `cargo xtask check network` for network-owner or manifest changes.
- HIL smoke: `cargo hil run icmp-latency tcp-bidirectional station-reconnect` (`hil-run` skill).

Skills: `protocol-change`, `driver-or-hardware-change`, `hil-run`, `vendor-evidence`, `qualification-entry` (catalog `qualification/catalog/esp32s31/wifi-phy.toml`), `push-and-ci`.

Package detail: the READMEs of [lower-mac](../../../crates/protocols/ieee80211/lower-mac/README.md), [upper-mac](../../../crates/protocols/ieee80211/upper-mac/README.md), [sta](../../../crates/protocols/ieee80211/sta/README.md), [arbiter](../../../crates/protocols/ieee80211/arbiter/README.md), [S31 station role](../../../crates/roles/esp32s31/ieee80211/sta/README.md), [S31 AP role](../../../crates/roles/esp32s31/ieee80211/ap/README.md) and [S31 composition](../../../crates/composition/esp32s31/embassy/ieee80211/README.md).

Portable Station PMF keeps management CCMP replay separate from data TIDs.
Its IGTK switches only at the group-key terminal; repeated M3 and same-IGTK
rekey must preserve the existing management PN/IPN. Never remove the pairwise
key before a protected leaving Deauthentication's terminal. Model air frames
carry real CCMP ciphertext and MIC; their separate `plaintext` view is the DMA
input. Use the wire view and independently encrypted AESCCM/BIP fixtures for
crypto assertions. `receive_ccmp` must verify MIC before publishing verified
RX metadata, including SPP's A-MSDU-present bit.
Management reception wraps the same `CcmpAssociation` transform with a separate
PN frontier: do not duplicate its AAD/nonce or MIC-failure erasure. CCM accepts
only header-built `Aad` layouts (22/24/28/30 bytes), never arbitrary AAD slices.
Management's reserved CCMP octet and Key ID bits must also be zero: the pinned
vendor `ieee80211_ccmp_decrypt` rejects them before replay or decryption. The
pin and entry offsets are documented in RSN's `management_ccmp` rustdoc.
The interface-scoped model holds one hardware CCMP record per interface; keys
select that record by interface identity. The affine association owner remains
unique, and the complete returned slot bank gates any change of mode.

SPP A-MSDU belongs to the association. Backend capability `SppScope` chooses
whether hardware can program each association or only an entire interface;
AP admission is generic over that type. S31 is interface-scoped: the fourth
argument of `hal_crypto_enable` reaches interface-control bit 9, not a key
entry. Keep PTK/GTK on the same association, apply its decision before PTK,
and retain it through the last key removal. In software CCMP, SPP retains
the QoS A-MSDU-present bit in both AAD and nonce flags (Linux v6.12
`ccmp_gcmp_aad` / `ccmp_special_blocks`). The first-DA injection guard remains
required independently of this negotiation.

Association end consumes the owner and its complete `VacantKeyBank`, including
unused spare slots. Only KeyRemoved returns an installed slot; Poisoned retains
it. A partial collection cannot end an association or select another mode.
S31 needs no end command: `wDev_remove_KeyEntry` calls
`hal_crypto_clr_key_entry`, which clears validity and key words without touching
interface SPP. With the full bank back, the old bit has no CCMP effect; the next
association programs its decision before its first key. Station teardown removes
GTK before PTK so the returned PTK association owner can reunite the full bank.
