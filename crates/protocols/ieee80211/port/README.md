# IEEE 802.11 lower-MAC port lifecycle

`oer-ieee80211-port` is the lifecycle of the IEEE 802.11 lower-MAC port, which the Wi-Fi block owns
(#709): `PortBackend`, which `LowerMacBackend` (`oer-ieee80211-lower-mac`) extends; `run_port`, which runs a
`PortBody` over a just-installed backend joined with the backend's driver
and takes the backend apart after the body's uninstall; the `Owner` in its
`Disabled` and `Enabled` states, `Failed`, `PoisonedOwner` and `Ended`;
the running phase, one object under `&mut` (`PortPhase`) lent as a `Lent`
of the owner's borrow; and `ClockError`. The driver is the one owner of
the backend's hardware state, and the phase is the body's handle over
single-producer single-consumer queues to it
([owner decisions on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129037560)).
Every rule a caller controls is a type, each with a compile-fail test in
the crate documentation.

Rust still permits dropping or forgetting an owner. The type boundary
prevents constructing `Ended` without consuming the owner through its
uninstall or fail-stop path. The host model poisons unrecycled RX buffers;
normal and test builds share the same lifecycle and cancellation behavior.

`PortBackend::Terminal` is distinct from its running `Event`: disable's sink
receives command completions only. The backend recycles unsolicited RX and
preserves a coalesced loss indication for the next phase. RX buffers already
lent to the body remain charged until that body returns them.

The vocabulary the blocks share (the brand and `Mint`, slots, permits and
seats, `PortResult`, `Poisoned`, `EventsLost` and the clock relation)
is `oer-radio-port`'s.

Cancelling `run_port`, including while the driver retires, sets the abandon
latch and calls the backend's fail-stop once. Cancellation does not panic,
uninstall hardware, or invent returned owners. The lifecycle crate denies
panic/unwrap/expect, implicit arithmetic, indexing and unchecked assertions
in production; its regression tests retain explicit assertion permissions.
