# IEEE 802.11 protocol boundaries

These packages contain portable MAC mechanisms, role policy, service contracts
and security. Hardware representation and radio execution live below and above
them respectively; they are not dependencies of the portable protocol policy.

Within the MAC package, formats and retained state are separated inside the
protocol that owns them:

| Module under `mac/src/` | Responsibility |
| --- | --- |
| `block_ack/frame` | Stateless Block Ack Action parsing and wire identifiers |
| `block_ack/session` | One TX agreement, its negotiation generation and alarm handling |
| `fragmentation` | Validated fragment identities and the shared fragment contract |
| `fragmentation/parsing` | Header/body validation without reassembly ownership |
| `fragmentation/reassembly` | Complete bounded reassembler, slots and admission tokens |
| `ap/profile` | Explicit advertisement values; the chip AP profile selects rates, capabilities and WMM parameters |
| `station/association` | Association capability types, validation and encoding |
| `station/management` | Probe/authentication management codecs |
| `station/security` | Security IE parsing and validation |
| `station/data` | Data codecs, with A-MSDU framing in `data/amsdu` |
| `station/sequence` | Separate management/non-QoS and per-TID TX sequence owners |
| `data/duplicate` | Association/peer-scoped receive retry history |
| `qos` | Typed traffic intent, UP/AC and DSCP classification helpers |
| `extensions/wmm` | WMM AC parameters and vendor IE parsing |
| `extensions/espressif/esp_now` | ESP-NOW v1/v2 framing and protected-envelope validation |
| `extensions/espressif/esp_now/v2/reassembly` | Caller-owned storage for a validated v2 datagram |

The public Block Ack, fragmentation, station and data namespaces expose their
protocol contracts. Root `wmm` and `esp_now` imports are compatibility exports
of the canonical modules. Production consumers use the explicit QoS, WMM and
ESP-NOW paths.

QoS classification includes the existing DSCP mapping and downgrade helpers.
The actual admission/downgrade loop still belongs to chip MAC TX runtime;
parsing an advertised WMM Parameter Set neither acquires admission nor selects
a hardware queue. AP encoders take an explicit `Advertisement`;
`chips/esp32s31/ieee80211/ap/src/profile.rs` selects the existing hardware
advertisement. The portable codec carries no implicit ESP32-S31 profile.

`softmac/src/contract` describes operation ownership, service capabilities,
resource limits and normalized statuses. Configuration, VIF and monitor
contracts remain explicit sibling modules. ESP-NOW peer/protocol owners live
in `softmac/src/extensions/espressif/esp_now/protocol`; secrets, peer generations
and replay state live in its `security` sibling. They use lower MAC codecs.
The MAC package must not depend back on SoftMAC peer or security policy.

WPA2 `crypto` retains secret ownership, derivation and zeroization; `eapol`
holds packet views and MIC handling. `frames/{security_ies,key_data,transmit}`
separates wire formats, while `state/{supplicant,authenticator}` retains the
complete handshake owners. Existing root exports preserve the caller contract.

Portable AP `service` retains one peer storage owner and separates `peer`,
`security`, `block_ack` and `power_save` operations into child modules.
Chip AP `engine` retains the hardware/service/key/beacon owner and separates
`management`, `tx`, `rx` and `power_save` operations without duplicating state.

Module namespaces do not create independent owners. Tests live in child files
beside their owner or at the shared protocol boundary. See the
[driver map](../README.md) for the complete layering contract.
