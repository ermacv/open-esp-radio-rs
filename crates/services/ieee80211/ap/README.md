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
| `PortApEnv` | The port and transmit policy (`PortClientEnv`), the image's monotonic `Timer` the `PortApAuthenticator` that gives each handshake an unpredictable authenticator nonce and its initial replay counter, and the `PortApSae` executor of a WPA3 BSS's SAE responder (`NoSae` for another BSS) |
| `PortApSae`, `InlineSae` | The access point submits one SAE frame at a time and polls for its output beside the port's input, so a Commit's elliptic-curve work runs where the composition places the executor while the BSS goes on; `InlineSae` runs the responder inline; a frame submitted while the last output is not taken is dropped (`sae_dropped`) |
| `PortApProfile` | The BSS: SSID, channel, beacon interval, DTIM period, the claimed `Advertisement`, the management rate and coexistence priority; its security is the `AccessPointService`'s |
| `PortAccessPoint::new` | Takes the client, the timer, the profile, the BSS's `AccessPointService` (peers, security, the management sequence beacons and responses share) and the beacon storage; refuses a service of another address |
| `PortAccessPoint::start` | Tunes the port to the BSS's channel, configures the access-point interface with `BSS_MEMBER` and `PROBE_REQUESTS`, restarts its TSF and, in a WPA2 BSS, installs the group key |
| `PortAccessPoint::run_until` | Publishes a beacon at each TBTT of the beacon's own absolute schedule, a late publication moving no later TBTT, its ERP and HT protection following the associated peers; answers each Probe Request for its SSID or the wildcard SSID with the current advertisement, at most one response per 10 ms; admits stations by Open System authentication and association, a repeated request answered again without resetting the peer; removes a peer that disassociates or deauthenticates; closes a peer the service finds inactive with a Disassociation (reason 4 for inactivity, else 2) when it was associated and a Deauthentication (reason 2); in a WPA2 BSS runs the four-way handshake as the authenticator (Message 1 after a successful association, Message 3 on a verified Message 2, both retransmitted as the service schedules, a peer closed when they run out) and, on a verified Message 4, installs the peer's pairwise key before authorizing it; removes a peer's pairwise key with the peer and before a new authentication; in a WPA3 BSS hands SAE frames to the executor, authenticates a station whose exchange the responder accepted with its PMK, forgets an unassociated one whose exchange failed, and sends the responder's replies in order |

Beacons go out once, unacknowledged, on the voice queue at the management
rate, stamped with the access point's time and carrying the TIM and the DTIM
count; Probe Responses go out on the voice queue too, acknowledged and
retried by the transmit planner. The composition enables the port. The beacon
carries an empty TIM until the access point buffers for sleeping peers.
EAPOL-Key frames go out as unprotected data MPDUs on the voice queue at the
management rate. A WPA3 BSS's IGTK reaches stations in Message 3; the port holds only the
group and pairwise keys. Not served yet: Block Ack actions (counted in
`unserved`) and data other than the handshake's EAPOL.

Tests: `tests/port_access_point.rs` runs the access point over the
`LowerMacModel` on virtual time.
