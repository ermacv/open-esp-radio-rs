# Architecture

The workspace separates chip-independent protocols from ESP32-S31 hardware
ownership. Dependencies point toward lower-level capabilities; executor and
board policy stay outside the core driver layers.

```text
open-esp-radio (facade)
├── Wi-Fi protocols
│   ├── open-esp-radio-ieee80211
│   └── open-esp-radio-wpa2
└── ESP32-S31
    ├── wifi-mac ───────────┬──> ieee80211
    │                       └──> pac ──> svd
    └── phy ──> hal ───────────> pac ──> svd

Reusable integration
├── embassy-net adapter ──> embassy-net-driver + embassy-sync
├── S31 Wi-Fi/Embassy   ──> adapter + S31 Wi-Fi MAC/HAL/PAC
└── S31 Wi-Fi/esp-hal   ──> S31 Wi-Fi MAC/PHY/HAL + esp-hal

Test harness (HIL)
└── board + clocks + PSRAM/flash + executor + embassy-net/smoltcp
    └──> facade + reusable integration
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
| `integration/network/embassy-net` | Bounded `embassy-net-driver` ownership adapter | A concrete chip or full network stack |
| `integration/esp32s31/wifi-embassy` | Wi-Fi DMA/network leases, async TX and IRQ wakeups | Board startup or network policy |
| `integration/esp32s31/wifi-esp-hal` | `esp-hal` singleton binding for the ESP32-S31 Wi-Fi backend | Board, PSRAM/flash or executor policy |
| `radio` | Public composition and re-exports | Board/bootstrap policy |
| `hil/esp32s31` | Test board clocks, boot, memory placement, executor, real `embassy-net`/smoltcp scenarios | Reusable radio implementation |

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

Portable Wi-Fi remains under `crates/wifi/`; future portable Bluetooth and
IEEE 802.15.4 code should receive peer protocol roots. Chip implementations
belong under `crates/<chip>/<protocol>/`. Coexistence policy that coordinates
physical radios belongs at chip scope rather than inside any one protocol MAC.

The hierarchy is deliberately chip-first: `crates/esp32s31/phy`, not
`crates/phy/esp32s31`. PAC, HAL, PHY, Wi-Fi, future Bluetooth/802.15.4 and
coexistence code all need one coherent view of a chip. Putting chips under a
generic PHY root would also encourage chip feature switches throughout one
crate. When a second chip demonstrates genuinely identical algorithms, move
those algorithms into a small trait-parameterized shared crate rather than
selecting whole backends with `cfg(esp32s31)`.

The current PHY entry path is qualified through Wi-Fi HIL. APIs that encode a
Wi-Fi-only mode or table should keep that name explicit so future Bluetooth,
802.15.4 and coexistence work can use the shared RF core without inheriting
Wi-Fi policy.

## Waiting and integration

Finite arithmetic runs immediately. Real delays and readiness edges are
represented through async ports. PAC and HAL do not depend on Embassy or any
other executor. The optional `esp-hal` adapter binds platform singleton tokens
and is reusable by non-test firmware. The separate Wi-Fi/Embassy crate owns
executor-specific radio composition. Neither is the test harness: the HIL
workspace alone owns board clocks and boot, PSRAM/flash placement, task
spawning, the concrete `embassy-net` stack (which uses smoltcp), flashing and
test policy.

The facade features preserve those boundaries. `wifi` selects only portable
802.11/WPA2 code, `esp32s31-wifi` adds the hardware Wi-Fi backend,
`integration-embassy-net` adds only the generic network adapter, and
`esp32s31-wifi-embassy` opts into the complete S31 Wi-Fi/Embassy composition.

## Transitional debt

`crates/esp32s31/phy/src/radio_hal.rs` still contains finite MMIO leaves whose
final home is HAL/PAC. Moving them is structural work and must preserve PHY
state-machine behaviour. The current boundary and remaining unsafe inventory
are recorded in [the PAC audit](PAC_AND_UNSAFE_AUDIT.md).
