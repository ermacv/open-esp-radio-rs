# IEEE 802.15.4 radio port lifecycle

`oer-ieee802154-port` is the lifecycle of the IEEE 802.15.4 radio port, which the IEEE 802.15.4 block owns
(#709): `PortBackend`, which `Ieee802154Backend` (`oer-ieee802154`) extends; `run_port`, which runs a
`PortBody` over a just-installed backend joined with the backend's driver
and takes the backend apart after the body's uninstall; the `Owner` in its
`Disabled` and `Enabled` states, `Failed`, `PoisonedOwner` and `Ended`;
a running phase's `PhaseParts`, each a `Lent` of the owner's borrow;
`Work` and the `RadioPortEvents`/`RadioPortClient` traits; `CancelError`
and `ClockError`. The block's lifecycle enums and correlation identities
live here too. Every rule a caller controls is a type, each with a
compile-fail test in the crate documentation.

The vocabulary the blocks share (the brand and `Mint`, slots, permits and
seats, `PortResult`, `Poisoned`, `EventsLost` and the clock relation)
is `oer-radio-port`'s.
