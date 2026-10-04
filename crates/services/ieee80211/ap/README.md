# Wi-Fi access point over the lower-MAC port

`oer-ieee80211-ap-service` runs an access point over any
[lower-MAC port](../../../protocols/ieee80211/lower-mac/README.md), through one
`PortClient` of
[`oer-ieee80211-upper-mac-service`](../upper-mac/README.md) and the port's
`EventRouter`, which the composition polls beside it. Policy belongs to
[`oer-ieee80211-ap`](../../../protocols/ieee80211/ap/src/lib.rs) (the beacon,
peers, security and power save); frame codecs belong to `oer-ieee80211-mac`.

| Item | Role |
| --- | --- |
| `PortApEnv` | The port and transmit policy (`PortClientEnv`) and the image's monotonic `Timer` |
| `PortApProfile` | The BSS: SSID, channel, beacon interval, DTIM period, the claimed `Advertisement`, the management rate and coexistence priority; its security is the `AccessPointService`'s |
| `PortAccessPoint::new` | Takes the client, the timer, the profile, the BSS's `AccessPointService` (peers, security, the management sequence beacons and responses share) and the beacon storage; refuses a service of another address, and an RSN service until its handshake is served |
| `PortAccessPoint::start` | Tunes the port to the BSS's channel, configures the access-point interface with `BSS_MEMBER` and `PROBE_REQUESTS`, and restarts its TSF |
| `PortAccessPoint::run_until` | Publishes a beacon at each TBTT of the beacon's own absolute schedule, a late publication moving no later TBTT, its ERP and HT protection following the associated peers; answers each Probe Request for its SSID or the wildcard SSID with the current advertisement, at most one response per 10 ms; admits stations by Open System authentication and association, a repeated request answered again without resetting the peer; removes a peer that disassociates or deauthenticates; closes a peer the service finds inactive with a Disassociation (reason 4 for inactivity, else 2) when it was associated and a Deauthentication (reason 2) |

Beacons go out once, unacknowledged, on the voice queue at the management
rate, stamped with the access point's time and carrying the TIM and the DTIM
count; Probe Responses go out on the voice queue too, acknowledged and
retried by the transmit planner. The composition enables the port. The beacon
carries an empty TIM until the access point buffers for sleeping peers.
Not served yet: the WPA2 handshake and pairwise keys of an RSN BSS, SAE,
Block Ack actions (counted in `unserved`) and data.

Tests: `tests/port_access_point.rs` runs the access point over the
`LowerMacModel` on virtual time.
