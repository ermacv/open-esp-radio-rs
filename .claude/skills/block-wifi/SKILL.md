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

The port is on the owner shape: `LowerMacBackend` and its parts in `crates/protocols/ieee80211/lower-mac/src/owner.rs`, its lifecycle (`run_port`, `Owner`, `PortBackend`) in `oer-ieee80211-port` (`crates/protocols/ieee80211/port`). The S31 backend is `Esp32s31Backend` over the static runtime `Esp32s31LowerMac` (`crates/runtime/esp32s31/ieee80211/src/lower_mac.rs`); it has no A-MPDU, beacon-timing or monitor operation yet, and the product datapath still drives the hardware directly (`DatapathRunner`) until #709 step 2 moves it onto the port. The portable port services (upper-mac service, ap, sta-ap, the sta port station) were deleted in #709 step 1; git history keeps them for the rebuild on the portable stack. The station is the portable sans-IO `oer_ieee80211_sta::machine` beside the channel arbiter (`oer-ieee80211-arbiter`); the S31 roles' orchestration (station, AP, monitor, ESP-NOW, STA+AP) was removed and returns on the port in #563's step 2 (tag `s31-station-old-path` keeps the old path).

## Shared contracts (change only with their owners)

`crates/radio/port` (`RadioPort`, `PortResult`, `Poisoned`, `ClockInfo`, `ClockRelation`, `RelationSample`, `Generation`), `crates/time` (`Instant`, `RadioInstant`), `crates/radio/coex`, the shared radio system `crates/runtime/esp32s31/radio` and `crates/composition/esp32s31/embassy/radio`, the HAL split `crates/hardware/esp32s31/hal/src/root.rs`, PAC and `registers/`.

## Traps

- Keep each package's `clippy.toml` as a regular source file. HIL source
  snapshots retain lint configuration and reject build-input symlinks; a
  symlink to a shared configuration prevents image capture before compilation.

- A backend declares its complete `PortBackend::Pools` layout and consumes `Mint::into_pools(self)` once. Include one `GatePool`; it yields `GateOpen` and the backend's `GateIssuer`, with no public `GatePlace` constructor. Each tuple component has a sealed pool path, including duplicate `GatePool` components; retain that path through gate/issuer and the phase's `GatePath`. Do not erase ticket, key, buffer or connection-source paths in wrappers. The source's path also stays in station/data epochs and contexts; two sources in one installation cannot produce interchangeable epochs.

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

- CCMP replay preparation returns an exclusive borrow of the selected
  frontier. Commit the token itself before publication, or drop it to cancel.
  Never replace this with a transferable PN/revision pair: resetting a key
  with the same wire ID also resets its revision, letting an old candidate
  corrupt the new frontier. A new material epoch starts at KeyRSC; keeping
  identical material means keeping the original replay owner.

- Pending key actions and installed GTK records retain `CcmpKey`, whose
  Debug is redacted and whose destructor zeroizes its bytes. Compare through
  borrowed material; a raw `[u8; 16]` in a derived Debug token exposes GTK.
  This owner discipline does not prove erasure of compiler temporaries.

- Authentication's `NeedCredit` owns only the retry runtime. An active
  `StaAuthenticationExchange` is created after ticket/buffer reservation and
  is consumed by response or timeout; only Retry returns the runtime. Do not
  call an active-exchange RX parser while waiting for credit or restore
  separate `active`/`terminal` flags beside the machine's owned stage.

- Connected plaintext EAPOL may recover only the exact authenticated completed
  M3, through `on_duplicate_message3`. Its owned M4 stage coalesces repeats
  until TX ends and precedes the next group response after a busy control TX.
  It never installs keys or resets replay. Group M1 requires the protected RX
  path, including integrity and replay admission; both replies use the PTK.

- Management disconnect admission is shared by all portable join/link stages:
  `parse_sta_disconnect` takes OwnAddress/Bssid and accepts our unicast or
  broadcast from that BSS. It never parses Protected ciphertext as a reason;
  PMF requires its separate authenticated-management owner, not RX metadata.

- Station commands enter through `Station::command`, consuming `StaRequest`.
  Each command kind reserves its own terminal cell; completion returns that
  request and refusal returns it synchronously. Never replace those cells
  with one optional notice: a refused background absence and a Disconnect can
  both owe results before the next action poll. Keep the exact AbsenceRefusal.

- Every port method but `next_event` is synchronous (`port.rs`, the trait's documentation).
- A port has one event stream in occurrence order (#266 D8): the S31 runtime numbers every event under the core's lock and merges its four queues by that number (`runtime/esp32s31/ieee80211/src/lower_mac.rs`, `Queues::take`). An extension event names its kind and interface (`LowerMacEvent::Extension { kind, vif }`), and the router routes it by that interface, never by role.
- Interrupt model ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals), #643): the MAC and power interrupts only read and clear their status and post the events, through `IrqConsumer::publish` (`hardware/esp32s31/driver/ieee80211/mac/src/irq/consumer.rs`); the port's owner task runs `LowerMacCore::service`. No Wi-Fi decision has a software deadline below 1 ms (ACK and Block Ack turnaround are hardware). The port backend `Esp32s31LowerMac` still runs `service` inside `on_interrupt` under the core's blocking lock; #563 removes it, so never add interrupt-side logic there.
- `RxEvidence` keeps provenance: never derive a value and report it `HardwareObserved` (`RxEvidence`, `lower-mac/src/rx.rs`). `RxMeta::channel` is configuration, not a per-frame observation (same file).
- The IRQ handoff belongs to the MAC driver, with no Embassy or runtime dependency. `take_batch` captures one finite RX-before-TX batch; work published during its processing stays for the next step. `Moderated` requires the platform's restore function at construction. Drain only after route quiescence; a failed quiesce retains facts and moderation. `irq-counters` counts hardware RX publications, including coalesced ones; diagnostics reads it outside the ISR. Software handoff and returned capacity do not increment it. Lane waits and batch processing have one driver consumer and must not run concurrently.
- The S31 port's radio clock is the MAC local time, affine to the monotonic clock (`MAC_CLOCK_RELATION`, `runtime/esp32s31/ieee80211/src/mac_clock.rs`): convert a port instant or stamp through that relation with a `clock_sample()` of its generation (`ClockRelation::project`), never by reading its microseconds as monotonic time. A generation comes only from its relation owner's `GenerationSource`, which is neither `Copy` nor `Clone`.
- A received frame carries `RxTimes { handoff, stamp }` (`hardware/esp32s31/driver/ieee80211/mac/src/rx/pool.rs`): the monotonic executor handoff and the raw MAC local-time receive stamp. The RX path never reads a clock; a consumer that needs the reception time converts the stamp: the station's beacon TSF follow in MAC time (`connected_control.rs`, `mac_local_time() - stamp`), the scan through a `MacClockSnapshot` (`runtime/esp32s31/ieee80211/src/mac_clock.rs`). HE Trigger/NDPA deadlines count from the handoff.
- The S31 station TSF has one writer, `StationTsf` (`hardware/esp32s31/driver/ieee80211/src/station_tsf.rs`): the hardware write takes a `StationTsfWrite` only the owner constructs, and the owner records every write in the portable `TsfRelation` (`lower-mac/src/extensions.rs`), whose generation changes at a jump, not at a beacon follow's drift. The port core and the role's connected control (one per association) each hold one, created with an epoch from `MacClockHandle::tsf_epoch`, so generations never repeat across reconnections or radio starts; never add a write around it. The access-point TSF has the same shape: `AccessPointTsf` (`hardware/esp32s31/driver/ieee80211/mac/src/ap_tsf.rs`) holds the `AccessPointTsfWrite` for `ApTsfHardware`'s reset and stop, and each restart or stop is a new generation.
- The TSF is never modular: a set across 2^64, or one after the counter passed 2^64, is a jump (`TsfRelation::set`), and TWT planning refuses a window past 2^64 (`BeyondTsfRange`). A TWT wake plan carries the station TSF generation it was computed in (`StationTwtWakePlan`); a plan of another generation is stale and must never be armed. No hardware TWT wake exists yet (#85).
- A beacon whose TSF the station cannot follow is malformed, and the station takes nothing from it: its TSF, TBTT schedule, TIM and liveness come from one frame. `connected_rx` rejects a beacon that a 32-bit MAC-time elapsed could carry past 2^64 (`beacon_tsf_followable`, `ConnectedRxError::UnfollowableBeaconTsf`), so control's follow never overflows. A join beacon that cannot be advanced to the start of power management ends the association as a disconnect (`ConnectedDisconnectReason::JoinTsfUnrepresentable`, candidate refresh), not a control error, which would stop the station terminally. Never drop only the TSF write: power management would then place TBTT in a TSF the station never took.
- The connected control (`ConnectedControl`, ~2.3 KB) is placed in a static slot (`runtime/esp32s31/ieee80211/src/roles/station/control_slot.rs`) and reached through a `PlacedControl` lease: configure it with `&mut` setters after placement, and never move it by value through services, teardown or faults, where each copy multiplies its size in the connected epoch's frames. Its `.psram.bss` cell is a `ZeroedStatic<MaybeUninit<_>>` written with `ConnectedControlSlot::new()` at the one-time claim: an empty slot is not all-zero bytes, and a zeroed region discards any other initializer ([statics in zeroed regions](../../../docs/architecture.md#statics-in-zeroed-regions)).
- The RX descriptor-to-buffer binding (`dma/src/rx_storage.rs`) stores each descriptor's offset to its buffer, so the arena's zero bytes are the identity binding the host `prepare_ring` path relies on; `dma_layout_rotated` writes a rotation.
- The station's power-save default is `SleepType::None` (`ConnectedPower::new`, `roles/esp32s31/ieee80211/sta/src/connected_control/power.rs`): RF sleeps only for coexistence, so ordinary HIL runs never exercise modem sleep.
- The TX completion and publication timeout is one composition constant, 250 ms (`TX_COMPLETION_TIMEOUT`, `composition/esp32s31/embassy/ieee80211/src/supervisor/mod.rs`).
- `cargo verification copies` lists MMIO-differing pairs, not all duplicate symbols. Use `check provenance` to detect ROM/library name ambiguity, and qualify every completion citation by its actual copy.
- Completion dispatch executes the library copy: `libpp[pp.o]::ppTask +0x19c` calls it directly; `libpp[wdev.o]::wdev_funcs_init +0x42c/+0x430/+0x434` installs its address for ROM table calls too. Library status 3 stops at `libpp[lmac.o]::lmacProcessTxComplete +0x252` (`wifi_assert(0)`); it is `InvalidStatus -> Poisoned`, even though the ROM body differs.
- Pinned RTS-error dispatch is not a generic protection failure: `lmacProcessTxRtsError` sends details `1`, `3..5`, `0xa0..0xad` to collision (`+0x124`), and `0xc0` to the key-error end (`+0xe6`). The remaining details only log (`+0x72`) and produce no terminal. Keep that publication until its real result or timeout ([owner clarification](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6158453731)).
- The fourth RTS-handler argument suppresses ending FES for `0xc0` when nonzero (`+0xd2`); do not derive it from the hardware Trigger-flow flag. The discovered ordinary completion callers pass zero: `libpp[lmac.o]::lmacProcessTxComplete +0x19a` and `rom[]::lmacProcessTxComplete +0x152`. `libpp[wdev.o]::wdev_funcs_init +0x900` installs the library handler in the ROM dispatch table.
- Handler queue `10` resolves through `our_tx_eb`, then its descriptor at `+52`, then queue bits `7:4` of descriptor word `+4` (`lmacProcessTxRtsError +0x1a..+0x2c`). It is not ordinary queue ten. Cite `lmacProcessTxseckiderr +0x26` for its FES end; `+0x98` is its call site inside `lmacProcessTxError`.
- `affine_spsc::split` takes `&mut` storage. The endpoints retain that exclusive borrow, so another pair cannot coexist. A new pair may receive values left by its predecessor; the queue owns them until receipt or destruction. Move the receiver itself across lifecycle boundaries; the producer cannot create one.
- Each MSDU queue endpoint has one `AtomicWaker`. Its readiness future borrows the endpoint exclusively until completion or cancellation, registers before inspecting the queue, and owns no value or credit. Keep an outgoing value with its caller until `try_send` publishes it; cancellation of readiness cannot discard it.
- Association's deadline starts at entry, before its first TX credit. A temporary comeback starts the next deadline at the scheduled return, so an occupied credit cannot leave either wait unbounded. Open station construction may have no group slots; protected construction returns its config and resources if the slot is missing.
- Portable station RX enters `Station::receive` once. MAC's `DecodedHeader` and `AdmittedData` locate QoS/HT/CCMP, require integrity evidence and commit replay before EAPOL/MSDU dispatch. The old data receive entry point exists only as a test helper; production cannot commit the same PN through a second API. `FrameControl` lives in MAC's `frame` module, shared by RX and lower-MAC header encoding.
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
