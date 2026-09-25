# RSN security

`oer-ieee80211-rsn` holds the allocation-free, hardware-independent RSN (IEEE 802.11
robust security network) protocol shared by WPA2 and WPA3 suites. It owns no
executor, timer, key slot or transmit path; chip role crates execute its typed
transmit and key-install requests.

| Module | Responsibility |
| --- | --- |
| `akm` | `Akm`, the negotiated suite: selector, key descriptor version, PTK expansion and EAPOL-Key MIC |
| `element` | Association RSN element validation and selection of its first supported suite |
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

PSK (`00-0F-AC:2`) is the only implemented suite. Adding one (for example SAE,
`00-0F-AC:8`) means adding an `Akm` variant; every property is an exhaustive
match, so the compiler lists each decision the new suite must make. PMK
establishment that is not a passphrase derivation, such as the SAE exchange,
and management-frame protection are separate additions.
