# Architecture

The workspace separates chip-independent protocols from ESP32-S31 hardware
ownership. Dependencies point toward lower-level capabilities; executor and
board policy stay outside the core driver layers.

```text
open-esp-radio (facade)
├── Wi-Fi protocols
│   ├── open-esp-radio-ieee80211
│   ├── open-esp-radio-wpa2
│   └── open-esp-radio-embassy-net
└── ESP32-S31
    ├── wifi-mac ───────────┬──> ieee80211
    │                       └──> pac ──> svd
    └── phy ──> hal ───────────> pac ──> svd

open-esp-radio-esp32s31-wifi-esp-hal ──> facade + esp-hal
HIL runtime                         ──> facade + esp-hal adapter
```

## Layer responsibilities

| Layer | Owns | Must not own |
| --- | --- | --- |
| `svd` | Generated register types | Driver policy or handwritten fixes |
| `pac` | Peripheral singleton, register geometry, finite transactions | Async scheduling or protocol state |
| `hal` | Typed power/clock/I2C/PBus operations and hardware boundaries | Board startup or an executor |
| `phy` | Shared RF/baseband calibration plus the currently qualified Wi-Fi profile | Protocol MAC state or raw peripheral ownership |
| `wifi-mac` | 802.11 DMA descriptors, RX/TX state, interrupts, EDCA, BlockAck and rates | PHY calibration or non-Wi-Fi MAC policy |
| `ieee80211` | Portable frame formats and protocol state | Chip registers |
| `wpa2` | Portable WPA2-Personal state and key material | Radio hardware |
| `embassy-net` | Bounded network-stack ownership adapter | ESP32-S31 details |
| `radio` | Public composition and re-exports | Board/bootstrap policy |

## Ownership

One Rust value owns the live radio state. Cold-init transitions move state into
child transitions and recover it at terminal states. Hardware operations use
non-cloneable capability values; there is no implicit C callback table or
C-owned parameter block in the source-only profile.

The generated register singleton is acquired in the PAC and exposed upward as
narrow semantic operations. MAC descriptor memory is a distinct ownership
domain: its volatile cells are shared memory, not peripheral registers.

## Multiple chips and radio protocols

ESP32-S31 is the first backend, not the project boundary. New chips belong
under `crates/<chip>/`; portable protocol logic belongs under
`crates/<protocol>/`. A chip/protocol-specific implementation belongs under
`crates/<chip>/<protocol>/`, as the current Wi-Fi MAC does.

Bluetooth/BLE, IEEE 802.15.4 and coexistence must not be added to the Wi-Fi MAC
crate. They will have their own protocol and chip-specific layers. `pac`, `hal`
and `phy` remain at chip scope because the current evidence already includes
shared RF calibration and separate Wi-Fi/BT gain banks. This is a reuse
candidate, not a promise of a universal PHY API: shared pieces should be
extracted only after a second protocol or chip supplies concrete requirements.

The current PHY entry path is qualified through Wi-Fi HIL. APIs that encode a
Wi-Fi-only mode or table should keep that name explicit so future Bluetooth,
802.15.4 and coexistence work can use the shared RF core without inheriting
Wi-Fi policy.

## Waiting and integration

Finite arithmetic runs immediately. Real delays and readiness edges are
represented through async ports. PAC and HAL do not depend on Embassy or any
other executor. The optional `esp-hal` adapter binds platform singleton tokens;
the HIL workspace owns task spawning, linker placement, flashing and test
policy.

## Transitional debt

`crates/esp32s31/phy/src/radio_hal.rs` still contains finite MMIO leaves whose
final home is HAL/PAC. Moving them is structural work and must preserve PHY
state-machine behaviour. The current boundary and remaining unsafe inventory
are recorded in [the PAC audit](PAC_AND_UNSAFE_AUDIT.md).
