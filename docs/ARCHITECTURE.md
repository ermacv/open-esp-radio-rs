# Architecture

The physical product boundary is [`driver/`](../driver/README.md). It separates
chip-independent protocols from hardware ownership. `hil/`, `validation/` and
`tools/` are consumers of the driver, never dependencies of it. Dependencies
point toward lower-level capabilities; executor and board policy stay outside
the core driver layers.

```text
open-esp-radio (facade)
├── Wi-Fi protocols
│   ├── open-esp-radio-ieee80211
│   ├── open-esp-radio-wifi-lmac
│   ├── open-esp-radio-wifi-sta
│   └── open-esp-radio-wpa2
└── ESP32-S31
    ├── wifi-sta ──> wifi-lmac + phy
    ├── wifi-lmac ──────────┬──> lmac contract ──> ieee80211
    │                       └──> registers ──> generated PAC
    └── phy ──> hal ───────────> registers ──> generated PAC

Reusable integration
├── embassy-net adapter ──> embassy-net-driver + embassy-sync
├── S31 Wi-Fi/Embassy   ──> adapter + S31 Wi-Fi LMAC/HAL/registers
└── S31 Wi-Fi/esp-hal   ──> S31 Wi-Fi LMAC/PHY/HAL + esp-hal

Production application
└── examples/esp32s31-station
    └──> facade + reusable integration + Embassy executor/net

Test harness (HIL)
└── board + clocks + PSRAM/flash + executor + embassy-net/smoltcp
    ├──> facade + reusable integration
    └──> S31 telemetry observer implementations
```

## Layer responsibilities

| Layer | Owns | Must not own |
| --- | --- | --- |
| `pac` | Generated register types and peripheral singleton | Driver policy or handwritten fixes |
| `registers` | Register geometry and finite typed transactions | Async scheduling or protocol state |
| `hal` | Typed power/clock/I2C/PBus operations and hardware boundaries | Board startup or an executor |
| `phy` | Shared RF/baseband calibration plus the currently qualified Wi-Fi profile | Protocol MAC state or raw peripheral ownership |
| `wifi-lmac` | 802.11 DMA descriptors, RX/TX state, interrupts, EDCA, BlockAck and rates | PHY calibration or non-Wi-Fi MAC policy |
| `esp32s31/wifi/sta` | S31 station startup, persistent channel authority, Association/peer policy and executor-independent STA TX ownership | Embassy, a network stack, board allocation or HIL policy |
| `ieee80211` | Portable frame formats and protocol state | Chip registers |
| `wifi/lmac` | Portable VIF/channel-context identity, capability profile and normalized TX/RX contracts | DMA, IRQ, chip registers or executor scheduling |
| `wifi/sta` | Portable STA MLME, scan/reconnect, beacon-loss and power-save decisions | Chip registers, DMA, executor timers or AP policy |
| `wpa2` | Portable WPA2-Personal state, transaction runners and key material | Radio hardware or an executor |
| `integration/network/embassy-net` | Bounded `embassy-net-driver` ownership adapter | A concrete chip or full network stack |
| `integration/esp32s31/wifi-embassy` | Wi-Fi DMA epochs/network leases, async TX and IRQ wakeups | Chip DMA memory representation, board startup or network policy |
| `integration/esp32s31/wifi-esp-hal` | `esp-hal` singleton binding for the ESP32-S31 Wi-Fi backend | Board, PSRAM/flash or executor policy |
| `radio` | Public composition and re-exports | Board/bootstrap policy |
| `examples/esp32s31-station` | Normal board allocation, executor, credentials and application network services | HIL commands, benchmark policy or reusable radio behavior |
| `hil/esp32s31` | Test board clocks, boot, memory placement, executor, real `embassy-net`/smoltcp scenarios | Reusable radio implementation |
| `hil/esp32s31/telemetry` | Atomic counters, IRQ correlation and qualification report snapshots for typed S31 observer events | Driver scheduling, ownership or protocol decisions |

## Wi-Fi split-MAC boundary

The project uses `HMAC` and `LMAC` as responsibility names, not as crate-name
aliases. Portable HMAC makes 802.11 protocol decisions: frame semantics,
per-TID sequence spaces, duplicate policy, key selection and the eventual
STA/AP state machines. LMAC owns bounded execution on one radio: DMA, IRQ,
hardware queues, attempt completion, contention/retry execution and
hardware-specific aggregation transactions. The handwritten `registers`
layer owns only the final register transactions over the generated PAC.

The boundary is not one boolean per feature. ESP32-S31, for example, captures
a transmit BlockAck in hardware but selects and retains missing MPDUs in
software. It executes a timed transmit attempt in hardware, while Rust owns the
retry budget, rate ladder and contention-window update. The portable
`open-esp-radio-wifi-lmac` contract therefore describes each operation
independently. The exact current backend profile is
`esp32s31::wifi::lmac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES` and is
also returned by `Esp32s31ConnectedStaPort::capabilities()`.

| Operation | Current ESP32-S31 owner |
| --- | --- |
| TX FCS, immediate ACK response, CSMA/CA countdown | hardware |
| Retry decision/rate ladder/contention update | source-owned software |
| TX sequence and CCMP key/PN selection | source-owned software |
| CCMP payload transform and MIC | hardware |
| RX BA agreement match and TX BlockAck capture | hardware |
| RX reorder and TX missing-MPDU retention | source-owned software |

This table describes the complete current service, not all physical chip
resources or a promised future offload. Compile-time memory profiles may
narrow the published limits. An HMAC must consume the profile instead of
scattering `cfg(chip)` tests through portable protocol code.

VIF identity and channel-context identity are separate. This matters before AP
exists: a future STA and AP may be distinct VIFs sharing the one S31 channel
context. Passive monitor support is represented as raw, normalized or
protocol-validated taps rather than a synthetic protocol role, so a slow
observer cannot acquire the normal RX owner. The capability profile advertises
only the owner graph implemented today: one STA, no AP/AP+STA and no monitor
tap.

Raw ESP32-S31 `TxCompletion` remains an attempt-level hardware record. The
Embassy ordinary-TX owner now additionally returns portable `MacTxStatus` with
the total attempt count, final typed rate, ACK semantics and ACK-SNR sample for
the logical exchange. `MacAmpduTxStatus` now joins every aggregate publication,
BlockAck delivery and the optional detached one-MPDU ordinary retry. In that
two-stage case the status is deliberately unavailable until the ordinary owner
has returned its own terminal `MacTxStatus`; aggregate and ordinary PHY rates
remain distinct. RX now uses the same rule: portable `MacRxMetadata<Rate>`
keeps a backend-selected typed PHY record and records the provenance of every
field. `NetworkRxFrame::normalized_metadata()` publishes channel, rate and
RSSI as `HardwareObserved` from the pinned public S31 RX-control ABI. Complete
vendor `dbg_dump_rx_ppdu` independently proves the adjacent
`cur_single_mpdu` status at byte `0x1f`, bit zero. The Espressif header defines
this as IEEE S-MPDU status, so it is preserved as `s_mpdu` and is not negated
into a broad A-MPDU claim. Physical A-MPDU is hardware-observed only for HT,
where complete `dbg_dump_rx_ppdu` separately proves HT-SIG Aggregation bit 27;
a forced HT20/MCS7/SGI HIL cell directly observed it on 78,127 benchmark
records with zero unavailable provenance. VHT/HE format validation instead
establishes A-MPDU containment as `ProtocolValidated(true)`: IEEE defines an
S-MPDU as the sole MPDU in an A-MPDU carried by a VHT/HE PPDU. This states
container membership, not member count, and HIL separately proved 117,190 HE
records retained exactly that protocol provenance. Crypto and A-MSDU remain independent
`Unavailable` fields at staging. The
connected protected-data parser carries the same record in each Ethernet
event: successful S31 RX-state/MIC admission supplies hardware-observed crypto
success, and complete data decapsulation validates whether the MPDU carries
A-MSDU. No owner may infer these values merely from Protected, A-MSDU or an
active BA agreement.

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
under `driver/<chip>/`; portable protocol logic belongs under
`driver/<protocol>/`. A chip/protocol-specific implementation belongs under
`driver/<chip>/<protocol>/`, as the current Wi-Fi LMAC does.

Bluetooth/BLE, IEEE 802.15.4 and coexistence must not be added to the Wi-Fi MAC
crate. They will have their own protocol and chip-specific layers. `pac`, `hal`
and `phy` remain at chip scope because the current evidence already includes
shared RF calibration and separate Wi-Fi/BT gain banks. This is a reuse
candidate, not a promise of a universal PHY API: shared pieces should be
extracted only after a second protocol or chip supplies concrete requirements.

Portable Wi-Fi remains under `driver/wifi/`; future portable Bluetooth and
IEEE 802.15.4 code should receive peer protocol roots. Chip implementations
belong under `driver/<chip>/<protocol>/`. Coexistence policy that coordinates
physical radios belongs at chip scope rather than inside any one protocol MAC.

The hierarchy is deliberately chip-first: `driver/esp32s31/phy`, not
`driver/phy/esp32s31`. PAC, HAL, PHY, Wi-Fi, future Bluetooth/802.15.4 and
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
executor-specific radio composition. Neither adapter is a composition root.
Normal firmware chooses board memory, tasks and network policy in
`examples/esp32s31-station`; HIL chooses its own board placement, traffic and
reporting policy under `hil/esp32s31`. Only the HIL root may own qualification
commands, fault injection or benchmark telemetry.

Observation follows the same dependency direction. Production integration
defines typed events at semantic RX boundaries and accepts an optional
object-safe observer. The concrete atomics, clock source, IRQ correlation and
snapshot/delta format live in `hil/esp32s31/telemetry`. With no observer
attached, the driver does not read a diagnostic clock; the remaining cost is
only the explicit optional branch at each observation site.

Fault injection follows the same boundary. The HIL may decorate the complete
production `ConnectedRunnerServices`, but it must first let the real transaction
acquire its lease/descriptor owner and then drive a normal production service
edge. It may not fabricate a lifecycle outcome. Protocol-v8 TX fault evidence
is emitted only after the reusable runner/task/RX teardown has returned and
the production TX owner itself reports reset-required. The decorator and its
host command remain HIL fixture code; descriptor quarantine remains driver
behavior.

The facade features preserve those boundaries. `wifi` selects only portable
802.11/WPA2 code, `esp32s31-wifi` adds the hardware Wi-Fi backend,
`integration-embassy-net` adds only the generic network adapter, and
`esp32s31-wifi-embassy` opts into the complete S31 Wi-Fi/Embassy composition.

The S31 STA TX boundary follows the same split. `esp32s31/wifi/sta::tx`
defines entropy, calibrated-power and monotonic-time ports plus the resource
bundle and finite pre-connected policy. The Embassy integration implements
only the time port in `tx_time`; board code supplies entropy and the owned PHY
calibration profile. Association PHY and HE power fields are derived in
`esp32s31/wifi/sta::association`, not in the executor adapter.
The station-wide unique control-TX owner follows the same rule: its epoch
state is in `esp32s31/wifi/sta::tx_epoch`; the integration crate contributes
only the extension which constructs `Esp32s31ControlTx` from a runtime-owned
descriptor slot and later reconstructs it from returned resources.
The persistent `PhyColdState`/platform/observer owner used for cold scan,
running scan and reconnect is similarly defined in
`esp32s31/wifi/sta::channel`; integration modules only implement their scan
and attempt traits for that owner.
The finite scan transaction and mandatory RX cleanup order live in
`esp32s31/wifi/sta::scan`. It expresses dwell in abstract ticks; only the
integration maps those ticks to an executor clock and binds the concrete
RX-DMA and probe-TX owners.

## Transitional debt

`driver/esp32s31/phy/src/radio_hal.rs` still contains finite MMIO leaves whose
final home is HAL/PAC. Moving them is structural work and must preserve PHY
state-machine behaviour. The current boundary and remaining unsafe inventory
are recorded in [the PAC audit](PAC_AND_UNSAFE_AUDIT.md).

The current `ieee80211` crate contains portable frame/state components but is
not yet a complete generic HMAC. Some vendor-derived rate, BA and station
composition remains in the ESP32-S31 LMAC/Embassy crates. Extraction should
follow the explicit service contract and a second consumer or deterministic
simulation test, not a directory-only rename.

The standalone ESP32-S31 station now proves that the driver is usable without
the HIL facade: cold PHY/MAC, active scan, WPA2, the connected PAC/IRQ/DMA
runner, DHCP and an application UDP echo service all execute through the
normal Embassy graph. Its internal-SRAM profile also exposed an architectural
constraint that HIL PSRAM placement hid: stage, network and reorder capacities
must be board-selected independently. `RxReorderFrameStorage` therefore has a
typed slot-count parameter instead of always reserving the vendor maximum.
The application now treats the connected path as a finite epoch. On peer loss
or a controller request it quiesces the interrupt route, cooperatively stops
the staged-RX task, returns its scratch, stops RX DMA, proves TX idle, clears
both association keys and feeds the resulting stopped RX/TX/network owners
back into `Esp32s31Station`. The outer application owner now has distinct
initial, disconnected/running-scan and reconnected phases. A board run
completed a controller-requested teardown, a finite 13-channel running scan,
all eight Authentication--ConnectedEntry stages again without a reset, and
then sustained 100/100 UDP echoes. The scan selected a fresh `ScanRecord`, so
the composition no longer pins reconnect to the old BSSID/channel. Selection
of a genuinely different AP or changed channel remains a separate controlled
hardware cell rather than an architectural ownership gap.
