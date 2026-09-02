# Bluetooth Code Architecture Audit

Date: 2026-09-02

## Outcome

The Bluetooth implementation has sound crate-level ownership direction, but
several files have become integration monoliths. This is not a radio-semantics
blocker by itself. It is a development blocker: adding another role currently
repeats the same scheduler-completion pipeline in several places and makes
missing or misordered transitions difficult to review.

The refactor must preserve these boundaries:

- generated PAC accessors remain the only register-field interface;
- controller-SRAM codecs remain private to the memory crate;
- the chip crate owns hardware sequencing and affine publication states;
- the portable HCI and Link Layer crates own protocol policy;
- the Embassy adapter owns waiting and orchestration, not radio semantics.

Splitting by line count alone would make the code harder to follow. The useful
boundaries are hardware transaction, radio role, and ownership phase.

## Current hierarchy

The compile-time dependency direction is acyclic:

    bt-hci
       ^
    portable Bluetooth HCI       portable Bluetooth LL
                 \                 /
                  ESP32-S31 Bluetooth LLL/controller
                    |          |             |       \
                  role       controller   common PHY  closed-PAC
                 events        memory                  domains
                    \          |             /
                     +----> ESP32-S31 HAL ----> generated PAC
                                                ^
    Embassy adapter ------> chip/controller + portable HCI/LL
    production integration ------> Embassy adapter + static resources

The portable LL has no chip dependency, and portable HCI has no ESP32-S31
dependency. The memory crate depends downward on HAL semantic address, fence
and completion types. The Embassy adapter depends on the chip controller, not
the reverse. These are healthy, mechanically enforced boundaries.

There are three hierarchy weaknesses:

1. The ESP32-S31 Bluetooth crate is simultaneously the physical controller,
   low-level Link Layer backend and part of command orchestration. This is the
   main internal pile.
2. The chip crate has a permitted direct dependency on the closed PAC. Most
   uses are generated semantic domains or affine observations, but the
   exception is broad. New runtime register transactions should enter through
   HAL; direct PAC use should contract to cold ownership and generated value
   types, then be re-audited.
3. embassy-sync appears in the chip and portable HCI crates. The primitives are
   executor-neutral in practice, but they couple the core layers to one async
   ecosystem. This is not a current blocker because the project is
   intentionally Embassy-only. Waiting policy and task selection must still
   stay in the adapter.

The first weakness has one concrete priority-zero inversion: HCI queue
resources and their `RawMutex` are currently joined to the controller typestate
before modem low-power timing, common PHY, baseband and BLE PHY initialization
finish. Hardware readiness must not depend on a transport or executor mutex.
The clean cut is a transport-free `BluetoothControllerHardwareReady` owner,
followed by a separate Controller composition step which joins HCI resources.
This should be corrected before expanding the HCI command surface.

The exact safe cut is after interrupt-owner publication and before production
system composition. Up to `BluetoothControllerInterruptOwnersPublished`, the
chain should carry only platform, timer, scheduler, PHY and interrupt-storage
types. A final `BluetoothControllerHciBound` should then join the already
published hardware owner to `LeControllerHciResources`; only that type and its
runtime endpoints may carry `RawMutex` and HCI queue capacities. CPU interrupt
routes are still inactive at this seam, and no HCI endpoint borrow exists, so
failure can return both affine owners without rollback MMIO.

The target internal hierarchy is:

    Embassy actor: wait, wake, select, cancellation
                         |
    Controller: HCI command epoch and role arbitration
                         |
    ESP32-S31 LLL: role event preparation and completion
                         |
    Scheduler core: list epoch, time, head/RUN, IRQ completion
                  /                       \
    SRAM memory codecs                 narrow HAL transactions
                                             |
                                      generated closed PAC

Dependencies may point downward only. In particular, scheduler core must not
know HCI commands, memory codecs must not know controller roles, and the
Embassy actor must not interpret registers or SRAM descriptor fields.

The long-term crate boundary should make that direction explicit:

    S31 contracts: controller SRAM address/time/list value domains
                                 |
    generated PAC -> HAL     memory layout/codecs
                    \          /
                 ESP32-S31 LLL
                         |
                    Controller
                         |
                 Embassy adapter

Today the memory crate also consumes HAL publication/completion proofs. This
is safe and preserves the required fences, but combines descriptor codecs with
the outer hardware ownership protocol. Split those as internal `codec` and
`protocol` layers first; only then consider separate crates. Do not remove the
affine proof joins merely to eliminate the dependency.

## Inventory

The audited Bluetooth Rust roots contain about 77,700 lines. The largest files
at the audit point were:

| File | Lines | Mixed responsibilities |
|---|---:|---|
| chips/.../bluetooth/src/scheduler.rs | 7,447 | Four roles, timeline admission, list ownership, completion and recycle |
| chips/.../src/controller_start.rs | 6,482 | Startup, time requests, HCI ordering, task service, scheduler and interrupt service |
| adapters/.../controller_command_task.rs | 4,163 | Actor state, command routing and every active role transition |
| bluetooth/memory/src/dtm_storage.rs | 3,283 | Raw allocation codecs, address binding and the complete DTM graph lifecycle |
| bluetooth/hci/src/dtm_order.rs | 2,995 | Common command epoch plus DTM, advertising and scanning order |
| chips/.../src/dtm_event_prepare.rs | 2,283 | Descriptor projection, scheduler preparation and completion |
| chips/.../legacy_advertising_active.rs | 2,054 | Active state vocabulary and runner logic |
| chips/.../src/dtm_active.rs | 2,025 | Active state vocabulary and runner logic |

dtm_order.rs and several codec files contain large test modules, so their
production size is less severe than the raw count suggests. By contrast,
scheduler.rs, controller_start.rs, and the Embassy command actor are mostly
production integration code and are the highest-risk monoliths.

## Findings

### Scheduler role duplication is the primary structural problem

Advertising, passive scan, peripheral connection and DTM each repeat the same
outer lifecycle:

1. reserve a timeline window;
2. join the exclusive empty software list;
3. publish the hardware head and RUN;
4. drain the fenced finished-list observation;
5. observe head retirement;
6. unlink the software list and pass the post-unlink gate;
7. recycle role-specific memory and release the reservation.

The role-specific memory and status rules are real and must remain distinct.
The list epoch, drain, head-retirement and removal-gate mechanics are common.
Continuing to copy the complete pipeline will create semantic drift. The
connection path already exposes this risk because it reached unlink before its
role-specific recycle/recurrence path existed.

Decision: split scheduler.rs into a small facade plus scheduler/epoch.rs,
scheduler/dtm.rs, scheduler/legacy_advertising.rs, scheduler/passive_scan.rs,
and scheduler/peripheral_connection.rs. Factor only the private epoch
mechanics. Do not erase role-specific typestates behind a broad public trait.

### controller_start.rs crossed from composition into operation

Startup and stable-owner publication belong together. Operational scheduler
forwarding, post-unlink mailbox transactions and recycle do not belong in the
same source file. The first refactor moves that coherent 1,058-line inherent
implementation to controller_start/scheduler_service.rs without changing the
type or API. Further cuts should separate controller-time acquisition and HCI
command-order wrappers from physical startup.

### The post-unlink implementation is incorrectly DTM-branded

dtm_post_unlink.rs contains a protocol-neutral capacity-one mailbox and
adapters for advertising, scan and connection. The name hides shared ownership
infrastructure and encourages DTM-specific names in public task APIs.

Decision: replace it with scheduler_post_unlink/, containing a generic sealed
mailbox core and separate role adapters. This is a rename of the abstraction,
not a compatibility layer; obsolete DTM-generic names should be removed in the
same change.

### Memory files are large but have a valid lower-layer purpose

The memory crate correctly contains volatile SRAM access and private bitfield
codecs. Those operations must not move upward merely to reduce file size.
dtm_storage.rs should instead be divided internally into allocation/layout,
address binding, publication lifecycle, and completion/recycle. Higher layers
must continue to see typed owners and semantic accessors only.

### dtm_order.rs is no longer a DTM-only module

It now owns the combined command epoch and ordering for DTM, legacy advertising
and legacy scanning. The portable design is correct, but the module name and
file boundary are stale.

Decision: keep one affine command epoch, then place role routing in
controller_order/{dtm,legacy_advertising,legacy_scanning}.rs. Do not create a
second HCI implementation; bt-hci remains the packet/transport API and this
crate retains only controller policy and ownership ordering.

### The Embassy actor should remain singular but not monolithic

One actor must continue to own the radio lifecycle. Splitting it into multiple
tasks would introduce arbitration and cancellation races. Its state vocabulary
and transition handlers can be separated by role while the top-level run loop
remains the sole dispatcher.

### Connection recycle must preserve the live link state

The completed connection owns two kinds of SRAM with different lifetimes.
The selected scheduler item and two-node RX rotation are event-local and must
be restored after unlink. Packet sequence/history, negotiated identity and
future encryption/control state are connection-long and must survive into the
next event.

The lower memory suffix now reflects that boundary: it validates the exact
software-list removal proof, copies RX data before mutation, and returns an
active CPU owner after restoring only the detached item and RX pool. It does
not call the cold-allocation reset. The scheduler suffix now joins that result
to exact timeline release and common-list reclamation. The portable event was
moved to `InFlight` at RUN and intentionally remains unadvanced after recycle:
the separate connection-destroy/status policy must be closed before one LL
completion can be committed.

The connection-memory file is also the first codec split: raw SRAM storage,
offsets, masks, address binding and word transforms live in the private
`peripheral_connection_memory/codec.rs`; the parent module contains semantic
values and affine lifecycle states. This is the template for the other memory
monoliths.

## Refactor order

1. Extract operational scheduler service from controller_start.rs (done in
   the audit change).
2. Split scheduler.rs by role and isolate the exclusive-list epoch
   (in progress: connection states and transitions are isolated).
3. Generalize and rename the post-unlink mailbox without legacy aliases.
4. Finish connection recycle and recurrence against those shared primitives
   (SRAM/RX plus scheduler release done; status/destroy classification, LL
   commit and recurrence remain).
5. Split the Embassy actor by role while retaining one task and one state slot.
6. Split the portable command-order and memory-codec files after the hardware
   lifecycle is complete.

Before step 5, remove the boot-chain inversion as one atomic refactor:
`Scheduler -> LowPower -> PHY -> Baseband -> BLE PHY -> interrupt publication`
must be HCI-free, followed by the single final HCI bind. This is a hierarchy
repair rather than a new feature and must not leave aliases for the old
HCI-carrying boot typestates.

Each step must preserve focused host tests, target checks and the source-only
audit. File moves must not add tests that merely restate generated masks,
addresses or symbol names.

## Driver readiness impact

The refactor does not replace missing functionality. The next functional edge
is connection completion classification, exactly one accepted LL event advance,
then provisional skipped-event recurrence and controller-runner integration.
The split is worthwhile before that work because recurrence would otherwise
add another copy of the largest duplicated pipeline.
