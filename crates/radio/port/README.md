# Radio port lifecycle and vocabulary

`oer-radio-port` owns the lifecycle shared by Bluetooth LE and IEEE 802.15.4,
and the capacity, error and clock vocabulary of all three radio blocks.
The two lifecycle implementations had identical product code; each also
exposed the same failing cases: a retirement waiter received no wake, and
an untyped driver exit was accepted as a successful uninstall. Their common
contract now lives here. Wi-Fi keeps its own lifecycle until integration.

| Value | Meaning |
| --- | --- |
| `Installed`, `InstallParts`, `PortBackend`, `PortBody`, `Owner`, `Ended`, `DriverExit`, `Retired`, `Detached` | One installed backend and independent driver, a borrowing typestate owner, and affine proofs of driver completion and detachment |
| `branded`, `Mint`, `PoolSet`, `PoolPath`, `PermitSupply`, `Slots`, `Permits`, `Slot`, `Permit`, `PermitPool`, `Seat`, `Line` | Capacity as branded tokens. `branded` runs an install with the `Mint` of a fresh brand `'id`, generic over it, so nothing branded leaves the install or reaches another; `Mint::into_pools(self)` consumes its authority and issues the complete layout declared by the backend's `Pools` associated type. `Slots<K, N>` supplies synchronous slots; `Permits<K, N, S>` supplies awaited permits, their line and caller seats. Custom layouts consume one `PoolBuilder<'id, Path>` to expand their components. Each tuple derives a sealed `Child<Path, INDEX>` for each member; every slot, permit, seat and line retains that identity, including repeated kinds in nested layouts. A FIFO consumes the indivisible `PermitSupply` once, so subsets cannot form interchangeable FIFOs. Neither authority can issue a second set; no token is `Clone`, `Copy` or `Default` |
| `PortResult`, `Poisoned` | Every port call returns `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one is only a poisoned backend, carrying the backend's cause, which portable code passes on without interpreting it. `next_event` returns it after every earlier event and again at every later call |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `ClockInfo<D>`, `ClockRelation<A, B>`, `RelationRate`, `RelationSample<A, B>`, `Generation<A, B>`, `GenerationSource<A, B>`, `Stamp<A, B>`, `Projected<T, S>`, `ProjectionError` | One relation between two clock domains, typed by its source `A` and target `B`: identical, or affine within a drift bound. A `RelationSample` pairs one reading of each clock with its uncertainty (a target span) and its generation; `project`, `project_back` and `project_span` are the one projection both ways, returning the value and an uncertainty that adds the drift over the distance from the sample, as a span of the target domain. A coordinate of another generation is `StaleSample`, a result outside the range `OutOfRange`. A generation has no public constructor: the relation owner's `GenerationSource` opens an epoch and jumps at each break. `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` |

## Conformance

The `conformance` module (the `test-support` feature) holds the rule every
permit pool meets, `permits_serve_takers_in_arrival_order`: its pool's
owner (a backend's held part, a runtime) calls it with the pool, three
seats of its line and at most eight permits it holds aside.

## Component authority and cost

`Root` is the path of a standalone layout. A tuple adds one `Child` per
component at each nesting level; custom factories preserve the path they
receive. The default `Root` type parameter describes a standalone pool,
not a way to erase a tuple component's identity. A line is a copyable receipt
for its waiter domain. A FIFO's capacity comes from its consuming supply;
several supplies can deliberately use one line while their permit paths
remain distinct.

| Consumer | Authority retained | Runtime cost of the path | Construction and cancellation |
| --- | --- | --- | --- |
| Wi-Fi | Gate and issuer, TX and Block Ack tickets, buffer/key pools, station connection source and its contexts | Zero additional bytes; indices remain `u8`, the epoch remains a `u64` | The phase accepts its associated gate path; terminals retain the submitted slots and connection context |
| Bluetooth LE | The backend's declared layout; the port test backend has a standalone slot component, production layouts without pools use `()` | Empty layout and Mint remain zero sized | Existing branded install/body lifecycle; consuming issuance has no new waits or cancellation state |
| IEEE 802.15.4 | Nested interface/configuration/pending-table paths and the separate radio-operation path | Zero additional bytes; each slot/permit/seat remains one byte | Configured and pending-entry owners retain their slot; operation guards release only to their source pool |
| FIFO runtime | Independent permit and waiter paths; opaque initial supply | No registry, allocation or new synchronization; line receipt remains zero sized | The whole supply constructs one pool; cancelled takes release their queue place and keep the borrowed seat |

Contract misuses and their positive twins live in `tests/ui/` and the
consumers' corresponding directories. `cargo xtask check ui` verifies exact
compiler diagnostics on host and RV32; unit tests cover shared waiter lines,
cancellation, empty layouts and all 256 indices.

## Lifecycle and cancellation

`Installed::new(backend, driver)` is the sole constructor. `run_port` consumes
it, keeps `backend` by value, and lends `&mut B` to `Owner<'p, 'id, B, State>`.
The driver is moved to static `PortBackend::drive` once; its signature has no
backend receiver to borrow. It has no installation lifetime: branded queues,
`Slot<'id>` and in-flight ownership belong in `Held<'id>`. The named
`InstallParts<'id, B>` separates held queues, body-local accounting and the
resources delivered to the body. The single mint still consumes the whole
capacity layout once.

Enable and disable are consuming, bounded transitions. A running phase is
one invariant `Lent`; its borrowed `PhaseParts` can serve disjoint work in
the same task. Terminal capacity is reserved when work is admitted, and
disable hands all remaining terminals to its sink before success. The backend
states each transition bound. The retirement deadline future is created
before the owner loan and cannot borrow the backend (`use<Self>`); it starts
on its first poll after retirement is requested.

A driver returns `DriverExit::Retired` with a proof obtained by consuming
`Retire` after its request, or `DriverExit::Faulted` with a proof obtained by
publishing the port's `PoisonState`. The core retains a faulted driver's owners
while the body drains earlier events and requests uninstall. Uninstall takes
`Retired` and `Detached` of the same brand. There is no untyped early exit.

Retirement and abandonment register and wake their waiters. The abandon latch
has independent core and driver waiters. A dropped owner wakes the core,
which closes admission, requests retirement and detaches after the driver
ends; a missed deadline fail-stops. `Owner::fail_stop` calls `abandon`
synchronously and returns an affine `FailStopped<'id, B>` proof. `abandon`
idempotently poisons the installation without unbounded waiting. It returns
`Abandon::Stopped` only with its `HardwareStopped` proof of masked IRQ entries,
bounded CPU exclusion and device stopping. `Abandon::Unconfirmed(Quarantine)`
consumes the authority without claiming those effects; it requires chip reset.
An overlong driver step or ISR may leave hardware running: every reachable
allocation stays in permanent quarantine until chip reset. Neither outcome
authorizes DMA backing reuse. `FailStopped::outcome` exposes this distinction
without consuming the body's affine proof.
The core retires independently while the body waits; only final `detach`
(consuming that outcome) and result delivery wait for the body.
Cancellation of `run_port` repeats synchronous quarantine as a safety net.
Forgetting an owner cannot construct `Ended`; its backend remains in the core. Hardware-reachable memory must
remain owned, static storage even when a future is forgotten.

The body chooses `PortBody::Output` before its generic `run` introduces `'id`;
that output cannot contain the installation's brand. It returns
`Ended::uninstall(owner, control, output)` or
`Ended::fail_stopped(proof, output)`. `run_port` returns `PortEnd::Uninstalled`
with owners and output, `FailStopped` with output, or `Abandoned` when the
owner was dropped and the core ended the installation without a body result.
BLE's service returns `Served` through this output instead of a side channel.

## Cost and Wi-Fi integration

The RV32 measurement uses two concrete backends and the same enable,
disable, retire, detach and cancellation workload in
[`examples/lifecycle_cost.rs`](examples/lifecycle_cost.rs). With Rust 1.97.1,
`riscv32imafc-unknown-none-elf`, opt-level 3, one codegen unit, LTO and aborting
panics, the former two lifecycle implementations measured 2282 bytes of
`.text`; the shared lifecycle measures 5940 bytes (+3658). This is a small
lifecycle ELF, not total firmware size or a hardware timing result.

| Checks | Code size | RAM and CPU | Signatures | Breakage | Alternatives | Reversal |
| --- | --- | --- | --- | --- | --- | --- |
| Typestate and opaque affine call/retire/poison/detach proofs are compile-time checks. Hardware refusal, poisoning and bounded waits remain runtime outcomes; lifecycle code has no explicit panic. | Test ELF `.text`: 2282 → 5940 bytes. Backend/body futures remain concrete monomorphizations; retirement and signal machinery add code. | No allocation. The measured RV32 future grows from 184 to 416 bytes per backend (+232). Three atomic waiter slots per installation: retirement, driver abandonment and core abandonment. Wake registration uses atomic synchronization; consuming proof and call tokens are zero-sized. Hot radio operations acquire no new lifecycle lock. | `Installed<B>`, `InstallParts<'id, B>`, static consuming `drive`, `DriverExit`, `Call`, `Quarantine`/`Abandon`/`HardwareStopped`/`Detach`, `Retired<'id, State>`, `Detached<'id>`, `PortEnd<B, Out>`; owner borrows `&mut B`, `Ended<'id, B, Out>` has no borrow lifetime. | Both migrated blocks consume `Installed` and borrow one phase. Wi-Fi adopts the same core after coordinated integration; its current code is unchanged. | Keeping two identical owners duplicates fixes. Moving `B` into its owner loses the core's asynchronous abandonment authority. A receiver-based `drive` needs an empty driver slot. Flag-only retirement loses wakes. | Rework signal/future representation if the added code or installation RAM exceeds the target image budget; preserve proofs, wakes and asynchronous teardown. Re-measure this ELF and the affected firmware before accepting that rework. |

Wi-Fi's migration updates the installation factory to return
`Installed::new(backend, driver)`, moves its driver out of the backend, and
implements static `drive` returning the two proved exits. Its branded queues
stay in `Held<'id>`; `resources` returns `InstallParts` instead of a tuple.
Backend calls take the core's affine `Call`; uninstall consumes `Retired` and
`Detached`. `retire_bound` returns a future without a backend borrow.
`abandon` immediately quarantines and returns a proved `Stopped` or an
`Unconfirmed` outcome; final `detach` follows the body's return and consumes
that outcome. Unconfirmed hardware requires chip reset and permanent backing
quarantine, not a runtime assertion that it stopped. `fail_stop`
returns `FailStopped`, which `Ended::fail_stopped(proof, output)` consumes.
`run_port` returns `PortEnd`; `StationService` supplies its result as
`PortBody::Output`. The generic phase, separate event/terminal types,
consuming owner transitions, `Failed::{Refused, Poisoned}` and `PoisonedOwner`
remain available. Integration must precede dependent Wi-Fi and S31 work
being rebased onto this contract.
