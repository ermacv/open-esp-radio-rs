# IEEE 802.11 channel arbiter

`oer-ieee80211-arbiter` is the arbiter of the channel one radio's station
and access point share, the third sans-IO machine of their loop
([owner decision on #563](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6129596602)).

The arbiter carries the lower-MAC `GateOpen<'id>` and `GateClosed<'id>`
directly. Both own the install's one `GatePlace`; only the backend's
retained `GateIssuer` turns a completed hardware barrier into closed
evidence. Arbiter actions transfer that place rather than abstract test
tokens or independently minted slots.

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

Admission places, answers, notices and actions are `must_use`, as is each
pending retune. Polling transfers work or a deadline to the caller; silently
discarding that result is diagnosed. Explicitly dropping an affine value
remains possible and does not complete its protocol obligation.

A join on another channel beside a running access point starts with the
access point's announced move there, by the `ApFollowPolicy`; the move is
never withdrawn, and a second such join within the policy's
`min_follow_interval` is refused with the moment to retry
([follow decision](https://github.com/ermacv/open-esp-radio-rs/issues/563#issuecomment-6130615564)).

`coordinator` keeps the access point's follow, search and loss policies
(`ChannelCoordinator`, `ApFollowPolicy`, `ApSearchPolicy`,
`UpstreamLossPolicy`), and `channel` the channels an access point may not
serve without radar detection. Coordinator facts, schedules and actions
carry `SupportedChannel<K>` from the backend's capability check. Applying
a start or move therefore needs no second channel conversion that could
discard the command after its state transition. The follow policy's
announcement count is nonzero at construction; it reaches the beacon
announcement without silently increasing a zero count to one.

Composition passes the backend's `PUBLICATION_BOUND` to `Arbiter::new`.
Absences reserve one current publication plus retune time for each move;
`listen_until` and the return deadline leave this budget before the absence
ends. Once the outgoing gate has closed, the late-lease check counts only
the remaining return publication. Retry-plan length never enters this bound.

Each retune carries a private `PendingRetune`, which retains its destination
and cycle purpose. The service owns it across disable, tuning and enable;
`retuned` or `retune_refused` consumes that exact action token. A channel
value alone cannot acknowledge a retune or create an unsolicited response.
While a gate cycle runs, the next input is its gate or retune terminal;
`poll_deadline` returns no timer. Planning a shared move consumes its deadline
and transfers the access-point schedule into the cycle. A waiting absence
is cancelled when that move becomes due. The active move retains a subsequent
coordinator retune until its gate opens; completing the first move then arms
the next one. A TBTT fact on the old channel updates beacon timing without
changing the active move's home destination. Lease and upstream-loss deadlines
resume after the cycle ends, and an elapsed deadline causes a finite transition.
Late admission is checked before the outgoing retune. Once that move is
admitted, its completion cannot charge the outbound budget again; the lease
ends on its original return deadline and restores the home channel. A leave
command deferred by an absence belongs to that absence's terminal, including
refusal and late cancellation, so returning its place also releases the command.
A refused lease move returns the next `PendingRetune` directly and enters
`Retuning` before handing it out. `None` means the port stopped. Without an
enabled phase, consumers poll `ArbiterNotice` through `poll_notice`; that
exhaustive type contains only station/AP notices and cannot take a gate or
retune token. `port_failed` covers both enable refusal and poisoning.

The station chooses search channels and requests their leases. The arbiter
places those absences around the access point's beacons and spaces dense
and sparse searches; the coordinator keeps follow and loss state without
an independent poll loop or channel cursor. `UpstreamFact::JoinFailed`
resumes the search from the original loss instant, including its loss
policy deadline. Joining does not grant a new search period. The coordinator
cannot be copied into another owner. The crate uses the canonical strict
lint block checked by tidy.
