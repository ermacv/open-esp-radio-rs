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
| `PortResult`, `Poisoned` | Every port call returns `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed, not installed and paused included); the outer one is only a poisoned backend, carrying the backend's cause, which portable code passes on without interpreting it. `next_event` returns it after every earlier event and again at every later call |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `LifecycleCommand`, `LifecycleEvent`, `LifecycleError` | `Enable`, `Disable` and `Quiesce`, each ending with `Enabled`, `Disabled`, `Quiesced` or `Failed { command }` (the port is as before the command); a refusal is `NotInstalled`, `AlreadyInState`, `InvalidState` or `Busy` |
| `CancelError`, `ClockError`, `NotInstalled` | A cancellation named no running work (`NotRunning`); the clock could not be read (`Unavailable`); the backend is not installed or is paused (`NotInstalled`, in every shared refusal and on its own for a port's other calls) |
| `Correlation`, `CorrelationIds`, `BACKEND_RESERVED` | Each port keeps its own 32-bit identity type (`TxId`, `RequestId`, `EventId`) and implements `Correlation`; the top 256 raw values are the backend's, and a caller's `CorrelationIds` allocator wraps before them |
| `ClockInfo`, `RadioEpoch`, `EpochError`, `RadioStamp`, `ClockSample` | The radio clock's resolution and whether its epoch is the image's monotonic time (`Monotonic`), advances at its rate within a drift bound (`Affine`) or has no known relation (`Unrelated`); `to_monotonic_with` and `from_monotonic_with` convert a `Monotonic` port's instants exactly, refuse an `Unrelated` one with `EpochError` and project between a `RadioStamp` (a radio instant and the generation of the clock relation it was taken in) and monotonic time from a `ClockSample` (a back-to-back reading of both clocks, its uncertainty and its generation, which a port returns on demand), returning the instant with an uncertainty that adds the drift over the distance from the sample; an `Affine` stamp of another generation than the sample is `StaleSample` |

Identities stay per port so that a completion of one port cannot be taken
for another's; the trait gives them one reserved range and one allocator.

## Event model

Every port has exactly one consumer of its events. Several users of one port
share it through a router that owns the event stream and dispatches each
event by its identity. Taking an event only dequeues it: timed work a
backend performs in software runs in the backend's own runner future, which
the composition polls for as long as the port exists.

The package has no state, never waits and depends only on `oer-time`.
