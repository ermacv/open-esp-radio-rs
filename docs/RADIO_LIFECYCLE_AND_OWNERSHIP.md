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
   └── proof impossible or cancelled ► ResetRequired

CommonReady ── power down ──► ClaimedOff
ResetRequired ── hardware reset only ──► ClaimedOff
```

`RadioConfig` and `RadioPlan` are not hardware states. Planning and capability
validation happen before the unique `ClaimedOff` owner is consumed. A failed
validation returns it unchanged. A failed hardware transition returns the
owner at the exact reached frontier, rather than manufacturing an earlier
state.

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
                                      │                         │
                                      └──── fault/cancel ──────► ResetRequired
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
    └── uncertain ownership ────► WifiResetRequired
```

The stopped owner is essential for safe application transitions. A clean
station stop can then start a standalone monitor without resetting the whole
chip, and a clean monitor stop can return to station. No such transition is
allowed directly between two live role graphs.

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

An active subsystem or Wi-Fi role graph belongs to one long-lived task. The
application receives a hardware-free controller containing only bounded
command/completion synchronization.

```text
application controller                  role/subsystem task
----------------------                  -------------------
publish stop intent  -----------------> observe at safe edge
                                        stop child tasks
                                        disable/quiesce IRQ route
                                        stop DMA and reclaim descriptors
                                        finish or quarantine TX
                                        release keys/protocol sessions
wait for completion <------------------ publish Stopped or Faulted
```

A request is not an acknowledgement. `Stopped` is legal only after the task
has returned the exact complete owner graph. Dropping a controller has no
hardware effect. Dropping or cancelling the task future is not shutdown: live
owners fail closed, reusable static arenas are poisoned, and the control domain
reports `Faulted`; that completion means the hardware frontier is
`ResetRequired`, not stopped.

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
| STA scan/join/connected/reconnect lifecycle | Implemented; scan is an internal STA state |
| Cooperative station stop and cancellation poison | Implemented; the exact application-specific phase owner and the platform attempt runner are returned together |
| Standalone normalized monitor | Implemented with bounded capture and finite IRQ/DMA shutdown |
| Standalone monitor stopped-only channel switch | Implemented on the task owner |
| Reclaimable task-stable PAC placement | Implemented by the register arena; a clean STA frontier can recover the exact `RadioRegisters`, while cancellation or an unreclaimed lease poisons reuse |
| Common reusable `WifiStopped` owner | Implemented at the common MAC/runtime boundary and returned by clean standalone-monitor dematerialization; STA now returns both halves needed for regrouping, but its production composition does not yet assemble `WifiStopped` |
| Runtime station to monitor and monitor to station transition | Not implemented |
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
| Running role to stopped Wi-Fi | Quiesce child tasks, IRQ, DMA, TX and keys before returning the role-neutral owner | Monitor returns `WifiStopped` through consuming `try_into_stopped`; station returns its phase owner plus runner and can reclaim the PAC owner, but final board-level regrouping remains |
| Station to standalone monitor | Station -> `WifiStopped` -> monitor; never a direct live transition | Not implemented |
| Standalone monitor to station | Monitor -> `WifiStopped` -> station; never a direct live transition | Not implemented |

The remaining STA regrouping work is deliberately narrow. The returned phase
owner contains stopped DMA/network/role storage and the returned runner
contains its interrupt epoch. The production composition must consume both,
recover the inactive interrupt setup and exact PAC owner, and move its owned
PHY/platform values into `Esp32s31WifiRuntimeParts`. It must not synthesize a
new setup token, reacquire PAC peripherals, or infer quiescence from a stop
request. HIL now explicitly exercises PAC-owner reclaim on its clean stop edge;
fault and cancellation paths retain reset poison.
| Standalone monitor to standalone monitor | Monitor -> `WifiStopped` -> monitor with role resources rebound to a fresh control epoch | Implemented after a clean stop; active/faulted owners are rejected |
| STA/AP with monitor tap | One role topology; tap is non-retaining and capability checked | Rejected by current capabilities |
| Wi-Fi with Bluetooth or 802.15.4 | Separate subsystem owners coordinated by coex leases | Rejected; no coex owner exists |
| Any faulted live frontier to another role | Hardware reset is mandatory | Replacement control/DMA epochs are poisoned where implemented |

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

The next required Wi-Fi API work is therefore:

1. move the station's post-cold scan/join path onto the existing
   `WifiStopped` runtime register owner;
2. make clean station shutdown return `WifiStopped` and its separate
   role-local DMA/network/executor resources;
3. retain an explicit faulted owner on every reset-only edge;
4. materialize station, AP or standalone monitor only by consuming
   `WifiStopped`;
5. add a supervisor command API after station and monitor have symmetric
   consuming transitions;
6. later extend the role topology for AP, STA+AP and monitor taps only behind
   truthful backend capabilities.

This ordering preserves the existing stop guarantees. A convenient mode
switch must not be implemented by reconstructing owners from static cells,
resetting completion flags, or weakening DMA/ISR quiescence requirements.

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
