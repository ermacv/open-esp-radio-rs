# IEEE 802.11 channel arbiter

`oer-ieee80211-arbiter` is the arbiter of the channel one radio's station
and access point share, the third sans-IO machine of their loop
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129596602)).

Only the arbiter changes the port's channel: it closes the transmit gate,
retunes the port and opens the gate again. The station asks it for a channel
(`Arbiter::request`) and gets its answer in the same step: a named `Lease`
of the channel from `from` until `until`, or a command to leave. Notices of
the lease start (`LeaseStarted`) names it. End and cancellation return the
unique `LeasePlace` without an ID: possession proves the sole lease ended.
Absence end and cancellation likewise return the unique `AbsencePlace`. Beside a
running access point, the station leaves the access point's channel only in
absences after its TBTTs; a station that tells its access point it leaves
says it is ready, or the lease is cancelled at its start. The lease's
`until` is the moment the port is back on the access point's channel, so
its beacons hold; the station listens until the return's retune, and a
lease whose gate closes too late for two retunes and the shortest useful
listening is cancelled. The access point's air reservation is a policy
option, off by default: the gate's closing takes one last frame, a
CTS-to-self the driver sends with the Duration left
([amendments](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129677418)).

A join on another channel beside a running access point starts with the
access point's announced move there, by the `ApFollowPolicy`; the move is
never withdrawn, and a second such join within the policy's
`min_follow_interval` is refused with the moment to retry
([follow decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130615564)).

`coordinator` keeps the access point's follow, search and loss policies
(`ChannelCoordinator`, `ApFollowPolicy`, `ApSearchPolicy`,
`UpstreamLossPolicy`), and `channel` the channels an access point may not
serve without radar detection.

Composition passes the backend's `PUBLICATION_BOUND` to `Arbiter::new`.
Absences reserve one current publication plus retune time for each move;
`listen_until` and the return deadline leave this budget before the absence
ends. Once the outgoing gate has closed, the late-lease check counts only
the remaining return publication. Retry-plan length never enters this bound.

Each retune carries a private `PendingRetune`, which retains its destination
and cycle purpose. The service owns it across disable, tuning and enable;
`retuned` or `retune_refused` consumes that exact action token. A channel
value alone cannot acknowledge a retune or create an unsolicited response.

The coordinator advances its bounded channel-search cursor with checked
arithmetic; an empty search has no cursor. The crate uses the canonical
strict lint block checked by tidy.
