# oer-ieee80211-sta-ap-service

A Wi-Fi station and access point on one lower-MAC port, without an
executor.

`PortStaAp` owns a port that two clients share: the station
(`oer-ieee80211-sta-service`'s `PortStation`) on one interface, the access
point (`oer-ieee80211-ap-service`'s `PortAccessPoint`) on the other, and the
channel coordinator (`oer-ieee80211-ap::coordinator::ChannelCoordinator`)
that decides the port's one channel. The station's upstream BSS decides it,
and the access point follows within its `ApFollowPolicy`.

| Operation | What it does |
| --- | --- |
| `PortStaAp::connect` | The station joins its upstream, alone on the port, and the access point starts on the upstream's channel when the policy lets it serve it |
| `PortStaAp::reconnect` | Discovers and joins the upstream on the running access point's current channel, serving the access point and delivering its data throughout the attempt; retains the station on failure |
| `PortStaAp::run_until` | Runs both clients at once, each until the next sync point (the access point's next TBTT, a retune the coordinator asked for, or the deadline), so no client's future is dropped mid-exchange; at each sync point it hands their reports to the coordinator and carries out its actions |

The actions it carries out:

- **Announce a switch.** When the upstream announces a switch, the access point announces the same move to its peers in the beacons that fit before the upstream's switch instant.
- **Retune.** The port moves once for both interfaces, and both clients are told their switch is done. With the access point running, this happens at its due TBTT; with the station alone, at the upstream's switch instant.
- **Stop the access point.** When the upstream goes where the policy does not let the access point follow, its peers are deauthenticated and the BSS closes. The station follows the upstream alone.
- **Start the access point.** It starts again once the port is on a channel it may serve.
- **Leave the upstream.** This is the alternative policy: the station leaves, and the access point keeps its channel.

Neither client tunes the port while both run. A lost upstream ends
`run_until` with `PortStaApEvent::StationEnded`; subsequent calls serve the
access point by itself. The caller may retry with `reconnect`, which grants
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

Searching other channels in protected absences is not done yet.

Its tests (`tests/sta_ap.rs`) run real drivers over
`oer-ieee80211-lower-mac`'s host models joined by `ModelAir`: an upstream
`PortAccessPoint`, the pair, and a `PortStation` that is a peer of the pair's
access point.
