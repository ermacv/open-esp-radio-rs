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
| `PortApEnv` | The port and transmit policy (`PortClientEnv`), the image's monotonic `Timer` the `PortApAuthenticator` that gives each handshake an unpredictable authenticator nonce and its initial replay counter, the `PortApSae` executor of a WPA3 BSS's SAE responder (`NoSae` for another BSS), the `RateControl` of each associated station (`oer-ieee80211-upper-mac`'s seam: `FixedRateControl`, or the Espressif `EspressifRateControl`), and the network's transmit source `Frames` (a `DestinationTxQueues` of `oer-ieee80211-datapath`) |
| `PortApSae`, `InlineSae` | The access point submits one SAE frame at a time and polls for its output beside the port's input, so a Commit's elliptic-curve work runs where the composition places the executor while the BSS goes on; `InlineSae` runs the responder inline; a frame submitted while the last output is not taken is dropped (`sae_dropped`) |
| `PortApProfile` | The BSS: SSID, channel, beacon interval, DTIM period, the claimed `Advertisement`, the management rate (beacons, management, group data), the coexistence priority, the CCMP packet-number step and the receive reorder gap time; its security is the `AccessPointService`'s |
| `PortApStorage` | The memory the composition places: the beacon template, the subframes of one A-MPDU (`aggregate::AmpduSubframes`, at most `PORT_AMPDU_SUBFRAMES`), the `HELD` network frames held for power save (its const parameters: their count, which the composition sizes for its peers' traffic and memory, and the network's frame type, kept as the owner rather than copied), shared by every dozing peer and the group, and the peers' receive Block Ack reordering (`oer-ieee80211-upper-mac-service::reorder::RxReorder`, `AP_MAX_CLIENTS` agreements) |
| `PortAccessPoint::new` | Takes the environment's `PortApParts` (the client, the timer, the authenticator, the SAE executor, the rate controls' configuration and the network's frames), the profile, the BSS's `AccessPointService` (peers, security, the management sequence beacons and responses share) and the storage; refuses a service of another address |
| `PortAccessPoint::start` | Tunes the port to the BSS's channel, configures the access-point interface with `BSS_MEMBER` and `PROBE_REQUESTS`, restarts its TSF and, in a WPA2 BSS, installs the group key |
| `PortAccessPoint::run_until` | Publishes a beacon at each TBTT of the beacon's own absolute schedule, a late publication moving no later TBTT, its ERP and HT protection following the associated peers; answers each Probe Request for its SSID or the wildcard SSID with the current advertisement, at most one response per 10 ms; admits stations by Open System authentication and association, a repeated request answered again without resetting the peer; removes a peer that disassociates or deauthenticates; closes a peer the service finds inactive with a Disassociation (reason 4 for inactivity, else 2) when it was associated and a Deauthentication (reason 2); in a WPA2 BSS runs the four-way handshake as the authenticator (Message 1 after a successful association, Message 3 on a verified Message 2, both retransmitted as the service schedules, a peer closed when they run out) and, on a verified Message 4, installs the peer's pairwise key before authorizing it; removes a peer's pairwise key with the peer and before a new authentication; in a WPA3 BSS hands SAE frames to the executor, authenticates a station whose exchange the responder accepted with its PMK, forgets an unassociated one whose exchange failed, and sends the responder's replies in order |
| `PortAccessPoint::announce_channel_switch` | Announces a move of the BSS to another 2.4 GHz channel in the next `count` beacons and the Probe Responses meanwhile (the Channel Switch Announcement after the TIM, the Secondary Channel Offset for 40 MHz, the count written down at each beacon); at the TBTT after the beacon that counted one `run_until` returns `PortApEvent::ChannelSwitch` and sends no beacon on the old channel |
| `PortAccessPoint::channel_switched` | Called by the port's owner once it has moved the port to the announced channel (`client_mut` reaches the access point's client); writes the beacon template again for that channel, keeping its TBTT schedule |

Beacons go out once, unacknowledged, on the voice queue at the management
rate, stamped with the access point's time and carrying the TIM and the DTIM
count; Probe Responses go out on the voice queue too, acknowledged and
retried by the transmit planner. The access point does not retune for a channel
switch: the port's channel belongs to the port's owner, which moves it for
every interface. The announcement is carried by beacons and Probe
Responses; a Channel Switch Announcement action frame is not sent yet. The composition enables the port. The beacon
carries an empty TIM until the access point buffers for sleeping peers.
EAPOL-Key frames go out as unprotected data MPDUs on the voice queue at the
management rate. A WPA3 BSS's IGTK reaches stations in Message 3; the port holds only the
group and pairwise keys. Management frame protection, which WPA3-Personal requires, is
not served yet: robust management frames go out unprotected, without BIP,
and SA Query is not answered. The direct S31 access point serves no WPA3 at
all (`wifi-security-wpa3-personal-access-point` is `absent`). Data: each authorized peer (Open by its association, a protected BSS's by
its handshake) has a link holding its pairwise key, the CCMP packet numbers
sent to it, its receive replay state and its duplicate filter.
`run_until(deadline, deliver)` takes the network's frames, one destination
after another, one at a time when it is ready to send them, and sends each,
its MPDU header encoded and its payload from the network's owner
(`TxMpdu`, `NetworkBody`), which the port holds while each attempt runs and
which goes back to the network when the exchange ends, to an authorized peer
(plaintext in an Open BSS; under its pairwise key, as QoS data to a QoS peer,
in a protected one) at its rate control's rate, or to the group (under the group key)
at the management rate, dropping one for no authorized destination
(`data_dropped`); and hands the MSDUs of each authorized peer's data to the
distribution system to the application as `PortMsdu`s, the only MSDU of an
MPDU in the port's buffer, after its duplicate check and, in a protected BSS,
its hardware decryption and packet-number check per lane (`duplicates`,
`replayed`, `rx_rejected`, `malformed`). Every MSDU goes to the
application, which bridges between peers if it wants to. Power save follows each authorized peer's power-management bit (any data
MPDU to the distribution system, Null Data included) and its PS-Poll,
through the service's accounting: a frame for a dozing peer is held, and
group frames are held while any authorized peer dozes; each beacon carries
the TIM of the peers with held frames and the group bit; a PS-Poll gets one
held frame, a peer that wakes gets every one, and the group frames a DTIM
beacon announced follow it, More Data set while more remain (`held`,
`held_dropped` with every slot taken, `released`). A peer's held frames go
with it. Receive Block Ack: an authorized peer's ADDBA Request is accepted when the
port and the reorder storage can hold its window (the port installs the
agreement, and the response succeeds) or declined; its data of that TID is
reordered before its packet-number check and decapsulation, an in-order MPDU
from the port's buffer and a kept one from the storage; its BlockAckReq
moves the window, a kept run goes past its gap after the profile's gap time,
and its DELBA or its removal ends the agreement in the port too
(`rx_agreements`, `behind_window`, `unbuffered`, `reorder_gap_timeouts`).
TX Block Ack: in a protected BSS the access point offers each newly
authorized HT QoS peer its TID-0 agreement by an ADDBA Request at once
(`tx_agreements_offered`), with the profile's `tx_block_ack_retry`: an
offer that is not acknowledged or not answered (`tx_agreements_failed`)
uses one attempt, and the next goes with the peer's data once the retry
interval has passed, while attempts remain. The peer's response makes the
agreement operational (`tx_agreements`) or declines it, which ends the
offers; the peer's DELBA as recipient, or its removal, ends it. While it is operational, a frame for the awake peer and the frames the network
has queued for it behind it go as one A-MPDU of TID 0 under its pairwise key
at its A-MPDU rate (`aggregates`, `aggregated_acknowledged`), as many as the
agreement's window, the port (`PortClientEnv::Aggregation`), the peer's HT
A-MPDU Parameters and the Best Effort TXOP limit the BSS advertises admit
(`AmpduLimits`), keeping the peer's minimum MPDU start spacing; the
subframes' headers and the network's frames that carry their payloads are
kept in `PortApStorage`'s `AmpduSubframes` until the exchange ends. An Open BSS
offers no agreement.
Rate control: a peer's link opens at its association, in every BSS, with
its own `RateControl`, built for the peer's HT width in the BSS (HT40 where
both support 40 MHz) from the link metric of its Association Request; its
single MPDUs go at the controller's MPDU rate and its A-MPDUs at its A-MPDU
rate, and every exchange's outcome teaches it. Group frames go at the
management rate. Only an authorized peer's data reaches the distribution
system, its link notwithstanding.
Not served yet: fragments (`malformed`).

Tests: `tests/port_access_point.rs` runs the access point over the
`LowerMacModel` on virtual time.
