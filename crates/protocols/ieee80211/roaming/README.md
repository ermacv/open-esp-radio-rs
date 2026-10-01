# IEEE 802.11k/v protocol logic

`oer-ieee80211-roaming` owns the practical k/v exchanges, neighbor database,
candidate selection, selected power services and Event/Diagnostic reports. It is `no_std`, allocation-free
and sans IO. Inputs are validated Action bodies, association identities, actual
measurements, transmission results and monotonic time supplied by the caller.
The core never reads a clock, sends frames, scans or changes an association.

Wire codecs live in [`mac::roaming`](../mac/src/roaming.rs). Status and action
values use protocol-specific types. Action categories, IE identifiers and
service-specific fields stay with their wire formats. Common MAC address,
SSID and TU facts come from `mac::management`; Ethernet representation, valid
user priorities and cipher parameters come from `mac::data`, `mac::qos` and
`mac::security`. Internal table capacities remain private to their owners.

Shared TCLAS validation checks classifier syntax. Each service decides whether
Processing is required and which additional attributes it permits.
`tclas::{ClassifierPacket, IpFields, Ports}` supplies normalized packet facts
and common Ethernet/IP matching for DMS and TFS without membership or delivery
policy. TFS owns payload/VLAN matching; DMS owns multicast admission and QoS
attributes. Unknown assigned values remain available for explicit admission
by the protocol owner.
Action bodies start
with Category and Action. Management headers, addresses, sequence numbers,
protection, FCS and packet/radio buffers belong to the existing integration.
The integration checks permitted source/destination roles, common BSS membership
and the latest advertised service capabilities before creating/routing owners.
Transition/RSNA event requests require an infrastructure BSS.

| Owner | Behavior |
| --- | --- |
| `neighbor::NeighborReportRequester` / `NeighborReportResponder` | Complete Neighbor Report exchanges and bounded response cache |
| `measurement::LinkMeasurementRequester` / `LinkMeasurementResponder` | Link Measurement exchanges with TPC, antennas, RCPI and RSNI |
| `measurement::RadioMeasurementRequester` / `RadioMeasurementResponder` | Beacon, Channel Load and Noise Histogram requests, execution groups, repetitions, conditions and complete reports |
| `database::NeighborDatabase` | Independently timed reports and scan observations, peer capabilities and BSS Load |
| `selection::RoamingSelector` | Compatible candidate filtering, scoring, confirmation, hysteresis, cooldown and failure backoff |
| `btm::BtmStation` / `BtmAccessPoint` | BTM proposals, explicit decisions, responses and bounded replay |
| `idle::BssIdleStation` / `BssIdleAccessPoint` | BSS Max Idle activity, keep-alive and expiry |
| `sleep::WnmSleepStation` / `WnmSleepAccessPoint` | WNM Sleep negotiation, DTIM wake planning and key/filter/power application |
| `sleep::SleepTrafficFilters` | Embedded TFS negotiation, matching, notifications and Delete-after-match |
| `tfs::TfsStation` / `TfsAccessPoint` / `TrafficFilters` | Standalone TFS full replacement, classifier admission, delivery and notifications |
| `events::EventJournal` / `EventStation` / `EventRequester` | Network-scoped event history, request conditions, response snapshots and frequent-transition alerts |
| `diagnostics::DiagnosticStation` / `DiagnosticRequester` | Manufacturer/configuration reports and admitted association/802.1X work with exact timeouts |
| `dms::DmsStation` / `DmsAccessPoint` / `DmsRegistry` | DMS membership, BSS-wide transactions, multicast classification and terminal sequence history |

## Driving an owner

Construct one owner for each peer and association generation. Deliver frames
whose addresses, association and negotiated management protection have already
been checked. Carry the original generation through queued RX/TX events.
Replacing an association retires its owners and requires a new generation.
A diagnostic-caused excursion preserves its work until return to the original
requester; `DiagnosticStation::reattached` binds the new generation explicitly.

1. Start a request or receive a proposal with the supplied `Instant`.
2. Read `transmission()` (`delivery` for Event/Diagnostic responders), copy its body into integration-owned TX storage, then
   call `admitted(id, now)` after the backend accepts that owned storage.
3. Feed terminal delivery through `tx_completed(id, outcome, now)`.
   `Acknowledged` means peer acknowledgement. Backend retries stay backend-owned.
4. Route received Action bodies to `receive`. A reply may arrive after admission
   and before TX completion; late completions cannot finish a newer operation.
5. Schedule `next_deadline()` and call `poll(now)` when due. Drain events and
   queued transmissions until the next deadline lies in the future.

`OperationId` is a local identity separate from the eight-bit wire dialog token.
Route it to the issuing owner. Event/Diagnostic Action dialogs require nonzero
tokens; standalone TFS dialogs and Diagnostic element tokens permit zero.
Avoid token reuse while an older reply could arrive within the same association.
Different owners have independent local identity sequences.

Const capacities bound complete retained bodies, entries, report frames and
tasks. Full storage is an explicit error; live protocol exchanges and optional/
vendor data are never silently evicted or cut. The event journal has an explicit
per-type history ring: a newer event replaces the oldest of that same type.
Malformed or oversized input cannot replace
a valid pending operation. Cancellation does not reclaim integration-owned
DMA/TX storage; the backend remains responsible for physical completion.

## Measurements

The radio responder retains an entire request and exposes `work()` with an
operation/round/group identity, request elements and permitted start window.
The Parallel bit groups the current measurement with the following one.
Groups run sequentially. Zero repetitions means one round; a finite value adds
that many rounds. The value 65535 repeats until cancellation/supersession within
the caller's bounded lease.
Individually addressed requests outrank multicast and broadcast requests.
Control permissions still apply to lower-priority frames.

Call `start_work` after actual scan/measurement admission. Complete the group
with a measured or rejected result for every request. Rejection needs no scan
admission. The owner checks tokens, types, duration and elapsed measurement time,
then suppresses false reporting conditions. It supports the ten Beacon
conditions, including signed offsets against supplied serving-AP RCPI/RSNI.
SSID/BSSID filters and mandatory durations are checked. Missing reference data,
unknown semantics and incomplete results are explicit errors. An empty Beacon
Report represents no observations. Group-addressed refusal/incapable results
are suppressed as the RRM procedure requires.

`beacon_channels` resolves exact channels, channel 0, channel 255 and AP Channel
Reports over the complete caller-supplied regulatory/backend permission set.
Table mode reads existing observations without scanning. `reported_beacon_body`
applies Reporting Detail and requested IE IDs in the observed frame's order.
The base Reported Frame Body wire limit ends at a complete IE and exposes an
omitted-element count; local storage shortage remains an error. Later report
fragmentation/extensions are retained by codecs but are not interpreted.

The requester retains complete report frames until its result window closes;
the first frame does not prove all repeated or Beacon reports have arrived.
Exact duplicates are ignored. Incapable is sticky for the association. TPC,
TSF, RCPI, RSNI, channel-load and noise bins are measured inputs, never values
fabricated from another metric. Connected scans, random start selection,
home-channel return, power-save and coexistence lifetimes remain integration work.

## Neighbor database and automatic selection

The database retains entire Neighbor Report IEs, including unknown subelements,
and independent scan observations with separate freshness deadlines. Neighbor
Reports carry no SSID: the caller supplies request context or leaves it unknown.
A report alone is a scan hint, not evidence of a reachable compatible AP.
Full batch admission is atomic and never replaces an unexpired live entry.

Construct the selector with immutable `NetworkProfile` (SSID and existing MAC
security policy), `SelectionPolicy` and the current association identity.
Each evaluation receives the database, complete allowed channel set, an
autonomous/BTM trigger and backend PHY/throughput assessments. Candidates need
a fresh observation, exact binary SSID, an RSN configuration accepted by the
existing association selector, a legal supported channel and sufficient RSSI.
The current BSS also needs a fresh observation; otherwise the result requests
a scan. Unknown throughput is not estimated from RSSI.

All scoring weights and thresholds are explicit. RSSI saturates at the configured
sufficient level; available-channel capacity, actual throughput estimates and
AP preference contribute configured bonuses. Unavailable bonus data contributes
zero. Preference zero excludes a candidate; other preferences remain advisory.
BTM abridged lists and candidate lifetimes constrain eligibility. Old stored BTM
preferences do not influence a later autonomous choice. Equal scores use stable
BSSID ordering.

A better candidate must exceed the improvement threshold and persist through
the confirmation interval with a second actual observation. Time alone cannot
confirm one scan sample. A decision is emitted once, with a validity deadline.
Call `admitted` only when beginning the actual association attempt, and `finish`
with its authenticated terminal result. Failures cause bounded exponential
backoff; success installs the new link generation and starts cooldown. Replace
the old association's database and dialogs at that boundary.

## BTM decisions and timing

`BtmStation::proposal()` exposes the complete request and timing. Route a
selector result with its proposal identity into an explicit `BtmDecision` after
validating fresh backend facts. `CandidateSource::Request` requires exactly one
live non-excluded advertised candidate. `Scan` declares an independently observed
target while respecting live exclusions and an abridged list.

Candidate validity uses the actual beacon interval in TU. A nonzero
disassociation timer counts TBTTs from the supplied next TBTT; zero specifies no
disassociation deadline. BSS termination requires an explicit TSF-to-monotonic
mapping. Decision and response deadlines respect known disassociation/termination
deadlines. Timeout queues rejection if time remains to send it.

Acceptance produces `TransitionReady` after acknowledged response delivery.
Only then does the integration begin the actual transition and admit its
selection. Exact duplicate requests preserve the original deadline and replay
a cached response without a second transition. The same live token with changed
content is a conflict. Distinct proposals supersede with a new local identity.

## Idle and WNM Sleep

BSS Max Idle periods count 1000 TU (1.024 seconds). Accepted STA-initiated
data/management traffic refreshes an AP lease. STA keep-alive acknowledgement
proves accepted delivery; queueing or a failed transmission does not reset the
lease. Protected keep-alive policy requires available negotiated protection.
Supply an explicit keep-alive margin and integrate its deadline with physical
wake scheduling, including an otherwise indefinite WNM Sleep interval.

WNM Sleep intervals count DTIMs. Supply actual beacon interval, DTIM period,
next DTIM and negotiated BSS Max Idle. Entering requires accepted TFS filters
and retirement of GTKSA/IGTKSA as negotiated. Protected exit supplies current
and pending GTK/IGTK to the existing security owner, which retains replay and
anti-reinstall rules. Without PMF, exit requires the ordinary group-key handshake.
The current profile validates CCMP GTKs and BIP-CMAC-128 IGTKs; unsupported key
formats are explicit errors. Key data is redacted in Debug output.

STA acceptance produces `ApplyRequired`; state changes through `services_applied`
only after the required services have completed. AP `prepare_response` validates
and owns the complete response before filter/queue/key services change.
The owned `SleepResponsePlan` is returned through `services_applied` after AP
application, or through `cancel_response` before changing services. Acceptance
publication follows application. Expiry/loss where services or peer state may
have changed exposes `Unknown` and recovery, never a claimed rollback.
Cached replay cannot reapply services or extend the original lease.

`SleepTrafficFilters::install` checks complete embedded TFS request/status
coverage and retains alternatives/vendor data. TCLAS types 0 through 5 match
normalized Ethernet/IP/VLAN facts and original plaintext payload. Type 3 offsets
start after the MAC header. EAPOL-Key frames have the mandatory delivery filter;
group delivery continues for other STAs. `evaluate` stages notification IDs,
TIM triggering and Delete-after-match. `applied` deletes filters only after the
required notification and matched-frame queue admissions. `cancel` preserves
filters if admission failed. Embedded and standalone TFS share this classifier
and delivery owner. WNM Sleep admission requires at least one accepted filter;
standalone TFS also permits an empty cancellation or an entirely denied replacement.

## Standalone Traffic Filter Service

`TfsAccessPoint` retains the latest complete request and installs an explicitly
supplied full response only after status coverage, classifier support, storage
and response encoding pass. Omitted filters are removed. An empty request
cancels all filters; an entirely denied replacement also leaves no active
filters. Alternative classifier proposals remain hints and are never installed
implicitly. Replayed requests resend the retained response without installing
filters again or extending the dialog deadline.

`TfsStation` retains the acknowledged negotiation, correlates complete status
coverage and validates all Notify IDs before changing local membership.
A lost admitted replacement makes membership uncertain and emits
`RecoveryRequired`; a new acknowledged full replacement establishes it again.
The lower MAC must perform ordinary retry/duplicate suppression before Notify
routing. `TrafficFilters` owns matching and queue-dependent deletion;
notification and matched-frame admission remain explicit integration inputs.

## Event Reports

`EventJournal` belongs to a supplied `NetworkIdentity` representing one ESS or
IBSS. Its separate per-type capacities retain at least the five most recent
Transition, RSNA and Peer-to-Peer records. WNM Log records contain complete
caller-supplied RFC 3164 ASCII messages, including the MAC address TAG; UTC offset,
time accuracy and TSF are supplied facts. A terminated peer link removes its
logged initiation. Active-link connection duration is computed at report time.
Keep the journal across a BSS transition and call `change_network` on leaving
the network. Retire the peer dialog at a BSS transition.

`EventStation` prepares an entire bounded snapshot before publishing it.
Recognized address, transition-time, outcome, AKM/EAP and operating-class/channel
conditions select the most recent matching entries; unknown conditions are
ignored. A zero response limit selects all retained entries. Empty results use
an explicit successful element without event data. Complete IEs span additional
Action frames without splitting an IE. `prepare_with_admission` also accepts
explicit Failed, Refused and Incapable policy decisions; rejected requests
activate no alert subscription. Unsupported later event types and frequent
thresholds beyond the configured history capacity report Incapable.

Feed newly logged transitions to `transition_logged`. The local event identity
prevents duplicate alerts. Counts use supplied monotonic time and the request's
TU interval. IEEE 802.11-2012 gives conflicting status=4 layouts in 8.4.2.70.1
and 10.23.2.2. Parsing accepts both; `FrequentTransitionFormat::StatusOnly` or
`WithLastEvent` is required explicitly by the station constructor and frequent
report encoders. There is no default, including for raw report-frame encoding.

`EventRequester` retains complete initial report frames for its configured
response window. Frequent-transition alerts continue to correlate with the
retained subscription after that window closes, until replacement or
cancellation. Admission is retained independently for each Event Token.
Autonomous reports and later monitoring alerts have separate
bounded histories with explicit clear methods. Incapable is sticky for the
peer association. The first response does not prove all frames have arrived.

## Diagnostic Reports

`DiagnosticStation::receive` preflights the complete request and every task's
exact timeout in seconds before replacing current work. The admission callback
only supplies resource/policy facts. Accept confirms backend support, network
membership, channel permission and availability of the requested profile/EAP/
credential types; it must not execute a diagnostic while a frame is preflighted.
Unsupported work can return Incapable; failed or refused work has its own status.
Zero timeout leaves no time to execute or send a result.

`works` exposes task identities and complete information elements. Manufacturer
results retain repeated antennas, collocated radios and device types plus all
available identity/firmware/certificate values supplied by the integration.
Configuration results can contain multiple profiles, each identified by its
Profile ID. Unknown/vendor information and all credential values are retained.
Callers supply every available field required for the requested diagnostic type.
The owner validates complete results and packs whole report IEs across frames;
full storage or invalid results do not complete a task.

Association and IEEE 802.1X tests require `admitted_work` after the existing
association/security owner accepts the task. Successful execution reports must
name the requested AP and include the underlying 802.11 Status Code; an 802.1X
result also includes EAP method and credential types. Successful diagnostic
execution can report a failed association/authentication Status Code. These
protocol owners do not implement a second association or EAP stack.

A new dialog supersedes old work and pending reports. Remote Cancel discards
outstanding work; `cancel_work` reports local cancellation with status=4.
`poll` expires work and prevents a report from being admitted after the requested
timeout. `association_changed` normally cancels the dialog. Passing the causal
identity of an admitted active diagnostic preserves it across its own transition;
return to the original requesting AP with `reattached` and a new generation.
Stale RX/TX and retired task results cannot change the retained work.

`DiagnosticRequester` collects whole report frames throughout its response
window, correlates task/type/profile identities and rejects conflicting profile
results atomically. Incapable suppresses further requests of that type within
the same association. Remote cancellation is a new, explicitly tokened request.

Event and Diagnostic requests may end with a Destination URI IE. `delivery`
selects ordinary Action delivery while the requesting AP is reachable. URI
delivery becomes available only after the supplied continuous Beacon absence
covers the ESS Detection Interval in minutes. URI parsing, transport framing,
protection and execution belong to the integration; a diagnostic excursion
preserves the original requester and cannot send its report to the test AP.

## Directed Multicast Service

`DmsRegistry` is BSS-wide; each `DmsAccessPoint` is per peer. Prepare an owned
transaction from the AP's request identity and a caller's explicit resource/QoS
admission. Full response encoding and all membership changes succeed before
publication. `respond` verifies BSS, association/request identity and registry
revision, queues the response, then commits once. Stale transactions and full
storage cannot partially change membership. Replays do not commit again.

Add accepts multicast TCLAS types 0/1/4. Order-independent identical classifier
sets share a DMSID across STAs; complete QoS/vendor parameters remain per peer.
Change replaces parameters while keeping classifiers; Remove terminates even
when its peer has already removed the stream. Autonomous AP termination uses
dialog token zero. Denial alternatives are retained; modified hints are not
guessed to identify one of several denied Adds.

For an original group MSDU, `recipients` yields each matching STA once and
`group_delivery_required` checks the complete associated-peer list. The backend
must transmit an individually addressed A-MSDU to each recipient while retaining
the original group destination in its subframe. A group copy remains required
when any associated STA is not covered. Scheduling, QoS and physical buffer
ownership stay with the existing AP/data owners.

The station maintains its granted group-address list. Active grants discard
matching group copies; termination retains the supplied last group sequence for
the caller's bounded in-flight frame lifetime. Serial comparison handles wrap
and reports the ambiguous half-range explicitly. Unsupported and never-group-
transmitted sequence codes remain distinct; they do not promise duplicate
suppression. Lost admitted requests retain exact retry data and expose unknown
membership where applicable. An autonomous notice during Remove cannot extend
terminal history or discard known sequence evidence.

## Integration boundary

This implements the selected practical subset, not every original k/v service.
Location, other RRM types, FMS, timing measurement, MBO and multi-link WNM
extensions are not claimed. Vendor/later Event/Diagnostic payloads remain
opaque; their vendor-specific procedures are not provided.

ESP32-S31 roles/runtime do not yet route these exchanges or advertise support.
Actual measurements, protected management, scan/association coordination,
security/queue services and physical sleep/wake remain integration work.
The [canonical catalog](../../../../qualification/catalog/esp32s31/wifi-phy.toml)
marks these composed products partial. Host behavior and chip compilation do
not establish on-air interoperability or qualification.

The shared formats/procedures can be cross-checked against
[hostap definitions](https://github.com/freebsd/freebsd-src/blob/main/contrib/wpa/src/common/ieee802_11_defs.h),
[RRM handling](https://github.com/freebsd/freebsd-src/blob/main/contrib/wpa/wpa_supplicant/rrm.c)
and [WNM handling](https://github.com/freebsd/freebsd-src/blob/main/contrib/wpa/wpa_supplicant/wnm_sta.c).

Original Event/Diagnostic procedures and formats follow
[IEEE 802.11-2012, sections 8.4.2.69–72 and 10.23.2–3](https://mrncciew.com/wp-content/uploads/2014/10/ieee-802-11-2012.pdf).
Hostap definitions alone do not establish implementation of these report types
or resolve the frequent-transition layout conflict.
