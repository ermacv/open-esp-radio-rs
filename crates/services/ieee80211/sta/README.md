# Wi-Fi station drivers

`oer-ieee80211-sta-service` drives the station state machines of
[`oer-ieee80211-sta`](../../../protocols/ieee80211/sta/README.md). That crate
is sans-IO: it owns state, policy and the ports; this crate owns the futures
that wait on those ports and on the `oer-time` `Timer`, for any executor.

| Module | Driver |
| --- | --- |
| `join` | `StaJoinRunner`: Open System and SAE Authentication and Association against absolute millisecond deadlines |
| `scan` | `StaCandidateScanService`: one finite channel plan and candidate selection; `StaScanBackend`: the channel-visit transaction (switch, receive, optional probe, dwell, stop) over a `StaScanPort` |
| `attempt` | `StaAttempt`: every pre-connected phase of one attempt over a `StaAttemptPort`, once and in order |
| `station` | `StaLifecycleService`: outer attempt, reconnect, backoff, disconnect and stop |
| `port` | The station over any [lower-MAC port](../../../protocols/ieee80211/lower-mac/README.md) |

Every driver returns the exact caller-owned radio state at each success,
retry, stop and failure edge. The ESP32-S31 runtime implements the scan and
attempt ports with its chip owners and polls these drivers.

## The station over the lower-MAC port

`port` implements the ports of these drivers and of the WPA2 runners of
[`oer-ieee80211-rsn-service`](../rsn/README.md) over any
`Ieee80211LowerMacPort`, without chip types. Frames leave through
`UpperMacTx` of
[`oer-ieee80211-upper-mac-service`](../upper-mac/README.md), one port
submission per attempt.

| Item | Port it implements | Over the lower-MAC port |
| --- | --- | --- |
| `PortLink` | The station's client of the port's `PortRouter` | Reads the router's receive, extension (TBTT) and lifecycle queues and transmits through `UpperMacTx` over the router; a loss is a `PortInput::EventsLost` input, the terminal poisoned event ends every phase with `PortLinkError::Poisoned` |
| `PortScan` | `StaScanPort` | `LowerMacSetting::Channel` (a backend that retunes only while disabled answers `Busy` and is disabled, tuned and enabled again), the station filter `OTHER_BSS_MANAGEMENT` (or `LowerMacMonitor` through `with_monitor` when the filters lack it), a Probe Request per channel, beacons and Probe Responses into a `ScanTable` |
| `PortJoin` | `StaJoinBackend` | Open System and SAE Authentication and Association Requests; receive is the station filter `BSS_MEMBER` with the access point's BSSID |
| `PortHandshake`, `PortKeyInstall` | `RsnHandshakeBackend`, `RsnKeyInstallBackend` | EAPOL in data MPDUs; the pairwise and group CCMP-128 keys through `install_key` (`PortKeys`), removed again on a failed install; Message 4 in the clear or under the pairwise key |
| `PortConnection` | The connected data plane | QoS or non-QoS data with per-TID sequence numbers and a CCMP header from `CcmpTxPacketNumber`, at the access category of the user priority; receive duplicate filter, Block Ack reordering (ADDBA, DELBA, BlockAckReq) into the station's slots, CCMP replay check per lane after reordering, A-MSDU deaggregation; SA Query under management frame protection; Deauthentication and Disassociation; the Group Key Handshake under the pairwise key |
| `PortPowerSave` | `oer_ieee80211_sta::modem_sleep` | Runs for every association with the profile's `sleep_type` (`set_sleep_type` restarts it); TBTTs and TSF of `LowerMacBeaconTiming`, which the station's port must implement, doze as `TxGate { open: false }`, Null frames with the Power Management bit, the frame held while the station dozes |
| `PortStation`, `PortAttemptPort` | `StaAttemptPort` | Scan when the candidate must be refreshed, tune, authenticate (Open System, SAE, or Open System resuming a cached SAE PMKSA), associate, program the BSS filter, handshake, install keys, enter the connection |
| `PortStationLifecycle` | `StaLifecycleBackend` | Attempts through `StaAttempt`, the connection served for a `PortStationApplication`, backoff |

The integrator names its port, transmit-policy parameters, backoff entropy,
timer and key-data unwrap once in a `PortStationEnv`, and supplies the
station's addresses, rates, channels, capabilities and the CCMP
packet-number step in `PortStationConfig` and `PortStationProfile`; the
Espressif step is a value of
[`oer-espressif-ieee80211-policy`](../../../protocols/espressif/ieee80211/policy/README.md),
which this crate does not depend on.

The port's one event consumer is its `PortRouter`, the `EventRouter` of
`oer-ieee80211-upper-mac-service`, which the composition polls
(`EventRouter::run`) beside the station and the backend's runner. All of
one station runs in one task that owns its `PortLink`; the router hands
every completion to the exchange that registered its identity and queues
received frames (up to `PORT_BACKLOG`) and TBTTs until a phase reads them.
After a loss, an exchange whose completion may be in the gap cancels its
attempt and either receives the completion or ends with
`UpperMacTxError::CompletionLost`; a lifecycle command whose terminal may be
in the gap ends with `PortLinkError::LifecycleLost`. Received frames in the
gap are gone and the connection goes on.

The station reads a protected MPDU the backend reports as
`DecryptedAndIntegrityVerified` with its CCMP header kept and without the
MIC, the mirror of what it hands the backend for transmission.

A WPA2 connection answers the access point's group rekeys. An EAPOL frame
under the pairwise key goes to the connected supplicant, never to the
caller: a new Group Message 1 installs its group key through `install_key`,
removes the old one, restarts the group replay window at the frame's receive
sequence counter and is answered with Group Message 2 under the pairwise
key; a repeat of the last one is answered again with no key change. An
unprotected one is dropped with the other unprotected data, and a rejected
frame counts in `eapol_rejected`.

The station contends with the access point's EDCA parameters as the vendor
station does: the advertised WMM Parameter Element from its scan record once
the station is tuned, so Authentication and Association already use it, and
the Association Response's set in its place when the response carries one.
Each goes whole to the port (`LowerMacSetting::Edca`: AIFSN and TXOP limit)
and to the transmit planner (each access category's contention window). An
access point without the element leaves the backend's defaults; a later
change of the set in its beacons is not followed.

Associated, the station keeps its access point as the association left it
(`StaAssociatedPeer` of `oer-ieee80211-sta`, in `PortConnectionConfig::peer`):
the PHY, the HT capabilities and A-MPDU parameters, the HE capabilities, HE
state and BSS color, and the BSS protection facts, which the transmit planner
applies from then on. The nominal HE packet padding is the integrator's
policy (`PortStationProfile::he_packet_padding`). An HE association carries
the station's HE power elements (`PortStationProfile::he_power`), from its
calibrated transmit power; without them the HE Association Request is
refused. The data rate is still the profile's: the peer's capabilities do
not yet bound it.

A QoS HT or HE association under CCMP negotiates the TX Block Ack
agreements of `PortStationProfile::tx_block_ack` once connected, through
`StaTxBlockAckOriginator` of `oer-ieee80211-sta`: an ADDBA Request per TID
of the policy, in order, each from that TID's next sequence number, while the
station is awake. A request that is not acknowledged or not answered within
the negotiation timeout uses one of the TID's attempts and is sent again
while attempts remain; the access point's answer ends them, and its DELBA as
recipient ends the agreement.

`PortStation::send` queues a frame (`PortSend::Queued`, or `Full` with the
eight-frame queue full) and `run_until` sends the queue while the station is
awake, a dozing station keeping it. A run of one user priority goes as one
A-MPDU when that TID's agreement is operational, the keys are installed and
the port aggregates at the data rate (`PortStationEnv::Aggregation`:
`PortAmpduAggregation` over a port with `LowerMacAmpdu`, `NoAggregation`
otherwise); the run is bounded by the agreement's window, the port's
subframes and length, the peer's Maximum A-MPDU Length and the TXOP limit
of the access category in the association's EDCA set (the aggregate's
`max_ppdu_duration_micros` and its BlockAck at 6 Mb/s after a SIFS), and the
subframes keep the peer's minimum MPDU start spacing. Every other frame goes
alone. `PortConnection::tx_counters` counts what left and was
acknowledged; an exchange whose completion was lost counts as failed, and
any other error ends `run_until`.

An association that protects its management frames keeps the BIP receive
state of its IGTK (`oer-ieee80211-rsn`'s `BipReceiver`), from Message 3 and
each group rekey that carries one. A group-addressed robust management frame
(Deauthentication, Disassociation, or an Action of a robust category) counts
only when it verifies under that IGTK: a verified Deauthentication or
Disassociation ends the association, a group Action carries nothing the
station answers, and one that does not verify, or arrives without a
Management MIC element, counts in `bip_rejected`, as the vendor's
`sta_bip_check` drops it.

The station chooses its mode for each access point with
`oer-ieee80211-sta`'s `select_association_phy` and the profile's
`preference`, as the S31 station does: HE20, HT40 when the access point's
HT elements agree on a secondary channel and the port tunes 40 MHz, HT20,
or legacy for an access point without HT. An HT40 association tunes the
primary channel with its secondary above or below (`ChannelWidth::Mhz40Above`
or `Mhz40Below`). The station transmits at the link's data rate; a 40 MHz
transmission waits for the rate bound by the peer's capabilities.

A receive reorder window that buffers an MPDU behind a missing one waits
the profile's `rx_reorder_gap` from the first MPDU it retained (the
Espressif stack's 300 ms), then releases the buffered run past the gap and
counts it in `reorder_gap_timeouts`; a full slot store releases the oldest
run at once.

The coexistence schedule belongs to the radio system the station shares its
RF with, not to the Wi-Fi MAC behind the port: the environment names it once
as `PortStationEnv::Coex` (`PortCoexistence`; `NoCoexistence` for a station
alone on its RF). The power manager decides every input with its `view()`
and hands it every `PmCoexAction`: event requests and releases, the schedule
interval, phase restarts and the flexible period. A refused effect ends the
operation with `PortLinkError::Coexistence`. The beacon receive priority is
the MAC's: the manager applies `LowerMacSetting::RxBeaconPriority`, and the
backend receives beacons at its radio system's beacon-window priority, at
zero, or withdraws the request.

The station supervises its link as the S31 station does, in software above
the port (`oer-ieee80211-sta`'s `StaLinkMonitor`, the profile's
`PortLinkSupervision`): each beacon and each Probe Response of its access
point keeps the link; after `timeout` without one (the Espressif 6 s) it
sends the probes of `STATION_LINK_PROBE`, three addressed to the access
point and two broadcast, 500 ms apart, and leaves with
`PortDisconnect::BeaconLoss` after the last goes unanswered.

It does not yet do: a data rate bound
by the peer's capabilities (and with it 40 MHz transmission), PS-Poll, the hardware beacon receive time
(`PmAction::RxBeaconTime`: the S31 register model leaves the meaning of its
second value open, so the port has no setting for it until that is
established), and roaming.

Its tests (`tests/port_station`) run it over `oer-ieee80211-lower-mac`'s host
model with a scripted access point on virtual time: an active scan, an Open
System join with data both ways, a WPA2-PSK connection whose keys are
installed through the port, a Block Ack window released in order with a
replay and a duplicate dropped, power save around a TBTT with a buffered-data
TIM, a Deauthentication by the access point, an SAE join with an SA Query
under management frame protection, and the lifecycle rejoining after a
Deauthentication.
