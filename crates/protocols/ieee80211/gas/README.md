# GAS and ANQP protocol logic

`oer-ieee80211-gas` implements bounded, allocation-free GAS requester/responder
dialogs and ANQP query handling. It is `no_std` and sans IO. Frames, monotonic
time, provider facts and terminal transmission results are supplied inputs;
the crate has no radio, executor, filesystem or network dependency.

## Ownership

| Owner | Responsibility |
| --- | --- |
| [`mac::gas`](../mac/src/gas.rs) | Advertisement Protocol IE and all four GAS Action formats, categories, status and fragment vocabulary |
| [`mac::anqp`](../mac/src/anqp.rs) | Sixteen-bit ANQP envelopes, base records and Hotspot 2.0 Release 1–3 records |
| [`requester`](src/requester.rs) | One peer query, fixed deadline, token quarantine, delayed Comeback, bounded assembly and whole-query restarts |
| [`responder`](src/responder.rs) | Concurrent bounded dialogs, provider handoff, fragment cursor, MAC retry response cache and replay leases |
| [`anqp`](src/anqp.rs) | Request expansion, explicit provider resolutions, Home Realm filtering, response correlation and OSU/NAI consistency |
| [`anqp::Requester`](src/anqp/client.rs) | Protocol-0 composition; validates completed ANQP before reporting semantic success |

MAC formats reuse the existing bounded byte cursors and management-element
parser. Endian conversion uses Rust's integer methods at each field. Shared
Public Action categories, addresses, SSID limits, sequence numbers and TU
duration stay with their MAC owners. GAS treats advertisement-protocol identity
as data; it does not depend on ANQP network records or roaming associations.

The management integration owns channel access, addresses, MAC headers and
sequence allocation, fragmentation below GAS, protection and radio buffers.
It validates the permitted Public/Protected Dual category and routes the
original peer context through queued events. A `PeerIdentity` epoch distinguishes
owner incarnations; concurrent owners that share event routing use distinct
epochs. Recreating an owner requires retiring its queued work and quiescing old
peer responses before reusing its wire tokens. Relabeling an old RX event with
a new epoch does not establish freshness.

## Driving GAS

1. Construct a requester with a validated peer context, explicit timeout/restart
   policy and `token_reuse_guard`. Construct a responder for one advertisement
   protocol with explicit provider/delivery lease, fragment limit, Comeback
   delay and response-length advertisement.
2. Start `request` or deliver a validated Action body to `receive`. Responder
   receive also takes the original MAC `SequenceNumber` and Retry bit. Deliver
   duplicate requests to this owner so it can replay their saved response.
3. Copy each `Transmission` body into integration-owned TX storage. Call
   `admitted(id, now)` only after that storage is accepted. Protocol storage is
   never lent to DMA. Backend retries remain one admitted submission.
4. Feed the terminal `tx_completed` result. A peer response or next Comeback
   may arrive before that result. Superseded completions cannot change newer
   work. Response cursor advancement requires an ACK or a new Comeback proving
   reception; merely publishing bytes does not advance it.
5. Drive `next_deadline` and `poll` with nondecreasing time. Drain responder
   expiry events repeatedly at that time. A provider can `defer`, later
   `respond` with a complete immutable payload, or `reject` explicitly.
6. Drain the requester's terminal outcome before starting another query.
   `cancel` retires the operation; late provider results and TX completions
   cannot revive it. Responder cancellation also releases its replay slot, so
   the caller must retire related MAC retries before accepting new work there.

Comeback Requests contain no requested fragment number. A lost Comeback
Response therefore restarts the entire query with a different available token;
it does not send a fresh same-token request that could skip a fragment. A
Retry-marked request with the same MAC sequence replays the saved bytes.
Sequence equality with Retry clear is new work, including sequence wrap.
Older retries cannot prove receipt of a newer response. MAC receive admission
keeps retry ordering within the shared sequence comparison horizon and retires
superseded backend retry work; an exactly half-space comparison is an explicit
error rather than guessed ordering.
Repeated provider delays are accepted without extending the fixed deadline.
Comeback status 95 is nonterminal and can carry fragments, matching hostap's
requester behavior; other failure statuses do not publish response data.
An unknown-dialog failure can advertise the responder's service identity;
terminal rejection does not require a protocol match that the responder cannot
recover from a Comeback Request alone.

Each retired token remains unavailable until the original dialog deadline plus
the supplied guard. Integration sets that guard to cover peer response/cache
lifetime beyond the local deadline and retained MAC RX work. Exhausting the
eight-bit space reports the next reusable-token deadline rather than requiring
an owner reset. A restart waiting for a token remains bounded by its original
deadline. No finite peer/radio lifetime is assumed by default.

## ANQP records and providers

Base formats cover Query/Capability Lists, Venue Name, Emergency Call Number,
Network Authentication Type, Roaming Consortium, IP Address Availability, NAI
Realm/EAP methods and parameters, 3GPP Cellular Network, geospatial/civic
location, Location URI, Domain Name, Emergency Alert URI/NAI, TDLS Capability,
Neighbor Report and vendor-specific records. Venue URL is also supported.
Externally defined 3GPP content, location report representations, URI execution
and TDLS Peer Information remain exact borrowed producer data; this crate does
not generate measurements or run those external procedures.

Hotspot 2.0 ANQP supports Query/Capability Lists, Operator Friendly Name, WAN
Metrics, Connection Capability, NAI Home Realm Query, Operating Class, OSU
Providers List, Icon Request/Binary File, Operator Icon Metadata and OSU
Providers NAI List. Nested lengths, counts, text and final tails are checked.
OSU SSIDs remain binary. Provider order is preserved; when both OSU provider
and NAI lists are returned their counts must agree. Terms and Conditions
notifications are WNM messages and are outside this ANQP owner.

After a responder `Query` event, parse `query(id)` with `anqp::Query::parse` and
resolve every item from `requests()` in order. Pass that resolution slice to
`anqp::Response::prepare`, then pass the resulting bytes to the GAS responder's
`respond`. Provider work may be deferred; both dialogs and payload storage are
bounded independently. Malformed queries have an explicit parse error; the
integration chooses its GAS rejection policy rather than fabricating records.

`Resolution::Unsupported` explicitly means the service does not implement an
identifier. Implemented but unconfigured information needs a record containing
the standard's absent optional fields and any mandatory provider facts. A
missing icon uses an Icon Binary File failure status, not a silent omission.
Filenames are exact provider lookup keys; this crate never interprets them as
filesystem paths. No provider record, unknown extension or oversized response
is silently truncated or evicted.

A Home Realm query filters complete configured NAI Realm records by encoding
and exact semicolon-separated name equality, retaining their EAP parameters.
Its answer is a base NAI Realm element. A full NAI Realm query takes precedence
when both are present. Credential eligibility and domain-selection policy
remain separate from these wire/procedure checks.

`anqp::Requester` retains the query snapshot and composes the GAS requester.
Its terminal outcome distinguishes valid ANQP, transport rejection/timeout and
invalid ANQP. Unrequested/duplicate records, unmatched Home Realms and
inconsistent OSU/NAI counts are never exposed as a successful ANQP result.

## Bounds and integration limits

Requester `FRAME` bounds outgoing Action bodies and retained individual RX
bodies; `RESPONSE` bounds complete assembly. Responder `SLOTS`, `QUERY`,
`RESPONSE` and `FRAME` bound concurrent leases, provider queries, complete
snapshots and cached replies. Replay caches consume slots until their fixed
lease expires. The token quarantine table contains one `Instant` for each of
the 256 wire values. ANQP response preparation uses a bounded private snapshot
and, for Home Realm filtering, an additional `SIZE`-octet scratch buffer.
Insufficient storage, wire-length overflow and exhausted slots are explicit
errors. GAS's fragment count and advertised response limit also apply.

ESP32-S31 management routing, pre-association channel scheduling, beacon/probe
advertisement and Protected Dual protection are not connected to these owners.
The source implements protocol logic; it does not establish on-air
interoperability, Passpoint credential/profile selection, OSU HTTP procedures,
enterprise connection composition or certification.

## Format references

Wire facts follow IEEE 802.11-2012 sections 8.4.2.95, 8.4.4 and 8.5.8.12–15.
Implementation references are upstream hostap's
[`gas.c`](https://raw.githubusercontent.com/freebsd/freebsd-src/main/contrib/wpa/src/common/gas.c),
[`gas_query.c`](https://raw.githubusercontent.com/freebsd/freebsd-src/main/contrib/wpa/wpa_supplicant/gas_query.c),
[`gas_serv.c`](https://raw.githubusercontent.com/freebsd/freebsd-src/main/contrib/wpa/src/ap/gas_serv.c)
and the Hotspot ANQP identifiers in
[`ieee802_11_defs.h`](https://android.googlesource.com/platform/external/wpa_supplicant_8/+/b6f94540c451270a44d28e782088421aaac5e31c/src/common/ieee802_11_defs.h).

Focused host regression tests and portable compilation:

```console
cargo test -p oer-ieee80211-mac -p oer-ieee80211-gas -p oer-ieee80211-roaming
cargo check -p oer-ieee80211-gas --target riscv32imafc-unknown-none-elf
```
