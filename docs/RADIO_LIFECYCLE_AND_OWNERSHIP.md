# Radio lifecycle and ownership

This document defines the target lifecycle and ownership model for the whole
radio device and for protocol subsystems. It is normative architecture, not a
feature-status claim. A type or transition described here may be planned but
not implemented by a concrete backend.

Three different statements must remain distinct:

- an **architectural transition** is a shape that the design permits;
- a backend **capability** means that the complete owner graph for that
  transition exists in source code;
- **qualification** means that the implemented capability has current
  evidence in the qualification ledger.

Configuration types may describe future subsystems so unsupported requests
can fail before hardware moves. Their existence does not advertise a backend
capability.

## Ownership hierarchy

The physical radio is the root ownership domain. Protocol subsystems and
Wi-Fi roles are nested owner graphs; they are not independent handles to the
same MMIO or RF frontend.

```text
board peripheral tokens
          │
          ▼
physical radio owner
  power / clocks / resets / common calibration / RF frontend
          │
          ├── radio scheduler / coexistence owner
          │     RF leases / deadlines / protocol context switching
          │
          └── materialized subsystem set
                ├── Wi-Fi device
                │     channel contexts / Wi-Fi PHY / MAC / roles
                ├── Bluetooth device
                │     LE controller and, where supported, BR/EDR controller
                └── IEEE 802.15.4 device
                      802.15.4 PHY / MAC / protocol timing
```

The hierarchy does not imply that every chip has all three subsystems. A
single-protocol backend still follows the same model with an exclusive
scheduler that always grants the only implemented client.

Bluetooth LE and Bluetooth BR/EDR are not modelled as unrelated top-level
physical radios. They belong to one Bluetooth subsystem family and may expose
separate controller owners or scheduler clients when the hardware requires
it. This prevents ambiguous peer roots named both `bt` and `ble`, while still
allowing a chip to implement LE only, BR/EDR only, or both.

Each live hardware resource has exactly one non-cloneable Rust owner. Value
objects such as plans, capabilities, channel descriptions and reports may be
`Copy`; peripheral, DMA, interrupt-route, key-slot and protocol-session owners
may not be reconstructed from those values.

Supervisor ownership is **transitive**, not an obligation to keep every token
as a field of one stopped struct at all times. While Wi-Fi is running, the
unique Wi-Fi owner, plus the current exclusive S31 RF authority, is moved into
the selected role future. That future is itself held and polled by the local
radio-supervisor actor, so the supervisor has not handed the radio to an
arbitrary application task. On a clean terminal edge the future returns the
complete owner graph; only then may the supervisor store `WifiStopped` again
or materialize another role. With future coexistence, the actor retains the
common physical owner and scheduler while protocol futures receive linear,
bounded RF leases rather than the complete common root.

"Lending the radio to Wi-Fi" therefore means a consuming state transition,
not returning `&mut Radio`, cloning a handle, or sending a PAC/DMA owner through
a runtime channel. A borrow could not soundly survive an open-ended async
service and would make cancellation and owner return ambiguous.

## Physical radio lifecycle

The conceptual physical-radio states are:

```text
Unclaimed
   │ claim unique peripheral tokens
   ▼
ClaimedOff
   │ validate plan, power, common calibration
   ▼
CommonReady
   │ materialize supported subsystem owner set
   ▼
SubsystemsReady
   │ start one or more subsystem services
   ▼
Running
   │ cooperative stop / remove subsystem
   ▼
Quiescing
   ├── proof complete ──────────────► SubsystemsReady / CommonReady
   ├── operation still pending ─────► Quiescing
   └── contradictory hardware evidence ► HardwareFaulted<RetainedOwner>

CommonReady ── power down ──► ClaimedOff
```

`RadioConfig` and `RadioPlan` are not hardware states. Planning and capability
validation happen before the unique `ClaimedOff` owner is consumed. A failed
validation returns it unchanged. A failed hardware transition returns the
owner at the exact reached frontier, rather than manufacturing an earlier
state. `HardwareFaulted<RetainedOwner>` is terminal only for this driver
instance: it exposes no method which can construct a reusable stopped owner.
A board may reset the chip, halt or preserve diagnostics, but reset is not a
driver state transition and is not selected by a lower layer.

`CommonReady` owns only chip-wide facilities that are genuinely common:

- power and clock domains;
- RF frontend and synthesizer ownership;
- common calibration records and their validity identity;
- temperature/power compensation inputs shared by protocols;
- the ability to create the chip's radio scheduler.

It does not contain Wi-Fi rates, Bluetooth access addresses, 802.15.4 symbol
timing or any protocol MAC state.

## Subsystem lifecycle

Every selected protocol has a finite subsystem lifecycle:

```text
Absent -> Prepared -> WaitingForRadio/Running -> Quiescing -> Stopped
                                      │              │
                                      │              └─ pending -> Quiescing
                                      └─ contradictory evidence
                                             -> Faulted<RetainedOwner>
```

`Prepared` owns protocol-specific static resources and a scheduler client, but
has no live DMA/IRQ transaction. `Running` may alternate between holding an RF
lease and waiting for one; waiting does not destroy protocol state. `Stopped`
has returned every live transaction and can be dematerialized back into the
parent radio owner.

With coexistence, stopping Wi-Fi need not stop Bluetooth or IEEE 802.15.4. The
parent radio remains `Running` while any subsystem is live and becomes
`SubsystemsReady` only after all selected subsystem services are quiescent.
No subsystem may directly power down common RF resources while another
subsystem owner exists.

## Coexistence and radio scheduling

Coexistence is a chip-wide ownership service, not a Wi-Fi feature and not a
portable universal PHY. It arbitrates the physical resource shared by Wi-Fi,
Bluetooth and IEEE 802.15.4.

A scheduler request conceptually contains:

- protocol/client identity;
- earliest start and optional deadline;
- expected bounded duration;
- priority and preemption policy;
- operation class, such as beacon window, connection event, scan dwell or
  ordinary transmit.

The returned lease proves temporary permission to use the shared RF frontend.
It does not transfer ownership of another protocol's state. Protocol context
save/restore belongs to the chip-specific scheduler/backend and must itself be
a finite typed transition.

The scheduler must not participate through a slow async queue in every
hard-real-time response. Wi-Fi ACK, Bluetooth connection-event turnaround and
802.15.4 ACK timing remain inside the corresponding hardware/lower-MAC fast
path. Coexistence grants a suitable time window before those operations and
enforces ownership at window boundaries.

The current ESP32-S31 backend has no source-owned coexistence service. Its
capabilities therefore describe exclusive Wi-Fi ownership. Future scheduler
types must not cause those capabilities to claim coexistence prematurely.

## Wi-Fi device and role ownership

Wi-Fi is one radio subsystem with its own role-neutral stopped owner. The
target boundary is conceptually `WifiStopped`: common Wi-Fi PHY/MAC setup and
persistent storage are retained, while no role owns a live IRQ route, DMA
walker, TX transaction or key session.

```text
WifiStopped
    │ materialize a capability-checked role topology
    ▼
WifiRolesRunning
    │ cooperative role shutdown
    ▼
WifiRolesQuiescing
    ├── complete owner return ──► WifiStopped
    ├── operation still pending ──► WifiRolesQuiescing
    └── contradictory evidence ───► WifiFaulted<RetainedOwner>
```

The stopped owner is essential for safe application transitions. A clean
station stop can then start a standalone monitor without resetting the whole
chip, and a clean monitor stop can return to station. No such transition is
allowed directly between two live role graphs.

The role-neutral owner and role-local resources remain separate while a role
is stopped. Starting a role consumes both values. Its task may move the local
DMA, network and executor owners through any number of internal phases, but a
clean terminal edge must return them before `WifiStopped` can be reconstructed.
Keeping an unused copy of a resource bundle beside a running task is not
evidence that the resources themselves were reclaimed.

Wi-Fi materializes a **role topology**, not necessarily one enum mode. Future
capabilities may permit:

- one station VIF;
- one access-point VIF;
- STA and AP VIFs sharing one channel context;
- a standalone monitor which exclusively owns the receive path;
- a bounded monitor tap attached to a normal STA/AP receive path.

The concrete backend validates the complete topology before moving role
resources. One chip may support only a strict subset.

### Station

Scanning is a station lifecycle state, not a peer Wi-Fi role:

```text
StationPrepared
  -> Scanning
  -> Authenticating
  -> Associating
  -> Securing
  -> Connected
       ├── peer loss / reconnect request -> ReconnectBackoff -> Scanning
       └── stop request                  -> Quiescing -> WifiStopped
```

An application reconnect request may intentionally tear down the connected
epoch, scan and establish a new one. A future observation-only scan request
needs a separate contract: it must state whether an active connection is
disconnected, whether only the current channel is observed, or whether bounded
off-channel dwell is permitted. It must not silently reuse reconnect policy.

### Access point

AP is a peer protocol role, not a station mode. Its future owner graph contains
channel/TBTT and beacon ownership, a bounded peer set, per-peer security and
TX/RX state. In an STA+AP topology both VIFs may share one channel context; the
AP channel policy must then follow the scheduler and STA constraints rather
than independently retune the radio.

No AP owner graph is currently implemented for ESP32-S31.

### Monitor

Two monitor forms have different ownership:

- **standalone monitor** is an exclusive role topology. It owns promiscuous RX,
  RX DMA and the role's interrupt epoch;
- a **monitor tap** is a bounded non-retaining observer attached to a normal
  RX pipeline. It is not a VIF and must not own or backpressure the DMA ring.

A slow tap may lose observations from its own capture pool but cannot delay
normal station/AP receive processing. Raw, normalized and protocol-validated
tap points are separate capabilities.

Standalone monitor channel switching is a finite stopped-only transition
unless a future hopping service explicitly owns both the active capture epoch
and its channel schedule. Merely writing a new channel while RX DMA is live is
not an acceptable application interface.

## Task and controller split

The physical radio supervisor is one long-lived local actor. An active
subsystem or Wi-Fi role graph is a finite future owned by that actor, not a
detached owner-holding task. The application receives a hardware-free
controller containing only bounded command/completion synchronization.

```text
application controller                  local radio-supervisor actor
----------------------                  ----------------------------
publish start plan   -----------------> consume WifiStopped/resources
                                        pin and poll selected role future
publish stop intent  -----------------> request role stop at a safe edge
                                        keep polling the same future
                                        stop child tasks
                                        disable/quiesce IRQ route
                                        stop DMA and reclaim descriptors
                                        finish or quarantine TX
                                        release keys/protocol sessions
wait for completion <------------------ retain Stopped or Faulted owner
```

A request is not an acknowledgement. `Stopped` is legal only after the task
has returned the exact complete owner graph. Dropping a controller has no
hardware effect. Dropping or cancelling the task future is not shutdown: live
owners fail closed, reusable static arenas are poisoned, and the control domain
reports `Faulted`; that completion means the hardware frontier is retained and
cannot be reused. It does not mean that a lower layer requested or performed
reset.

`Drop` may perform a bounded best-effort mask/quarantine operation, but may not
spin, wait asynchronously, claim quiescence or release storage that hardware
might still address. Reset poison is sticky until a board/chip reset
reinitializes the relevant domain.

## Memory and interrupt ownership

Persistent storage and a live transaction are distinct owners:

- a DMA arena owns stable memory placement and address tables;
- a prepared ring owns initialized descriptors but no active walker;
- a live ring token proves hardware may access the arena;
- a stopped ring proves the walker released every descriptor;
- a lost live token poisons that arena for reset.

The same rule applies to interrupt routing. A callback/function pointer is not
proof of route ownership. One epoch owner couples the platform route, mask,
wake runtime and the hardware state which can produce events. A new epoch may
start only after the previous route is confirmed inactive.

## Current ESP32-S31 mapping

The following table separates existing source from the target architecture.

| Boundary | Current state |
| --- | --- |
| Value-only radio/Wi-Fi planning and capability rejection | Implemented by `RadioConfig`, `RadioPlan`, `WifiConfig` and `WifiPlan` |
| Unique physical owner and Wi-Fi cold start | Implemented by `Radio` and `start_esp32s31_radio` |
| Post-cold-start role narrowing | Implemented by `Esp32s31StartedRadio::{try_into_station, try_into_standalone_monitor}` |
| STA scan/join/connected/reconnect lifecycle | Implemented inside one outer station task. The cold owner begins with identity only; `InitialScan` creates the selected peer before `InitialJoin` |
| Cooperative station stop and cancellation poison | Implemented; the exact application-specific phase owner and platform attempt runner are returned together. Protocol-v16 HIL requires clean PAC/IRQ/resource regroup, reconstructs `WifiStopped`, completes two monitor start/stop epochs from the same returned resources and requires a replacement station to reach connected entry and quiesce again |
| Standalone normalized monitor | Implemented with bounded capture and finite IRQ/DMA shutdown |
| Standalone monitor stopped-only channel switch | Implemented on the task owner |
| Reclaimable task-stable PAC placement | Implemented by the register arena and common station reclaim/restore transactions. All four STA phases recover or republish the exact `RadioRegisters`; an active IRQ epoch, cancellation or an unreclaimed lease returns a faulted owner and cannot expose reusable state |
| Common reusable `WifiStopped` owner | Implemented at the common MAC/runtime boundary. Station materialization consumes it before a phase starts; common runtime reclaim verifies inactive IRQ plus PAC return before the station role can reconstruct it. The returned phase graph can be rebound to a fresh station identity without starting DMA; standalone monitor has the same clean dematerialization |
| Runtime station to standalone monitor transition | Qualified in HIL through `WifiStopped`; hardware-free controller, stopped dispatcher, local actor shell and concrete station/monitor future drivers exist. The production ESP32-S31 epoch-runner composition remains |
| Runtime standalone monitor to station transition | HIL starts a second real station task from the monitor-returned owner, requires connected entry, stops it and reclaims the exact owner graph again; qualified on the current board |
| Concurrent monitor tap with STA/AP | Described by portable configuration, rejected by current S31 capabilities |
| AP or STA+AP owner graph | Not implemented and rejected by current S31 capabilities |
| Bluetooth LE, Bluetooth BR/EDR and IEEE 802.15.4 owner graphs | Not implemented; subsystem requests are configuration vocabulary only |
| Coexistence scheduler | Not implemented; current radio ownership is exclusive |

The current ESP32-S31 Wi-Fi capability profile intentionally advertises one
station or one standalone normalized monitor, one channel context, no AP, no
STA+AP and no concurrent monitor tap. Future code must update capabilities
only when the complete owner graph exists, and qualification remains a
separate evidence step.

The application-visible transition matrix is therefore:

| Requested transition | Architectural rule | Current ESP32-S31 API |
| --- | --- | --- |
| Connected STA to rescan/reconnect | Internal finite STA transition; return all connected-epoch owners before scanning | Implemented by `request_reconnect` and peer-loss handling |
| Running role to stopped Wi-Fi | Quiesce child tasks, IRQ, DMA, TX and keys before returning the role-neutral owner | Monitor returns `WifiStopped` through consuming `try_into_stopped`; station uses the common runtime reclaim transaction before `Esp32s31StationRoleOwner::into_stopped`. HIL and the production example both package the returned role-local graph; binding that production graph to the supervisor epoch runner remains |
| Station to standalone monitor | Station -> `WifiStopped` -> monitor; never a direct live transition | Qualified in HIL with the exact station-returned PAC, interrupt route and role resources; public commands exist, S31 backend binding remains |
| Standalone monitor to station | Monitor -> `WifiStopped` -> station; never a direct live transition | HIL consumes the monitor-returned owner into a second station scan/join/security/connected epoch, then cooperatively stops and regroups it without manufacturing PAC/IRQ capabilities; qualified on the current board |
| Standalone monitor to standalone monitor | Monitor -> `WifiStopped` -> monitor with role resources rebound to a fresh control epoch | Implemented after a clean stop; active/faulted owners are rejected |
| STA/AP with monitor tap | One role topology; tap is non-retaining and capability checked | Rejected by current capabilities |
| Wi-Fi with Bluetooth or 802.15.4 | Separate subsystem owners coordinated by coex leases | Rejected; no coex owner exists |
| Any faulted live frontier to another role | Hardware reset is mandatory | Replacement control/DMA epochs are poisoned where implemented |

The common station resource graph now groups the physical PHY/platform/IRQ
authority, reusable DMA/TX/scratch storage and board services as three explicit
owners. Both the production example and HIL carry that graph through station
phases. Common reclaim and restore transactions normalize the direct-PAC and
arena-backed phase frontiers while returning the original owner on failure.
The stopped phase can also be rebound for a new station identity: old initial
join state returns to initial scan, while disconnected/reconnected resources
return to running scan so the next candidate replaces the old peer. This
rebind never starts or stops DMA and therefore rejects a live pre-connected
ring. The returned phase owner is reunited with the station role owner only by
the existing `into_stopped` transition. That transition alone may rebuild
`RadioRegisters` plus PHY/platform values into `Esp32s31WifiRuntimeParts`. It
must not synthesize a new setup token, reacquire PAC peripherals, or infer
quiescence from a stop request. Fault and cancellation paths retain reset
poison.

## Target application composition

Normal firmware should put dynamic role selection in one radio-supervisor
task. That task, not arbitrary application code, owns the stopped hardware
frontiers and performs consuming transitions.

```text
application tasks
      │ typed requests / reports only
      ▼
radio supervisor
      ├── physical radio owner
      ├── coexistence scheduler, when implemented
      ├── stopped/running subsystem owners
      └── current Wi-Fi role topology
```

The station's initial scan and join path now starts from the existing
`WifiStopped` runtime register owner. Its common resource graph is also shared
by the production example and HIL. The common station engine now owns
phase dispatch and the candidate-refresh precondition for both consumers.
Connected RX/TX/control construction, the finite channel/authentication/
association/WPA2 join transaction and the complete quiesce/teardown
transaction are common. Initial and running scan now also use one common
owner-returning transaction. The initial invocation is a real actor-owned
phase and cannot be bypassed by a caller-proven candidate; scan observation
is emitted through a separate read-only observer, while hardware ports retain
only finite owner transactions and the hardware-safety
classification of scan failures is shared. Join resources are transferred as
one named owner bundle instead of a consumer-specific positional argument
list. Initial and reconnected connected epochs now additionally use
one owner-returning PAC publication and live-RX transition; first-epoch static
resources are never initialized on reconnect. Embassy network ownership is
also chip-independent and common: the device constructs a stack once, while
the exact stack and radio runner survive reassociation. Board-facing task
stop/owner-return signalling is common as well and poisons reuse when either
endpoint disappears before completion; raw signal reset is not an allowed
recovery operation. The primary protocol owner is additionally hidden behind
a common auxiliary-task group until every per-epoch endpoint completes. HIL
benchmark traffic now uses that contract instead of resettable stop/stopped
signals, so a stale or aborted benchmark task cannot qualify station teardown.
The connected runner can no longer be sequenced
differently by HIL and normal firmware: one transaction performs observation,
classifies the still-live runner exit, closes IRQ publication and task
ownership, then carries that exit through control/RX/TX/key teardown. The
persistent network start/reuse transition is common as well; consumers supply
only their stack initializer and IP policy. Concrete RX/TX/control owners now
enter one assembly transaction which also joins the persistent network runner;
HIL fault injection is a statically dispatched service decorator applied only
after successful production composition. The staged RX protocol task shares
one cancellation/drain/owner-return operation; only executor/core placement
remains board policy. The next required Wi-Fi API work is
therefore:

1. bind the existing supervisor actor shell to the default production
   ESP32-S31 resource profile; its command API and symmetric local role drivers
   plus station reclaim/restore/rebind transactions are implemented;
2. make the production engine consume `StationRequest` as the only SSID, PMK,
   scan and reconnect policy source, then return either `WifiStopped` plus role
   resources or an opaque `Faulted<RetainedOwner>`;
3. move the remaining connected policy wiring out of the production and HIL
   hardware ports; observations are already a separate read-only engine hook;
4. reduce the example to board allocation, application requests and network
   services, and HIL to protocol/measurement/fault hooks;
5. later extend the role topology for AP, STA+AP and monitor taps only behind
   truthful backend capabilities.

This ordering preserves the existing stop guarantees. A convenient mode
switch must not be implemented by reconstructing owners from static cells,
resetting completion flags, or weakening DMA/ISR quiescence requirements.

The connected-service extraction is incremental but its boundaries are
already normative. Example and HIL consume the same initial/reconnected epoch,
network and security handoff, and atomically compose the same RX/TX/control
graph. They use one fixed connected interrupt activation which emits the
non-forging durable RX probe, and one shutdown transaction which closes IRQ
publication, returns executor task owners, tears down control/RX/TX/keys and
returns network plus driver owners together. Board-specific logging, traffic
tasks and fault classification remain outside that transaction.

The application control plane now has a hardware-free typestate, request and
mailbox implementation in `radio::supervisor`. `WifiServiceRequest` combines a
checked `WifiPlan` with independent owned station/monitor policies; it is not a
global mode enum and can later carry concurrent STA+AP and monitor-tap
services. `WifiSupervisorConfiguration` records provisioned role resources
and checks a fresh active topology per command. The Embassy mailbox gives
applications a controller containing no hardware capability. The earlier
detached-service scaffold was removed because returning a `Running` value from
`start()` did not itself keep a `!Send` role future progressing. The S31
adapter now drives the real station and standalone-monitor futures locally
alongside the supervisor command endpoint. Applications construct this pair
through `prepare_esp32s31_radio_supervisor`: only the hardware-free controller
escapes, while `Esp32s31RadioSupervisorTask` retains the endpoint, stopped
owner and concrete epoch runner together. Its internal actor retains the
stopped owner between complete `run_epoch` futures, performs planning before
hardware moves, advances generations only after accepted starts and
permanently quarantines a faulted owner while rejecting further commands.
Preparation failure returns the runner and stopped frontier instead of
dropping them. The complete radio graph is intentionally `!Send`, so it must
not cross an
executor-task boundary in a `Signal` or `Channel`. The actor instead selects
between its hardware-free command endpoint and the locally held role future.
A stop command requests cooperative shutdown but the stop response is
published only after that same future returns a quiescent owner. The station
security frontier owns its PMK and sequence counters by value, so credentials
move with that future and can change only after the preceding service has
stopped. The physical supervisor can then compose independent Wi-Fi,
Bluetooth and IEEE 802.15.4 child states with common-RF/coex ownership above
them.

## Multiple chip backends

The lifecycle vocabulary is chip-independent; the hardware owner graph is
not. ESP32-S31 and a future ESP32-C5 backend each provide their own PAC/HAL,
register transactions, calibration storage and concrete capabilities.

Portable plans and protocol state must not contain MMIO base addresses, chip
names, vendor ABI versions or fixed global-parameter layouts. A chip backend
maps semantic operations to its PAC and reports the role/subsystem graphs it
actually implements. Similar register geometry between chip generations is a
reason to compare implementations, not sufficient evidence for one shared
unsafe or register layer.

Common RF algorithms, scheduler contracts or protocol-independent DMA
primitives move into shared crates only after at least two real consumers
demonstrate the same ownership and failure semantics. Calibration records
remain tagged by the concrete identity needed to prove that reuse is valid;
they are data returned by a transition, not hidden global state.
