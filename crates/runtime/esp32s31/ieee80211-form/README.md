# ESP32-S31 Wi-Fi execution form

`oer-esp32s31-ieee80211-form` is the runtime-layer ownership prototype for a
separate S31 driver. It depends on hardware owners and portable contracts;
it has no dependency on esp-hal or a composition. The adapter supplies the
routing authority and context-free ISR entry. The adapter-to-form dependency
is provisional until review of the form.

The portable entry consumes `Installed<B>`, with S31 Driver as `B::Driver`,
and branded state belongs to `Held<'id>`. The lifecycle core owns Driver and
lends it to the driving future. The linked fixture retains `RadioRuntimeOwner`
by value in permanent backing.
The portable entry is an integration boundary; it cannot add a second IRQ
registry or change the backing's reclamation proof.

`scope::driver::Driver` is the unbranded installation owner. Its `drive`
method borrows it exclusively until retirement; that future owns neither
Driver nor the backing lease. The executable binding in
[`scope/tests/lifecycle.rs`](src/scope/tests/lifecycle.rs) uses the real
shared `Installed`, `run_port`, `PortEnd`, `FailStopped` and `Abandon` types.
Its frontend models lifecycle only; it does not migrate the old S31 runtime.
The independent linked hardware fixture retains `RadioRuntimeOwner` by value.
The tests retain a non-static context and an affine installation slot in
`Held<'id>`, and never register an address into Held with either executor.

After the driving loan ends, consuming `Driver::finish` returns
`State::Released` or `State::Quarantined` explicitly. The latter retains
`ScopeReturn` in `Quarantined<S>` and has no owner or lease extraction. On
`Abandon::Unconfirmed`, the core must instead retain Driver itself without
calling `finish` or its destructor. The synchronous backend abandon path has
already masked the inputs and closed admission. The entered ISR or step can
finish against permanent backing even if the driving future and the outer
container holding the quarantined Driver are destroyed. There is no registered
address into Driver, its future, Held or the result latch.

The shared `DriverLoan` binding is pending publication. The current lifecycle
test binding owns Driver under the published trait, awaits its borrowed
`drive`, then consumes `finish`. Separate regressions reject consuming Driver
during that borrow and exercise explicit abandon followed by quarantine with
no Driver destructor. An owned Driver's Drop remains a synchronous cancellation
safety net; the core's Unconfirmed path must not rely on it. Unlike ordinary
Released return, the common core's abandonment policy retains even a
successfully stopped installation's backing until reset.

Driver and returned state share one temporary `Quarantined<S>` holder with
`ManuallyDrop<S>` contents. Its drop releases nothing. The DriverLoan
integration must remove this declaration and use the shared core's
`Quarantined<S>` directly, without an alias or an S31 wrapper.

Every await in `Driver::drive` preserves a state accepted by retirement,
abandon and consuming finish. Cancellation only ends the loan: it cannot
take a descriptor, queue or hardware owner out of backing. The two suspension
points wait for the core's retirement request and the executor's retirement
completion. A parameterized host test uses the real Retire from run_port,
cancels before polling and after 1–3 polls at each await, then exercises finish,
retirement and abandon separately. Its bounded DMA ownership model verifies
that each published address retains its exact queued buffer, successful
teardown clears every descriptor and queue, and each buffer returns once.

## Ownership and return

```text
composition: one permanent SRAM backing and its ISR address slot
  ScopeStorage
    ├─ Header: exact callbacks, separate driver and IRQ addresses
    ├─ DriverRegion: by-value hardware and exclusive IRQ consumers
    ├─ IrqRegion: owned IRQ banks, route, scheduling and atomic signals
    └─ IrqStorage / PowerIrqStorage: facts, wakers, optional diagnostics

caller: DriverScope::run future
    ├─ affine pinned lease of that backing
    ├─ borrowed AbandonLatch, never reachable from registered callbacks
    └─ arbitrary Context, body future and its retained result

common-core binding: Driver owns the same scope lease and latch loan
    driving future ── exclusive loan ──▶ Driver
    Unconfirmed ──▶ quarantined Driver, no Drop and no backing recovery

ScopeSlot ── admission counter + atomic address ──▶ Header
    driver executor ──▶ DriverRegion
    MAC / POWER ISR ──▶ IrqRegion (contains no driver address)
```

The composition declares the backing in SRAM. At boot it transfers its unique
`&'static mut ScopeStorage` through `pin_static`, following the permanent-pin
pattern used for BLE. There is no allocator, shared `take` method or runtime
"already taken" flag. The lease is neither `Clone` nor `Copy` and never exposes
a mutable reference to its backing. `Released` returns that same lease for a
subsequent installation. `Poisoned` never returns it; only chip reset permits
another installation. The scope owns the lease, not the allocation. In the
common-core binding that lease remains in Driver while its driving future
only borrows Driver.

This changes the original §3.1 inline-storage proposal. A bounded timeout in
`Drop` cannot make inline storage safe: dropping `Pin<Box<Future>>` would free
the registration while an entered actor still uses it. Permanent backing
survives owning-future drop, `forget`, and timeout. The reclamation regression
holds an actor on a second core, drops the owning pinned future after timeout,
then lets the actor finish against its still-live backing.
The separate forget regression pauses MAC ISR, POWER ISR or a driver step,
forgets the owning future, then resumes that actor. Registration identity and
queued buffer owners remain unchanged, no lease returns, and body, hardware,
IRQ and buffer drop counters all stay zero.

The body runs on the caller's executor. Scheduling only pends a separate
driver step. A normal body return requests retirement and retains the result
until the driver accounts for its accepted work. Detachment masks MAC, POWER
and driver-SWI, closes admission, waits for entered actors, and calls the
synchronous `stop(&mut Driver)` exactly once **only after successful drain**.
The successful output contains a returned lease and either retired owners or
opaque quarantined owners after cancellation. Quarantined owners expose no
restart, clock-release or extraction operation. IRQ `quiesce` consumes its
epoch once; there is no `AlreadyQuiesced` state or refusal.

Explicit fail-stop consumes `ScopeAccess` and completes that synchronous
mask/close/wait path before returning to a possibly still-pending body. Its
reply is retained for the later destructor, so `stop` is not called twice.
Cancellation of the entire run future uses the same path from `DriverScope`.
An empty latch means no output was delivered, for example because an owning
future was forgotten; it does not claim successful release or poisoning.
Context needs no `'static` bound and is unreachable by the registration.
Forgetting a borrowed `Pin<&mut Future>` leaves the future's own destructor
responsible for cancellation. No lifetime is extended or transmuted.

## DMA custody and clock release

`StableDmaBacking` requires both the allocation and its exclusive reservation
to survive a leaked lease. A borrowed stack buffer cannot satisfy that unsafe
contract. The pinned TX pool consumes permanent storage; its leaked radio
lease leaves the slot reserved. The tagged and returning wrappers preserve
this guarantee while allowing shorter-lived CPU-only metadata. Retained
A-MPDU storage keeps its backing owners in `ManuallyDrop`, so forgetting the
DMA owner and then destroying its arena cannot free those allocations. Host
regressions read the retained bytes after the metadata and arena end.

Ordinary retirement recovers the by-value radio owner and inactive IRQ setup.
The resulting stopped installation may consume its Wi-Fi clock obligation
through `WifiClocked::release_clocks`, whose exact result is
`Result<ClockReleased, (StoppedInstallation, ClockPoisoned)>`. Success moves
on to common-power release once. Physical failure retains both stopped
register custody and the unresolved obligation; there is no retry method or
replacement `WifiClocksOn`. Host/RV32 UI pairs reject repeated release and
release through poison. The shared modem planner's recovery remains #757.

This clock failure follows an already stopped installation. Scope timeout
instead returns `ScopePoisoned`, without a stopped owner or access to any
clock-release operation: entered actors or hardware may still be running.

## Command ownership, stages and ordering

`scope::commands::CommandStorage` owns two SPSC stores in Held. Its unique
construction authority retains the same `T` and `K` as `CommandPools<T, K>`;
an authority for larger pools cannot initialize a smaller store. The only
producer methods consume their actual affine command admissions. There is
no `Full`, `try_send`, second producer, or generic public queue insertion.

| Store | Capacity | Admissions retained by commands |
| --- | --- | --- |
| Setup | `2 + 1 + 1` | STA/AP configure, retune, Enable |
| Phase | `T + 2*K + 2 + 2*2 + 2 + 1` | TX tickets, pairwise install/remove, GTK mutation, STA/AP configure and TSF, gate, Disable |

The phase formula is the lower-MAC command bound plus the distinct lifecycle
Disable place. Each interface contributes **both** its `ConfigurePermission`
and `TsfPermission`; configure is a FIFO command carrying its exact receive
value and opaque context. `Configuration::configured` and `Sampling::sampled`
construct the actual `Configured` and `TsfSampled` terminals, returning those
same permissions. Configure is never placed in a coalescing settings cell.

Enable is the setup sender's final publication and consumes that sender into
`PhaseSender`; the receiver changes only when it consumes Enable. Disable
does the reverse and leaves a `DisablingSender` with no enqueue methods.
The Disabled reply retains both lifecycle places until it is consumed to
recover SetupSender. S31's enable operation is synchronous within a bounded
hardware step, so there is no hardware `Enabling` owner containing invented
pending data. Waiting for that step's reply still belongs to the common core's
consuming enable transition.

Closing the gate transfers its owner into the FIFO. Only a dequeued
`ClosingGate` can make the barrier reply. `open_gate` first publishes its
internal OpenGate command and only then returns GateOpen. At most that Open
and the next Close can coexist; no external caller can forge a barrier reply
from an Open owner and exceed those two places. FIFO order puts the Open
before every subsequent Submit, and Close after every earlier Submit.

Submission contains the complete lower-MAC `InFlight<C, Ticket, M>` and the
actual S31 `TxAdmission` (`KeyUse` in its protected variant). Queue movement,
execution projection and terminal preserve context, ticket and whole MPDU;
there is no separately indexed body collection. The full-capacity regression
fills all 15 places for `T=2, K=2` before receiving, including two accepted
MPDUs with two independent bodies each. It verifies original context and
body addresses, SN and PN through the public Attempt/InFlight/Completed API.

Coalescing settings and BA state require one SPSC triple buffer per setting
kind: producer-private back, consumer-private front, and an atomically
exchanged middle. Publishing the latest complete value uses Release before
publishing the next FIFO command, then wakes the driver. The receiver must
**Acquire the command first**, acquire each changed settings cell, and apply
the resulting values before executing that command. Reading cells before
the FIFO Acquire would leave a race even if each cell were individually
atomic. The current `InterfacePhase::{add,replace,remove}_block_ack` contract
also specifies desired state, not commands or asynchronous replies. Each BA
slot therefore has its own triple buffer; its affine agreement retains the
slot while installed in the caller's desired state. Removal returns the slot
immediately, and reuse before the driver runs replaces that pending removal
with the new desired agreement. No intermediate hardware installation is
promised. BA adds no FIFO capacity term. An operation with an individual
reply would instead need its own affine request credit and a FIFO command;
the current BA API has no such operation. This ordering is the required S31
binding, not a claim that the old runtime already satisfies it.

The private queue storage is part of the same scope bridge primitive. Its
producer Release publishes a fully initialized value, the receiver Acquire
moves it once, and receiver Release frees its cell before any command credit
can return. Typed credits bound occupancy by the const capacity, including
stage transitions and the internal OpenGate. Dropping an unregistered store
destroys at most its fixed capacity. Registered callbacks never contain its
address: a timed-out external step cannot keep a reference into Held after
the common core frees its own future.

## Shared lifecycle boundary

The published shared contract gives `phase` and `disable` both `&Held<'id>`
and `&mut Local<'id>`. Its `enable(&mut B, Call)` has neither; `setup` receives
Local only. A backend constructed before the fresh `'id` cannot contain a
branded FIFO endpoint or the sender's stage owner. Consequently the current
lifecycle witness can exercise Driver ownership and cancellation, but a
frontend using the two real command FIFOs needs access to these parts at
enable/setup as well.

The proposed shared change is to pass `&Held<'id>` and `&mut Local<'id>` to
enable, and add `&Held<'id>` to setup. The common Owner already retains both
parts; it would lend them in exactly the same way as phase and disable.
The shared-contract owner implements this boundary; this package does not
change radio/port. An erased pointer into a movable
Held value, a backend registry, or a third untyped lifecycle FIFO is not part
of this form.

| Checks | Code size | RAM and CPU | Signatures | Breakage | Alternatives | Reversal |
| --- | --- | --- | --- | --- | --- | --- |
| Existing brand and exclusive Local loan identify the installation and sender stage; no runtime ownership test | Not measured until the shared owner implements the change; remeasure the existing linked lifecycle fixture | Pass existing references; no new allocation or queue. Two RV32 reference arguments for enable, one for setup; effect on concrete futures still requires measurement | `enable(&mut B, Call, &Held<'id>, &mut Local<'id>)`, `setup(&mut B, Call, &Held<'id>, &mut Local<'id>)` | Both shared lifecycle implementations and their test models must adopt the signature; Wi-Fi uses the added loans | Placing branded endpoints in pre-brand B is impossible; indirect registration would weaken the current ownership boundary | Revisit the ownership split if measured cost is excessive, retaining typed FIFO stage and capacity proofs |

## Quiescence, poison and priorities

Each ISR or driver entrant increments the shared counter **before** checking
admission. Closing the gate and observing the counter use sequentially
consistent atomics: an entrant not seen by the zero observation must see the
closed gate and cannot project the registration. Detach masks all hardware
routes synchronously, closes admission and performs a fixed positive number
of observations. It acknowledges driver-SWI without taking the IRQ-bank lock,
so a stuck actor holding that lock cannot block the timeout path itself.

Successful drain excludes every prior actor on every bound core. Only then
may the scope borrow the driver for `stop`, detach register owners, destroy
its private handoff borrows and return the lease. A late rejected entrant
accesses only the permanent address slot. Lease reuse initializes fresh facts,
wakers and diagnostics before opening the next installation.

`Poisoned` means **masked inputs, closed admission, permanent DMA and owner
quarantine**. Hardware may still be running. It is not `HardwareStopped`,
and its type exposes no stopped owner, lease or clock-release operation.
Timeout performs no driver borrow and calls no `stop`. Later actor completion
cannot turn poison into release. The composition must report the fault and
arrange a chip reset; it must not attempt reinstallation or clock release.

Driver steps process a finite IRQ batch and bounded command work, returning
`WakePlan::Ready` or specific sources and a future deadline. A step timeout
therefore indicates a hang or defect, not ordinary backpressure. Known causes
to investigate are a non-returning MMIO/device operation, a step or scheduler
violating its finite-work contract, a stopped core, and an IRQ-bank critical
section whose owner never exits. A caller preempting an actor can also exhaust
a deliberately short observation bound; this conservatively poisons the
installation without making that priority order unsafe.

Recommended priorities for latency are hardware ISR above driver-SWI above
caller. This is a performance recommendation, **not a safety condition**.
The same admission/quarantine proof covers the opposite order and execution
on another core. Unit tests control both same-core preemption orders and
cross-core overlap, detachment during either actor, successful drain and
cancellation during an ISR. Composition still serializes driver steps across
cores, and its cross-core critical section serializes only the disjoint IRQ
register bank, never a borrow of Driver.

An independent emergency MAC stop or reset remains an open HIL question.
If a hung step can leave the transmitter active, composition will need the
BLE `EmergencyStop` pattern: a separate owner of only its own registers,
without `SharedRadioLease`. This prototype does not claim that masking stops
RF transmission and does not invent a reset sequence without stand evidence.

## IRQ handoff and wake boundary

`capture_mac` and `capture_power` finish all register, fact and diagnostic work
before returning private, lifetime-free publication values. Those values own
only snapshots and `IrqPublication` with its `Waker`. They contain no driver,
slot, counter, backing or registration reference. Admission guards and IRQ
borrows end before `wake(self)`. The ISR cannot access installation memory
after wake by the structure of this tail, not by convention about line order.
Driver completion follows the same rule. Composition wakers, including their
clone and drop callbacks, may only schedule: they must not synchronously poll
or detach. Tests assert that both wake tails run after the admission count
returns to zero.

`IrqStorage::split(&mut self)` lends one affine consumer. Every wait holds its
exclusive borrow until completion or cancellation. MAC has one finite batch
path: RX success precedes TX complete, timeout and collision. New publications
remain for another step, so continuous RX cannot enlarge the captured batch.
Power has its own exclusive consumer. Register-before-check polling closes
publication races; cancelling one wait cannot steal the next wait's wake.
Moderation is `ActiveEpoch<Moderated>`'s type, and consuming `quiesce` returns
`InactiveEpoch`. No global register bank or fallback moderation callback exists.

`IrqDiagnostics<'_>` borrows its installation's counters. Without
`irq-counters` neither that diagnostic type nor its counter accessors exists.
`mac_irq_cycles` additionally requires RV32 cycle measurement and
`task-poll-telemetry`. It includes status, facts and counters,
**excluding external waker invocation**. `CORE0_PERFORMANCE` and
`CORE0_RX_CYCLES` have no hard-IRQ fields.

The driver's AtomicWaker points only to the permanent ScopeSlot, never to
Driver, Held or a future. A wake enters the slot's admission domain and pends
the current scheduler; after close it cannot inspect an old registration.
Both MAC/power IRQs and software capacity publications use this same path.
Each dispatch registers before its Acquire snapshots. After arming a Wait
plan it checks remaining facts and retire/abandon again, making new work Ready
for the next finite step. A scheduler must not clear an already pending SWI
when arming Wait; the executor acknowledges the SWI before dispatch instead.
Tests inject publication inside the wait-arm boundary and retain a cloned
waker across detachment to exercise its wake-by-ref, wake and drop paths.

## The single unsafe primitive

`scope.rs` and its private bridge storage own the permanent backing lease,
command ownership transfer and all registration projections.
Its unsafe binding constructor requires unique routing authority, synchronous
masking on every bound core, an exclusive driver executor, cross-core IRQ-bank
serialization, and bounded non-reentrant callbacks. Retained hardware is
`Send`; shared scheduler/platform data is `Sync`. Their reachable memory must
survive forget and pass the SRAM closure audit. Context and the result latch
are excluded from this graph. The exact callback and address pair is installed
together, so private casts recover only the correct region type. IRQ region
types contain no `DriverRegion` or hardware pointer. The admission proof covers
all such projections; there is no second unsafe global-storage mechanism.
See [UNSAFE.md](../../../UNSAFE.md).

Host and RV32 UI twins reject duplicate waits, setup activation or quiesce,
publication after quiesce, partial register ownership, route reuse, stack
backing, repeated lease transfer, and recovery of backing or stopped hardware
from `Poisoned`. Cross-core execution and reversed priorities are permitted
and tested behaviorally.
Additional twins reject setup Submit, phase Retune, any enqueue after Disable,
duplicate configure/TSF permissions, forged gate replies, mismatched mint
sizes, undersized storage, reused Driver, consuming recovery during the driving
loan, and extraction from quarantine. The no-Drop quarantine regression frees
the container holding Driver while an entered MAC ISR, POWER ISR or driver
step is paused on another core, then verifies that each actor resumes against
the permanent backing without owner destruction or a lease return.

## Observation: one MPDU across several TX-DMA descriptors

The inspected code establishes descriptor chaining, but does **not yet prove**
that one S31 MPDU may span a header buffer and several separate SRAM body
buffers, or that hardware CCMP continues across those descriptor boundaries.
These are open HIL questions; this form adds no such hardware implementation.

The [DMA descriptor](../../../hardware/esp32s31/driver/ieee80211/dma/src/descriptor.rs#L20)
has a next-node address, and
[external A-MPDU publication](../../../hardware/esp32s31/driver/ieee80211/dma/src/tx_ampdu_storage.rs#L1088)
links retained allocations. However, the existing
[A-MPDU commit path](../../../hardware/esp32s31/driver/ieee80211/mac/src/tx/ampdu/commit.rs#L37)
places an eight-byte metadata prefix, an entire encoded MPDU and its MIC/FCS
space in each contiguous allocation. Its existing `SOURCE` cites complete
`libnet80211[ieee80211_output.o]::ieee80211_alloc_tx_buf` (cache-TX/type-nine)
and `libpp[pp.o]::ppAssembleAMPDU`: retaining and linking those ESF buffers
proves the aggregate's no-copy ownership path, not a descriptor continuation
inside one MPDU. The
[length calculation's sources](../../../hardware/esp32s31/driver/ieee80211/mac/src/tx/ampdu/length.rs#L203)
likewise account for complete PSDUs. Ordinary TX currently
[publishes one descriptor with a null next pointer](../../../hardware/esp32s31/driver/ieee80211/dma/src/tx_storage.rs#L232).

[Ordinary TX](../../../hardware/esp32s31/driver/ieee80211/src/ordinary_tx.rs#L1513)
reserves hardware MIC space and programs the hardware key selector; A-MPDU
metadata also includes per-MPDU hardware MIC length. These observations do
not establish the CCMP engine's handling of a split header/body stream.
HIL must distinguish a continuation of one MPDU from the next A-MPDU subframe,
verify the plaintext case, then CCMP decryption/MIC, one PN and one FCS across
multiple body buffers. Descriptor flags, alignment and supported split points
also remain to be established from complete vendor code or stand evidence.

The ownership form therefore keeps an opaque whole MPDU `M` inside `InFlight`.
`M` may own a header and several bodies with separate retained SRAM leases.
Admission, retry, terminal and quarantine move or retain that whole owner;
there is no single-slice or one-descriptor restriction and no reconstruction
from an independent body table. This leaves the hardware question open without
constraining the eventual memory shape.

## Linked placement and cost

`tests/placement.rs` links the actual HAL, MAC and scope code in
`examples/scope_placement.rs`. This fixture is never flashed or used as HIL
evidence. SRAM and cached ranges come from `platform/esp32s31/chip.toml`;
`tests/placement.x` states the production linker section provenance. Header,
driver and shared IRQ memory live in the permanent backing section. The entry
stub separately establishes an SRAM stack for the executor and ISR.

The ELF audit follows the transitive call graph, every retained waker vtable
entry and the private header callback families resolved through DWARF. It
rejects unresolved calls and walks relocations through callback data and
vtables, including the full containing object when LLVM merges globals.
It checks the whole backing symbol as well as stack addresses. Negative linked
twins move code, read-only data, mutable data, backing or stack to cached
memory; each must fail. Ordinary and diagnostic feature variants pass the same
audit. Composition must apply this audit to its actual executor binding.

Measured RV32 release fixture costs: the body keeps both command stores live,
configures an interface and performs Enable/Disable. The command layout uses
two TX tickets and two pairwise slots, a one-word context, and three words for
an MPDU's header and two body owners. This measures the form, not a hardware
scatter/gather implementation or the complete lower-MAC backend.

| Checks | Code size | RAM and CPU | Signatures | Breakage | Alternatives | Reversal |
| --- | --- | --- | --- | --- | --- | --- |
| Affine lease, IRQ consumers, epoch and FIFO credits at compile time; finite atomic admission drain and IRQ-bank transactions at run time | 5398 B executable sections; 5662 B with diagnostics; one monomorphization per scheduler binding | Fixed permanent backing 124 B per installation, 156 B with diagnostics; 12 B ISR slot; total writable statics 141 / 173 B; command stores 528 B; fixture caller frame 800 B in both variants, separate 4096 B stack; backing size is independent of the body's future size | `ScopeStorage`, `ScopeLease`, `DriverScope`, `Released` / `Poisoned`, borrowed `IrqDiagnostics`; no `Context: 'static` | Consumers must own their IRQ handle; global snapshots lose IRQ fields; poison cannot yield hardware or a lease | Inline future storage cannot survive a timeout followed by free; priority ordering cannot prove cross-core quiescence | Reject bindings whose linked graph leaves SRAM, whose callbacks remain unresolved, or whose fixed backing and stack exceed composition's measured budget |

Run `cargo test -p oer-esp32s31-ieee80211-form` and `cargo xtask check ui`.
Generated ELF files and closure reports remain under ignored
`target/agent-a/form-placement/`.
