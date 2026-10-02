# RSN security

`oer-ieee80211-rsn` holds the allocation-free, hardware-independent RSN (IEEE 802.11
robust security network) protocol shared by WPA2 and WPA3 suites. It is
sans-IO and owns no executor, timer, key slot or transmit path: chip role
crates execute its typed key-data unwrap, transmit and key-install requests,
and the handshake runners of
[`oer-ieee80211-rsn-service`](../../../../services/ieee80211/rsn/README.md)
wait on its ports.

| Module | Responsibility |
| --- | --- |
| `akm` | Key descriptor version, PTK expansion and EAPOL-Key MIC of each `oer_ieee80211_mac::security::rsn::Akm` |
| `sae` | SAE commit/confirm, hunting-and-pecking and hash-to-element password elements, anti-clogging tokens |
| `bip`, `management_ccmp` | BIP-CMAC-128 for group-addressed and CCMP for individually addressed robust management frames |
| `element` | Association RSN element policy and selection of its first supported suite; wire syntax is `oer_ieee80211_mac::security::rsn` |
| `crypto` | Zeroizing PMK/PTK owners, PSK passphrase derivation and the association security binding |
| `eapol` | Validated borrowed/owned EAPOL-Key packets and MIC verification |
| `frames` | EAPOL-Key/Ethernet construction, GTK key data and owned security elements |
| `aes` | RFC 3394 key wrap and unwrap and the asynchronous unwrap port |
| `state` | Shared station and authenticator four-way transitions, typed by ordinary, FT or OWE suite |
| `ft` | FT key hierarchy, initial association binding, STA/AP transitions, DS relay and key-holder contracts |
| `owe` | OWE key hierarchy, association/four-way binding, scoped PMKSA lifetime and reciprocal transition discovery |
| `supplicant` | Station orchestration: typed key-data unwrap and key-install requests and their completions, response deadlines |
| `runner` | Ports and values of the handshake and key-install runners |
| `keys`, `retry` | Owned CCMP key installs and event-driven EAPOL retransmission |

A `Ptk` records the suite that derived it, so MIC generation and verification
cannot mix suites. The authenticator takes its suite from the station's
validated Association element; the supplicant takes it from its own
Association element. Both reject EAPOL-Key frames whose descriptor version
differs from that suite.

The ordinary RSN suites are PSK (`00-0F-AC:2`), PSK-SHA256 (`00-0F-AC:6`) and
SAE (`00-0F-AC:8`). The MAC package names them and the negotiated
association (`oer_ieee80211_mac::security::AssociationSecurity`); this crate
owns only their cryptography, each property an exhaustive match over the
suite.

## Opportunistic Wireless Encryption

The MAC [`owe` module](../../mac/src/owe.rs) owns DH Parameter and Transition
Mode IE framing. `Group` names groups 19/20/21 and their compact x-coordinate
widths. Unknown group numbers remain observable for an AP rejection; a known
group must have its exact public-key width.

[`owe`](src/owe.rs) keeps the full 32/48/64-octet PMK and group-specific
KCK/KEK geometry in zeroizing owners. It uses the existing RSN state machines
with `Group`, HMAC-SHA-256/384/512, and the shared RFC 3394 AES-128/AES-256
wrap implementation. Wider MICs require `EapolKeyFrame::parse_with_mic_length`
or `OwnedEapolFrame::try_copy_with_mic_length`; the legacy entry points select
16 octets. Frame authentication validates the retained suite identity and
returns a `Result`, including for legacy `Ptk` keys.

`KeyPair` uses RustCrypto's P-256/P-384/P-521 arithmetic for compact DH.
The caller supplies fresh uniformly sampled scalar octets; the constructor
rejects zero, out-of-range and incorrectly sized scalars. Finishing consumes
the private key, validates the peer point, and derives a role-bound PMK.

OWE `InitialAssociation` borrows the exact advertisement and selected
security fields. It binds Message 2 to Association Request and verifies
Message 3's advertised RSNE/RSNXE and GTK/IGTK before returning keys. PMF
negotiation rejects contradictory MFPR/MFPC and incompatible requirements.
The cache scope includes both addresses, SSID, group and negotiated PMF;
expiry and monotonic time are explicit. A full cache does not evict a live
entry implicitly. Cache resumption requires an offered, echoed, live PMKID;
a matching PMKID makes the response DH body irrelevant under RFC 8110 4.5.

`TransitionPair` constructs both reciprocal advertisements from one pair
of identities. `TransitionTarget` identifies a directed search and validates
the observed OWE counterpart, including an explicitly hidden SSID. Channel
hints do not bypass regulatory admission. Open-mode fallback, scan scheduling
and association execution belong to caller policy.

`owe::Station` and `owe::AccessPoint` own association DH/cache selection,
stable retries, explicit group preferences, monotonic deadlines and
role/generation/transmission tickets. Status 77 requests a fresh scalar for
the next configured group; invalid keys terminate the attempt. AP admission
checks policy before ECC and releases its four-way binding only once, after
an acknowledged Association Response transmission. A duplicate request
retains the original public key and does not release another PMK. The caller
captures the request ticket when dispatching a STA response and separately
admits its MAC addresses, SSID and Open System Authentication. `next_deadline_us`
supplies wakeups for a caller-owned clock; no executor or radio is imported.

Chip-role OWE selection, MLME composition and key execution remain unconnected.
Portable protocol coverage does not constitute Enhanced Open product support.


## Fast BSS Transition

[`ft`](src/ft.rs) owns FT-PSK, FT-SAE and FT-802.1X with the SHA-256
hierarchy and CCMP-128/BIP-CMAC-128. FT suites are a separate `FtAkm` type:
an ordinary PMK expansion cannot accidentally derive an FT PTK. The MAC
[`ft` module](../../mac/src/ft.rs) owns MDIE, FTIE, RIC and timeout wire
syntax and Authentication/Action frame bodies. The shared management TLV
parser remains below both FT and k/v. No new endian or packet-reading
framework is involved.

`InitialAssociation` binds the original Association Response's MDIE and
FTIE and the caller's selected Association Request security to
`RsnStaState<FtAkm>` or `RsnApState<FtAkm>`. The existing state actions still
order derivation, MIC verification, key-data parsing, key installation and
EAPOL transmission. Its Message 2 inserts PMKR1Name; Message 3 validates that
name and the exact advertised RSNE/RSNXE and original MDIE/FTIE before
returning the existing zeroizing GTK/IGTK owners. Reassociation deadlines
are TUs; key lifetimes are seconds. The association owner completes Message
4 delivery before retaining the root for later roaming.

`InitialKey` requires a PSK, a PMK from successful SAE with its explicit
`SaePwe`, or the complete MSK from successful external EAP. FT-802.1X uses
MSK octets 32..64, while ordinary EAP PMKs use octets 0..32. This crate
implements no EAP exchange and consumes no unauthenticated SAE result.

For roaming, `Station` and one-peer `AccessPoint` expose borrowed pending
transmissions and explicit operation IDs. Every admitted operation gets
one terminal completion. The caller supplies a fresh cryptographic SNonce
or ANonce for each new exchange, one monotonic microsecond epoch, nonzero
timeouts and an association generation it never reuses. Retries keep the
same nonce and authenticated bytes, receive a fresh operation ID, and never
extend the original deadline. Untrusted frame errors leave the current
exchange available for a valid retry. The caller continues polling until
a terminal event or the next explicit deadline.

The target AP obtains PMK-R1 through `KeyLookup` and `KeyDelivery`, validates
the Reassociation MIC and asks the resource owner to answer every RIC
request. The AP service supplies its existing GTK/IGTK. It installs the
pairwise key and association state before sending success; the station
validates and unwraps all response keys before its own atomic transaction.
A failed transaction must roll back all side effects. Cancellation or
expiry after admission returns `RollbackRequired`, and the owner stays
busy until `rollback_completed`. Before reporting rollback, the caller
finishes or cancels admitted hardware work so no late operation can restore
the removed keys. Connected key owners transfer once;
a cached AP response answers identical retries without reinstalling keys.
RIC refusal is an explicit error: the association owner sends a failing
Reassociation status and cancels the uncommitted FT exchange.

`RootKeyHolder` and `R1KeyHolder` expire keys and reject a full cache rather
than evicting live entries. `AuthorizedKeyRequest` requires the caller's
configured target/R1KH authorization; key-holder IDs in a radio frame confer
no trust. The caller seals and authenticates key grants over its
backhaul. A grant carries remaining lifetime, not the source AP's clock:
`KeyDelivery::from_authenticated_grant` subtracts an explicit upper bound on
queue/transport age and creates expiry in the recipient's epoch. The
requesting AP alone checks its operation deadline; clocks on different APs
need not share an epoch. `DsRelay` forwards a station's FT Action exchange between the
current and target AP, matches operation, addresses, domain and nonce, and
holds no keys. Backhaul queuing, authentication, retries and framing stay
with that transport owner; relay cancellation invalidates its operation.

These interfaces have no executor, allocation, radio calls or autonomous
clock. Chip FT negotiation/advertisement, association migration, frame
routing and hardware transaction adapters are not implemented. SHA-384,
SAE-EXT-KEY, OCI/BIGTK and multi-link FT layouts are outside these three AKMs.
Protocol vectors and simulated exchanges do not establish on-air readiness.

Wire, KDF and integrity behavior follows the
[hostap common FT implementation](https://android.googlesource.com/platform/external/wpa_supplicant_8/+/6a7451e1ba172b3574014ff76619a469f2f3eda6/src/common/wpa_common.c),
[station FT procedure](https://android.googlesource.com/platform/external/wpa_supplicant_8/+/6a7451e1ba172b3574014ff76619a469f2f3eda6/src/rsn_supp/wpa_ft.c)
and [AP FT procedure](https://android.googlesource.com/platform/external/wpa_supplicant_8/+/6a7451e1ba172b3574014ff76619a469f2f3eda6/src/ap/wpa_auth_ft.c).
