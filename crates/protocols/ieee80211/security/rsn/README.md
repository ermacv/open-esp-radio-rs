# RSN security

`oer-ieee80211-rsn` holds the allocation-free, hardware-independent RSN (IEEE 802.11
robust security network) protocol shared by WPA2 and WPA3 suites. It owns no
executor, timer, key slot or transmit path; chip role crates execute its typed
transmit and key-install requests.

| Module | Responsibility |
| --- | --- |
| `akm` | Key descriptor version, PTK expansion and EAPOL-Key MIC of each `oer_ieee80211_mac::security::rsn::Akm` |
| `sae` | SAE commit/confirm, hunting-and-pecking and hash-to-element password elements, anti-clogging tokens |
| `bip`, `management_ccmp` | BIP-CMAC-128 for group-addressed and CCMP for individually addressed robust management frames |
| `element` | Association RSN element policy and selection of its first supported suite; wire syntax is `oer_ieee80211_mac::security::rsn` |
| `crypto` | Zeroizing PMK/PTK owners, PSK passphrase derivation and the association security binding |
| `eapol` | Validated borrowed/owned EAPOL-Key packets and MIC verification |
| `frames` | EAPOL-Key/Ethernet construction, GTK key data and owned security elements |
| `aes` | RFC 3394 key wrap and the asynchronous unwrap capability |
| `state` | Station and authenticator four-way-handshake transitions |
| `supplicant`, `runner` | Station orchestration, deadlines and key-publication ordering |
| `keys`, `retry` | Owned CCMP key installs and event-driven EAPOL retransmission |

A `Ptk` records the suite that derived it, so MIC generation and verification
cannot mix suites. The authenticator takes its suite from the station's
validated Association element; the supplicant takes it from its own
Association element. Both reject EAPOL-Key frames whose descriptor version
differs from that suite.

The implemented suites are PSK (`00-0F-AC:2`), PSK-SHA256 (`00-0F-AC:6`) and
SAE (`00-0F-AC:8`). The MAC package names them and the negotiated
association (`oer_ieee80211_mac::security::AssociationSecurity`); this crate
owns only their cryptography, each property an exhaustive match over the
suite.
