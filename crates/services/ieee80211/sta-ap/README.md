# oer-ieee80211-sta-ap-service

A Wi-Fi station and access point on one lower-MAC port, without an
executor.

`PortStaAp` owns a port that two clients share: the station
(`oer-ieee80211-sta-service`'s `PortStation`) on one interface, the access
point (`oer-ieee80211-ap-service`'s `PortAccessPoint`) on the other, and the
channel coordinator (`oer-ieee80211-ap::coordinator::ChannelCoordinator`)
that decides the port's one channel. The station's upstream BSS decides it,
and the access point follows within its `ApFollowPolicy`.

The search mode is a type parameter, selected when constructing the pair:

| Mode | Construction | Backend requirements | Search policy |
| --- | --- | --- | --- |
| `CurrentChannel` (the default type) | `PortStaAp::new(station, ap, follow_policy, upstream_loss, timer)` | The clients' existing port requirements | No off-channel channel list; observe the AP's operating channel, bounded by the mandatory `UpstreamLossPolicy` |
| `ProtectedSearch` | `PortStaAp::new_with_search(station, ap, coordinator, timer)` | `LowerMacAirReservation` and `LowerMacLiveRetune` | The coordinator's explicit `ApSearchPolicy`, including its channel list |

Both modes expose `connect`, `reconnect` and `run_until`. Missing backend
extensions cannot select protected windows, and a current-channel pair
cannot silently ignore a supplied channel list: its constructor accepts no
off-channel search policy.

| Operation | What it does |
| --- | --- |
| `PortStaAp::connect` | The station joins its upstream, alone on the port, and the access point starts on the upstream's channel when the policy lets it serve it |
| `PortStaAp::reconnect` | Discovers and joins the upstream on the running access point's current channel, serving the access point and delivering its data throughout the attempt; retains the station on failure |
| `PortStaAp::run_until` | Runs both clients until sync points, completes their exchanges, follows CSA and searches passively after upstream loss; a found upstream is joined on the owner's grant |

The actions it carries out:

- **Announce a switch.** When the upstream announces a switch, the access point announces the same move to its peers in the beacons that fit before the upstream's switch instant.
- **Retune.** The port moves once for both interfaces, and both clients are told their switch is done. With the access point running, this happens at its due TBTT; with the station alone, at the upstream's switch instant.
- **Stop the access point.** When the upstream goes where the policy does not let the access point follow, its peers are deauthenticated and the BSS closes. The station follows the upstream alone.
- **Start the access point.** It starts again once the port is on a channel it may serve.
- **Leave the upstream.** This is the alternative policy: the station leaves, and the access point keeps its channel.

Permanent moves use the shared upper-MAC `PortClient::retune`. On S31 the
channel setting refuses while enabled, so this operation disables the port,
sets the channel and enables it again. The S31 channel setting breaks both
VIFs' TSF relations; an announced CSA move tolerates that change. Temporary
search windows require the separate live-retune contract below.

In `CurrentChannel` mode the station cannot search other channels while the
access point serves, so the mandatory `UpstreamLossPolicy { after, action }`
(`oer-ieee80211-ap::coordinator`) states what happens when the upstream
does not come back; there is no implicit wait without end
([owner decision](https://github.com/ermacv/open-esp-radio-rs/issues/324#issuecomment-6095469384)).
`UpstreamLossPolicy::DEFAULT` is `after` 30 s with
`UpstreamLossAction::RescanAndRestart`: `after` the loss, the access point
stops (its peers deauthenticated, its BSS closed), the station scans alone
on its profile's channels and joins the upstream, and the access point
starts on the upstream's channel. A scan that finds no upstream starts the
access point again on its previous channel and ends that `run_until` with
the event `PortStaApEvent::UpstreamNotJoined`, which carries the attempt's
error; the next attempt follows `after` later, counted from the restart. Outside this bounded action the access point runs, unless the
follow policy stops it on an unservable channel. While the upstream stays
absent, the peers re-associate once a cycle. An upstream heard again on the
access point's channel before `after` is joined passively, the access point
untouched. `ProtectedSearch` carries no loss policy: its windows search
other channels instead. HIL evidence is #410.

Neither client tunes the port while both run. A lost upstream ends
`run_until` with `PortStaApEvent::StationEnded`; subsequent calls serve the
access point and search for the upstream. The caller may retry with `reconnect`, which grants
the station only the access point's current channel. It refreshes discovery
there, including after a coordinated channel switch, and never visits the
station profile's other scan channels. A candidate that requires another
channel or width fails before authentication. `connect` refuses to scan
while the access point runs, and both join entry points refuse an already
connected station. An announced move must finish before `reconnect`.
Once the station's attempt ends, `reconnect` lets the current AP run finish
at the next TBTT, including its exchange in progress. Returning can therefore
take about another beacon interval.

The shared upper-MAC router gives exchanges FIFO ownership of their physical
TX queue. A discovery probe and an access-point beacon wait for each other
instead of failing with `Busy`; exchanges on different queues still run
concurrently. The access point's peers, association epochs and beacon
schedule survive failed and successful upstream retries.

In `ProtectedSearch`, the coordinator's `ApSearchPolicy` schedules receive-only off-channel
windows: by default 20 ms, after each AP TBTT for 30 seconds, then about
once a second. An empty list observes only the current channel through the
same search loop. Between windows the AP delivers data and publishes
beacons while the disconnected station observes its upstream on that channel.

At an absence boundary both clients finish their started exchanges. The
owner pauses AP TX by not polling it, reserves the home channel's air with
CTS-to-self through `upper-mac-service::absence::PortAbsence`, tunes the
search channel through `LowerMacLiveRetune` and receives until the absolute
end, then returns home before resuming the AP. A temporary retune preserves
both TSF relations, interfaces, keys, BlockAck state and queue ownership;
it does not disable the port or enable monitor mode. CTS
must complete successfully before departure; a late CTS or exchange skips
an expired window. A failed reservation is reported with the radio home.
The window uses the configured policy's dwell; 20 ms is the default.
An empty window is refused. An expired window or one that reaches the next
TBTT is skipped while the AP continues serving its peers.
Dropping passive reception returns the radio home synchronously; dropping
an active TX remains subject to the completion/ownership contract (#213).
If returning home fails, the retained `AbsenceState` blocks every pair
operation with `RecoveryRequired` until the owner recovers the port and
recreates the composition.

A discovered candidate is kept for `connect_observed_on`: no Probe Request
or second scan runs. On the home channel it joins directly; on another
channel the AP first announces CSA and the owner moves the port once, then
joins. A failed join retains the station and the original dense-period
start. Known hidden BSSs are identified by the previous BSSID, with the
configured SSID used for joining and current security advertisements checked.
An unknown hidden BSSID cannot be identified passively. Equal beacon
intervals whose phases never overlap the windows can prevent passive
discovery indefinitely; there is no bounded discovery-time guarantee.
Active off-channel probes require a separate TX deadline/ownership design
(#215, together with #213).

Its tests (`tests/sta_ap.rs`) run real drivers over
`oer-ieee80211-lower-mac`'s host models joined by `ModelAir`: an upstream
`PortAccessPoint`, the pair, and a `PortStation` that is a peer of the pair's
access point. `ModelAir` respects CTS NAV, so peer data queued during an
absence waits until return. The tests cover same-channel recovery, a
discovered channel followed by CSA and rejoin, preserved downstream peers
and data, dense-to-sparse scheduling, delayed/failed CTS and cancellation
of passive reception, a configurable dwell and skipped late windows. A
backend that refuses enabled-port channel settings and implements neither
absence extension tests current-channel connect, reconnect, passive rejoin
and persistent CSA with lifecycle retuning and preserved downstream peers,
and, in virtual time, the upstream loss policy: an upstream back before
30 s leaves the access point untouched; one back on another channel after
30 s restarts the access point there; a scan that finds none restarts it on
its previous channel and repeats 30 s later.
The S31 protected mode still needs CTS support (#202) and a qualified
live-retune implementation (#218); S31 implements neither extension yet.
This host step supplies no HIL evidence.
