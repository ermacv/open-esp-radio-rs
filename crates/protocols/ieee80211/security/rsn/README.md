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
| `state` | Station and authenticator four-way-handshake transitions |
| `supplicant` | Station orchestration: typed key-data unwrap and key-install requests and their completions, response deadlines |
| `runner` | Ports and values of the handshake and key-install runners |
| `keys`, `retry` | Owned CCMP key installs and event-driven EAPOL retransmission |

A `Ptk` records the suite that derived it, so MIC generation and verification
cannot mix suites. The authenticator takes its suite from the station's
validated Association element; the supplicant takes it from its own
Association element. Both reject EAPOL-Key frames whose descriptor version
differs from that suite.

`CcmpKey` owns its temporal-key bytes, redacts Debug output and zeroizes its
storage on drop. Pending actions and retained station records use the same
owner; adapters and comparisons borrow the bytes. This covers those owners,
not arbitrary compiler temporaries.

GTK KDE parsing retains `GroupKeyId` in `RsnGtk` and `RsnKeyKind::Group`;
IDs outside the two-bit wire domain never reach a key installation. The
recorded M3 regression in `frames/captured_m3.rs` authenticates and unwraps
frame 8 of Wireshark's pinned public `wpa2-psk-mfp` capture. Derived KDE cases
change only its ID to exercise all four wire values, including zero.

The portable station can retain a completed key installation while waiting for
M4 delivery. `RsnStaSupplicant` authenticates a repeated M3 before advancing its
counter. The station retains the installed PTK and processes only changed GTK
material from a newer M3; an exact repeat produces M4 without an install.
`RsnStaReady::RetryInstall` carries `RsnStaRetryInstall`, which has no pairwise
field. `RsnConnectedSupplicant::on_message3` authenticates exact and newer
repeats with the retained PTK and produces only M4 or a group unwrap request.
The request binds the authenticated M3, including its full ANonce; its GTK/IGTK
completion cannot install PTK. Unwrap completion yields `RsnStaReady`, a type
without another KEK request, so callers exhaustively handle all remaining work.
A connected unwrap request owns its authenticated frame and a zeroizing KEK
copy. It has no busy latch or wrapping ticket: validation of a successor can
proceed while the station holds the current TX. Unwrap and install completions
check the committed replay frontier, so an overtaken request cannot roll it
back. The station owns the bounded deferred-frame policy and TX ordering.


`BipTransmitter` refuses a frame when its 48-bit IPN space is exhausted, or
when the output has no room for the MMIE; refusal consumes no IPN and leaves
the frame unchanged. The IGTK epoch's owner retains this state across an AP
restart or channel move rather than reusing IPNs under the same key.

The implemented suites are PSK (`00-0F-AC:2`), PSK-SHA256 (`00-0F-AC:6`) and
SAE (`00-0F-AC:8`). The MAC package names them and the negotiated
association (`oer_ieee80211_mac::security::AssociationSecurity`); this crate
owns only their cryptography, each property an exhaustive match over the
suite.

The strict-lint milestone [#751](https://github.com/ermacv/open-esp-radio-rs/issues/751)
requires boundary constructors to retain checked fixed-size fields as array
references obtained with `split_first_chunk`. Variable-length envelopes must
establish each boundary before those fields reach crypto and handshake
transitions. Runtime expect/unwrap paths become retained type proofs or typed
peer-input refusals, without mechanically replacing indexing with `.get()`
or suppressing the lint. Policy is enabled at the crate root when that
migration is complete, before step 2 of #563 merges.
