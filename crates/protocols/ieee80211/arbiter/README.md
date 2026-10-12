# IEEE 802.11 channel arbiter

`oer-ieee80211-arbiter` is the arbiter of the channel one radio's station
and access point share, the third sans-IO machine of their loop
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129596602)).

Only the arbiter changes the port's channel: it closes the transmit gate,
retunes the port and opens the gate again. The station asks it for a channel
(`Arbiter::request`) and gets its answer in the same step: a named `Lease`
of the channel from `from` until `until`, or a command to leave. Notices of
the lease (`LeaseStarted`, `LeaseEnded`, `LeaseCancelled`) name it. Beside a
running access point, the station leaves the access point's channel only in
absences after its TBTTs; a station that tells its access point it leaves
says it is ready, or the lease is cancelled at its start. The lease's
`until` is absolute, so the access point's beacons hold; the access point's
air reservation is a policy option, off by default.

`coordinator` keeps the access point's follow, search and loss policies
(`ChannelCoordinator`, `ApFollowPolicy`, `ApSearchPolicy`,
`UpstreamLossPolicy`), and `channel` the channels an access point may not
serve without radar detection.
