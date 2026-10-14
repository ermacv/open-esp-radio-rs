# Radio port vocabulary

`oer-radio-port` declares what the three radio blocks' ports share. Each
block owns its port's lifecycle (the owner, the body, the backend and its
joined driver): `oer-ieee80211-port`, `oer-ieee802154-port` and
`oer-bluetooth-port`. Code a second block needs moves back here under the
two-block rule.

| Value | Meaning |
| --- | --- |
| `branded`, `Mint`, `PoolSet`, `Slots`, `Permits`, `Slot`, `Permit`, `PermitPool`, `Seat`, `Line` | Capacity as branded tokens. `branded` runs an install with the `Mint` of a fresh brand `'id`, generic over it, so nothing branded leaves the install or reaches another; `Mint::into_pools(self)` consumes its authority and issues the complete layout declared by the backend's `Pools` associated type. `Slots<K, N>` supplies synchronous slots; `Permits<K, N, S>` supplies awaited permits, their line and caller seats. Custom layouts consume one `PoolBuilder` to expand their components. Neither authority can issue a second set; no token is `Clone`, `Copy` or `Default` |
| `PortResult`, `Poisoned` | Every port call returns `Result<Result<T, Refusal>, Poisoned<Fault>>`. The inner `Err` is a typed refusal (nothing changed); the outer one is only a poisoned backend, carrying the backend's cause, which portable code passes on without interpreting it. `next_event` returns it after every earlier event and again at every later call |
| `EventsLost` | Events were dropped. Reported once, in place of the first dropped event; events before it precede the gap and events after it follow it. A backend reserves at admission the slot of every terminal event and of data its protocol promises to deliver (acknowledged Bluetooth LE connection data), so the gap holds only data the protocol does not promise (received frames, advertising reports) and the consumer continues |
| `ClockInfo<D>`, `ClockRelation<A, B>`, `RelationRate`, `RelationSample<A, B>`, `Generation<A, B>`, `GenerationSource<A, B>`, `Stamp<A, B>`, `Projected<T, S>`, `ProjectionError` | One relation between two clock domains, typed by its source `A` and target `B`: identical, or affine within a drift bound. A `RelationSample` pairs one reading of each clock with its uncertainty (a target span) and its generation; `project`, `project_back` and `project_span` are the one projection both ways, returning the value and an uncertainty that adds the drift over the distance from the sample, as a span of the target domain. A coordinate of another generation is `StaleSample`, a result outside the range `OutOfRange`. A generation has no public constructor: the relation owner's `GenerationSource` opens an epoch and jumps at each break. `ClockInfo<D>` states a port's resolution and its relation to monotonic time (`monotonic`, `None` when none exists); `RadioStamp<D>` and `ClockSample<D>` are the radio-to-monotonic `Stamp` and `RelationSample` |

## Conformance

The `conformance` module (the `test-support` feature) holds the rule every
permit pool meets, `permits_serve_takers_in_arrival_order`: its pool's
owner (a backend's held part, a runtime) calls it with the pool, three
seats of its line and at most eight permits it holds aside.
