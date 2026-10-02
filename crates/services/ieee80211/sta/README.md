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
| `PortConnection` | The connected data plane | QoS or non-QoS data with per-TID sequence numbers and a CCMP header from `CcmpTxPacketNumber`, at the access category of the user priority; receive duplicate filter, Block Ack reordering (ADDBA, DELBA, BlockAckReq) into the station's slots, CCMP replay check per lane after reordering, A-MSDU deaggregation; SA Query under management frame protection; Deauthentication and Disassociation |
| `PortPowerSave` | `oer_ieee80211_sta::modem_sleep` | TBTTs and TSF of `LowerMacBeaconTiming`, doze as `TxGate { open: false }`, Null frames with the Power Management bit, the frame held while the station dozes |
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
MIC, the mirror of what it hands the backend for transmission. It does not
yet do: the group key handshake (rekeying), BIP for group-addressed robust
management frames, beacon-loss monitoring (`link_monitor`), the reorder gap
timer (a full slot store releases the oldest run instead), WMM EDCA
parameters from the access point, HT/HE association capabilities beyond the
caller's elements, A-MPDU transmission and TX Block Ack agreements, PS-Poll,
coexistence (the power manager runs with `CoexView::INACTIVE`), 40 MHz
channels and roaming.

Its tests (`tests/port_station`) run it over `oer-ieee80211-lower-mac`'s host
model with a scripted access point on virtual time: an active scan, an Open
System join with data both ways, a WPA2-PSK connection whose keys are
installed through the port, a Block Ack window released in order with a
replay and a duplicate dropped, power save around a TBTT with a buffered-data
TIM, a Deauthentication by the access point, an SAE join with an SA Query
under management frame protection, and the lifecycle rejoining after a
Deauthentication.
