# Radio port vocabulary

`oer-radio-port` declares what the three radio ports share: the IEEE 802.11
lower-MAC port (`Ieee80211LowerMacPort`), the IEEE 802.15.4 radio port
(`Ieee802154RadioPort`) and the Bluetooth LE radio port (`LeRadioPort`).
Each extends the base trait `RadioPort` with its own submission, event view
and capabilities, and takes the shared values from here, so that a caller
handles every protocol's refusals, losses, poisoning and lifecycle alike.

| Value | Meaning |
| --- | --- |
| `RadioPort` | The base trait: `type Event`, `next_event`, `now`, `cancel(id)` and `lifecycle`, with the port's identity type (`Id`), clock domain (`Domain`) and poison cause (`Fault`). The calls are asynchronous: a backend that decides at once returns a ready future |
| `RadioPortOwner`, `PhaseParts`, `Lent`, `RadioPortEvents`, `RadioPortClient` | The one authority over an installed port: neither `Clone` nor `Copy`, it takes on `&mut`, between running phases, what conflicts with work in flight (the lifecycle, a permanent channel move), consumes the events and ends any admitted work. `split(&mut self)` lends one running phase as `PhaseParts<'_, …>`, each part a `Lent` of that borrow (invariant in it, with no way to give the part up), so no implementation hands out parts that outlive the phase: the phase's one event consumer, one phase control for operations that only gate or tune admission (a transmit gate, beacon priority, modem-sleep membership), with one holder that other policies send requests to, whose operations never refuse as busy (stopping admission takes effect at once and resolves to a typed proof of a quiet air that only the backend constructs), and the protocol's client handles, each scoped to its interface and counted at the split; each kind of client names its work with its own identity type, which it chooses, so it cancels only its own work, by type; the backend keys work by client and identity, so clients of one kind (multi-PAN interfaces) may choose the same value; the port's identity of a work (`RadioPortOwner::Id`) is that pair, which the client handle forms (`port_id`), terminal events carry and the owner cancels a phase's leftover work by. While the parts live no owner call compiles; work they leave in flight ends through the owner's stream, with its bodies. Where several executor tasks need the port, the owner is an actor; no handle is shared over an internal mutex ([ownership and refusals](../../../docs/architecture.md#ownership-and-refusals)). The protocol ports move onto it in #581, and `RadioPort` then keeps no lifecycle or cancel |
| `PortResult`, `Poisoned` | Every port call returns `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one is only a poisoned backend, carrying the backend's cause, which portable code passes on without interpreting it. `next_event` returns it after every earlier event and again at every later call |
| Port value | A backend's `install` returns its port, for the one event consumer, and its control, for the composition; the control's `uninstall` consumes both, so no call reaches a backend that is not installed and no refusal says so. Shared-PHY maintenance is not a layer over `Quiesce` and never pauses a port: it runs under the radio arbiter's lease, and an operation that loses RF to it ends with its ordinary terminal event |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `LifecycleCommand`, `LifecycleEvent`, `LifecycleError` | `Enable`, `Disable` and `Quiesce`, each ending with `Enabled`, `Disabled`, `Quiesced` or `Failed { command }` (the port is as before the command); a refusal is `AlreadyInState`, `InvalidState` or `Busy` |
| `CancelError`, `ClockError` | A cancellation named no running work (`NotRunning`); the clock could not be read now (`Unavailable`), or the radio epoch has no later instant (`EpochExhausted`) |
| `Correlation`, `CorrelationIds`, `BACKEND_RESERVED` | Each port keeps its own 32-bit identity type (`TxId`, `RequestId`, `EventId`) and implements `Correlation`; the top 256 raw values are the backend's, and a caller's `CorrelationIds` allocator wraps before them |
| `ClockInfo<D>`, `ClockRelation<A, B>`, `RelationRate`, `RelationSample<A, B>`, `Generation<A, B>`, `GenerationSource<A, B>`, `Stamp<A, B>`, `Projected<T, S>`, `ProjectionError` | One relation between two clock domains, typed by its source `A` and target `B`: identical, or affine within a drift bound. A `RelationSample` pairs one reading of each clock with its uncertainty (a target span) and its generation; `project`, `project_back` and `project_span` are the one projection both ways, returning the value and an uncertainty that adds the drift over the distance from the sample, as a span of the target domain. A coordinate of another generation is `StaleSample`, a result outside the range `OutOfRange`. A generation has no public constructor: the relation owner's `GenerationSource` opens an epoch and jumps at each break. `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` |

Identities stay per port so that a completion of one port cannot be taken
for another's; the trait gives them one reserved range and one allocator.

## Event model

Every port has exactly one consumer of its events. Several users of one port
share it through a router that owns the event stream and dispatches each
event by its identity. Taking an event only dequeues it: timed work a
backend performs in software runs in the backend's own runner future, which
the composition polls for as long as the port exists.

## Conformance suite

The `conformance` module (the `test-support` feature) checks the shared
rules on any backend. A backend's test wraps it in a `Harness`: the port, how
to drive its futures beside its runner, and hooks that make its hardware or
model admit work, finish it, settle its runner and produce data the protocol
does not promise to deliver (naming the operation whose reserved terminal
carried it, where one did); `classify` sorts its events into a work
terminal, a lifecycle terminal and other events. `check_all` runs every
rule on fresh harnesses:

- every event produced before a loss comes before one `EventsLost`, an
  event produced after it follows the marker, and a reserved terminal is
  never lost;
- a cancel of admitted work is followed by that work's own terminal exactly
  once, and a later cancel is `NotRunning`;
- `Disable` with work in flight ends the work first, then `Disabled`, and
  nothing follows;
- `Quiesce` with work in flight lets it end, then `Quiesced`, nothing
  follows, and `Enable` admits work again;
- a poisoned backend reports the events from before the fault, then
  `Poisoned` at every event call and at once from every other call
  (`check_all_poisoning`, for a backend that implements `PoisoningHarness`;
  one that never poisons runs `check_all`);
- the caller's identities avoid the backend's reserved range.

A port's own rules (a refusal hands its arguments back) live beside the
port's definition and use the same harness. Backends run the suite from
their own tests; a test double that does not keep the port's contract (a
scripted model) does not.

The package has no state, never waits and depends only on `oer-time`.
