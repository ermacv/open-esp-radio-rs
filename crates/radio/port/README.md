# Radio port vocabulary

`oer-radio-port` declares what the three radio ports share: the IEEE 802.11
lower-MAC port (`Ieee80211LowerMacPort`), the IEEE 802.15.4 radio port
(`Ieee802154RadioPort`) and the Bluetooth LE radio port (`LeRadioPort`).
Each port declares its own requests and events; it takes these values from
here so that a caller handles every protocol's failures, losses and
lifecycle alike.

| Value | Meaning |
| --- | --- |
| `FailureClass`, `PortError` | Every port error is `Rejected` (nothing changed; not installed, paused or disabled belong here), `Recoverable` (admitted work ended without its result) or `Poisoned` (only a reset restores the port) |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. The consumer may continue and recovers work whose terminal event may be lost by cancelling it |
| `Poisoned` | The terminal event of a poisoned port, reported after every earlier event and again at every later call that takes an event |
| `LifecycleCommand`, `LifecycleEvent`, `LifecycleError` | `Enable`, `Disable` and `Quiesce`, each ending with `Enabled`, `Disabled`, `Quiesced` or `Failed { command, class }`; a refusal is `AlreadyInState`, `InvalidState` or `Busy` |
| `CancelError` | A cancellation named no running work (`NotRunning`) |
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
