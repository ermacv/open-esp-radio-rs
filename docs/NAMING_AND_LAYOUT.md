# Naming and repository layout

This document defines the vocabulary and target repository shape used by new
code and structural migrations. Existing paths that differ from this document
are transitional; their names do not define new architectural terms.

## Repository roots

Every tracked component has one primary role:

| Root | Responsibility |
| --- | --- |
| `driver/` | Production radio code that may be linked by normal firmware |
| `verification/` | Vendor-code analysis and executable comparison assets |
| `qualification/` | Machine-readable readiness claims and immutable evidence records |
| `hil/` | Target and host infrastructure for execution on real hardware |
| `examples/` | User-facing firmware compositions |
| `tools/` | Repository-wide generators and policy checks without product behaviour |
| `svd/` | Editable register-description sources |
| `docs/` | Architecture and user guidance, not a second status database |

Production code never depends on `verification/`, `qualification/`, `hil/` or
`tools/`. Verification and HIL may depend on production code.

## Radio vocabulary

| Name | Meaning in this repository |
| --- | --- |
| `ieee80211` | Frame formats, information elements, codecs and protocol value types |
| `softmac` | Portable MAC policy and the contract presented to a chip MAC backend |
| `sta` | Station MLME and role policy |
| `ap` | Access-point MLME and role policy |
| `security` | RSN, WPA2/WPA3, handshake and key lifecycle policy |
| `mac` | A chip-specific Wi-Fi MAC backend when nested under a chip |
| `mac-hal` | Final semantic MAC hardware operations; not protocol policy |
| `phy` | RF/baseband/channel/calibration state machines |
| `radio` | Chip-wide RF power, clocks, calibration and shared hardware ownership |
| `coex` | Arbitration of shared radio resources between protocols |
| `adapter` | A narrow binding to an external ecosystem or network API |
| `runtime` | Executor, time, interrupt wake and task composition |

`HMAC` is not a public layer name because it is ambiguous with the
cryptographic keyed-hash construction already used by Wi-Fi security code.
`LMAC` may describe an internal split-MAC responsibility, but it is not used
for both a portable contract and a chip backend. New public names use
`softmac`, `mac`, `mac-hal` or `mac-backend` according to the actual role.

There is no portable `ieee80211-phy` layer. IEEE 802.11 frame semantics are
portable; the RF/baseband PHY is part of a concrete chip radio backend.

## Target production tree

```text
driver/
├── common/
│   └── dma/
├── wifi/
│   ├── ieee80211/
│   ├── softmac/
│   ├── sta/
│   ├── ap/                 # add with the first real AP owner
│   └── security/
├── ble/                    # add with an implementation
├── ieee802154/             # add with an implementation
├── chips/
│   ├── esp32s31/
│   │   ├── radio/
│   │   │   ├── pac/
│   │   │   ├── registers/
│   │   │   ├── hal/
│   │   │   ├── phy/
│   │   │   └── coex/
│   │   └── wifi/
│   │       ├── dma/
│   │       ├── mac/
│   │       └── sta/
│   └── esp32c5/
└── adapters/
    ├── embassy-net/
    ├── embassy/
    │   ├── esp32s31-platform/
    │   └── esp32s31-wifi/
    └── esp-hal/
        └── esp32s31-wifi/
```

Chip-wide PHY and radio code stays outside `wifi/` because BLE and IEEE
802.15.4 may consume common RF calibration, clocks and power ownership.
Protocol-specific MAC semantics remain separate.

ESP32-C5 is introduced as a peer concrete backend. Code is promoted into a
cross-chip crate only after both backends demonstrate the same semantic
operation. Equal register offsets or vendor symbol names alone do not prove a
shared abstraction.

## Verification, qualification and HIL

```text
verification/
└── vendor/
    ├── engine/
    └── targets/esp32s31/
        ├── project.toml
        ├── target.toml
        ├── memory.toml
        ├── interfaces/
        ├── profiles/
        ├── dispositions/
        ├── baselines/
        ├── harness/
        └── probes/

qualification/
└── targets/esp32s31/
    ├── wifi-sta.ledger
    └── records/

hil/
├── protocol/
├── host/
│   ├── runner/
│   └── linux-net/
└── targets/esp32s31/
    ├── board/
    ├── bootstrap/
    ├── runtime/
    ├── telemetry/
    ├── linker/
    └── partitions/
```

The terms have separate evidence strength:

- **analysis** discovers symbols, interfaces, IR and possible MMIO effects;
- **verification** compares an implementation with an explicit reference or
  executable effect contract;
- **qualification** makes a readiness claim from implementation, host proof,
  vendor proof, bounded scheduling and current HIL evidence;
- **HIL** records what executed on a named hardware setup;
- **oracle** is a caller-owned vendor input, not an identity embedded in a
  production crate or generic verification engine.

## Migration rules

1. Do not combine a path rename with behavioural changes.
2. Preserve a compatibility re-export for one migration step when a public
   Rust path changes.
3. Move existing components before creating empty AP, BLE, IEEE 802.15.4 or
   coexistence crates.
4. A facade is introduced only when it owns a stable high-level API. A crate
   that merely re-exports every internal package is not an application API.
5. Qualification manifests are the source of current readiness. Markdown may
   summarize them but must not become an independent status database.
6. Wire/image CRCs protect transport integrity. Private vendor artifact
   identities belong to caller-owned verification configuration.
7. New facade code uses `adapters` and `adapter-*`. The former `integration`
   namespace and `integration-*` feature names are compatibility aliases only.
