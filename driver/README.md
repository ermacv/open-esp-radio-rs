# Driver architecture

The driver is an Embassy-only, source-owned radio stack. Applications provide
board identity, credentials and peripherals; HIL, traffic generators, UART
protocols, vendor artifacts and qualification policy stay outside `driver/`.

## Boundaries

```text
application (embassy-net Stack, DHCP, sockets)
    │
    ├── open-esp-radio public lifecycle and policy
    └── Esp32s31WifiDevice: embassy-net-driver::Driver
            │
            ▼
ESP32-S31 Embassy Wi-Fi runner (sole PAC/DMA/ISR owner)
            │
    ┌───────┴────────┐
    │ Wi-Fi HMAC     │ portable STA/AP frame, MLME and WPA2 logic
    │ S31 LMAC/DMA   │ queues, IRQ, descriptors and time-critical MAC work
    │ S31 RF/PHY     │ calibration, clocks, channel and baseband transitions
    │ registers/PAC  │ typed transactions and generated MMIO access
    └────────────────┘
```

The source tree follows the ownership boundary above. There is no compatibility
module for the former vendor-derived `wdev` naming.

- `radio`: public requests and typed role lifecycle.
- `wifi/{ieee80211,softmac,sta,wpa2}`: portable Wi-Fi protocol logic.
- `chips/esp32s31/{pac,registers,hal,phy}`: chip RF/register implementation.
- `chips/esp32s31/wifi/{dma,mac,sta,ap}`: ESP32-S31 Wi-Fi backend.
- `adapters/embassy/esp32s31-wifi`: internal concrete runtime implementation.
- `integration/esp32s31/embassy-wifi`: production composition and the only
  place applications enter the current ESP32-S31 station/AP/monitor service
  or its explicit ESP-NOW composition hooks.
- `common/dma`: audited generic pinned-memory foundation.

`embassy-net::Stack`, DHCP, sockets and network tasks are application-owned.
The driver exposes a persistent `embassy-net-driver::Driver`; it publishes
link down while Wi-Fi is idle/scanning/monitoring/disconnected and flushes
stale queue state at role boundaries.

## Ownership and lifecycle

One eternal runner owns the physical radio, PAC, DMA and interrupt routes.
Public handles carry commands only. Dropping a handle never destroys or resets
hardware.

```text
Radio runner: Starting -> Ready -> Faulted
Wi-Fi:        Idle -> Station -> Idle
              Idle -> Scan -> Idle
              Idle -> AccessPoint -> Idle
              Idle -> Station+AccessPoint -> Idle
              Idle -> Monitor -> Idle
              Idle -> Standalone ESP-NOW -> Idle  (explicit composition hook)
```

`WifiIdle`, `WifiStation`, `WifiAccessPoint` and `WifiMonitor` are affine.
Starting consumes idle; stopping consumes the role and returns idle only after
the runner has masked the source, drained pending IRQ work, completed or
quarantined TX, stopped RX, detached queues and published link down. If
quiescence cannot be proved, the runner enters a terminal fault and no reusable
idle owner is fabricated.

`WifiIdle::scan` is a finite operation: it consumes idle, actively scans the
requested channels, returns a bounded value-only report and restores idle.
It cannot associate. Station owns its separate candidate scan plus
authentication, association, WPA2, connected and reconnect policy. Monitor is
an exclusive capture role. Station defaults to `AlwaysAwake`; its optional
legacy power-save policy owns PM transitions, TIM/DTIM/listen scheduling and a
reviewed TBTT wake-programming prefix, but S31 RF/PHY modem-sleep entry remains
fail-closed before the unreviewed WDEVPWR binding. AP owns one validated HT20
or HT40 ERP/HT BSS, a bounded peer table, WPA2-PSK/CCMP, per-peer Block Ack,
pairwise HT A-MPDU unicast Ethernet, sleeping-peer queues and DTIM-gated group
traffic.
Each aggregate width and guard interval is bounded by the BSS geometry and the
associated peer's observed HT capabilities. The combined STA+AP role owns one
same-channel physical RX producer, one physical TX owner and one IRQ epoch;
the station association owns the channel and its loss tears down the complete
pair before either role can be reused. AP does not claim HE operation.

ESP-NOW v1 plaintext has strict portable framing, fixed-capacity peers,
connected normal-RX/TX ownership and an explicit standalone same-channel role
hook. Standard P2P OFDM/HT20 rates are selectable. Long Range executes and
restores only the reviewed low-rate PHY gate before rejecting the missing LR
PLCP/queue-vector contract. Encrypted-peer PMK/LMK, PN and replay ownership is
implemented and zeroizes secrets, while on-air encryption remains fail-closed
because the S31 ESP-NOW key selector and Action-frame AAD are not reviewed.

Ordinary station HE20 SU S-MPDU publication is implemented. Trigger and NDPA
events reach typed bounded handoffs but reject the unreviewed HE-TB vector and
feedback formatter before publication. MCS32 is modeled separately as HT
duplicate mode and flows through STA/AP selection diagnostics; S31 encoding
remains fail-closed instead of assigning an unevidenced selector. These source
boundaries are not additions to the qualification ledger without dated HIL
evidence.

BLE, Bluetooth, IEEE 802.15.4 and coexistence are not public runtime features
and have no placeholder public owner types. Internal typed PAC/HAL/LMAC
transactions exist for the reviewed Wi-Fi-side PTI/request leaves and COEX
hardware timers; they deliberately remain below the public capability boundary
until scheduler/lifecycle ownership and joint-radio hardware evidence exist.

ISR handlers are private backend details: they record pending work and wake the
runner. Examples contain no ISR, PAC, DMA or register assembly.

The production profile executes ordinary code and task stacks from PSRAM.
DMA-visible buffers, interrupt/trap stacks, critical data and the explicitly
audited hot/interrupt call graph remain in internal SRAM. The placement audit
fails if a DMA or interrupt owner crosses that boundary.

## Safety

Safe code above the generated PAC, DMA leaves and minimal platform runtime
cannot manufacture register, descriptor or interrupt ownership. See
[`UNSAFE.md`](UNSAFE.md). No public software-reset escape hatch exists.

### Register authority

The intended production direction is:

```text
reviewed register model -> generated/register-local PAC -> narrow HAL -> driver
```

The PAC owns register-local fields, masks, encodings and indivisible access.
The HAL owns polling, delays, multi-register order, lifecycle transitions,
recovery and runtime MMIO serialization. Wi-Fi role and protocol policy stays
in the driver. Rust ownership expresses who may perform an operation; it does
not by itself prove that a reverse-engineered register meaning is correct.

Capabilities are shaped by real exclusivity and sharing requirements. A
borrowed `RadioChannelHal` is the current channel-switch capability, not a
requirement to mechanically `split()` every peripheral. Copyable runtime
handles remain tied to the HAL-owned register arena, whose `RefCell` is the
explicit same-task serialization owner. A shared handle must not imply
unsynchronized cross-thread MMIO access.

Runtime and cold-MAC ownership is held by an opaque `RadioRuntimeOwner` in the
HAL arena. Its access handles provide finite serialized HAL transactions,
never a PAC callback. The cooperative STA facade reaches HE peer setup,
beamforming, TX/A-MPDU, RX DMA and connected control only through a guarded
`WifiMacHal`.

Powered PHY setup borrows an opaque `PhyHal`. It has no `Deref`, generic
register callback, conversion to a PAC owner, or compatibility alias. PHY has
no PAC Cargo dependency and may pass this capability only to named HAL
operations. `Radio::phy_hal_parts` exists where one lifecycle operation also
needs the platform singleton; `Radio::phy_hal_mut` borrows only the PHY
capability. These are ownership APIs, not mechanical peripheral `split()`.

Repository contracts reject direct production dependencies on the PAC above
HAL, PAC-owner re-exports, the removed broad PHY APIs and reintroduction of a
`Deref` escape. Further PAC-to-HAL movement is semantic: when a reviewed PAC
leaf contains polling, delay, recovery, lifecycle policy or a composition of
separately meaningful hardware operations, migrate that complete sequence to
HAL and compare the compiled production entry before changing evidence.

AP and STA share mechanisms only below their role policy. Common protected
data validation/decapsulation lives in the chip Wi-Fi crate. The Embassy
adapter's role-neutral `datapath` owns scheduling, network queues, IRQ, DMA RX
and physical TX; `StationRoleRuntime` and `AccessPointRoleRuntime` own their
separate protocol states. `SingleRoleServices` and `ConcurrentRoleServices`
only compose these owners. AP/STA peer state, security, Block-Ack negotiation,
rate policy and lifecycle remain separate.

Permanent STA and AP network endpoints share one finite physical TX credit
pool. Each endpoint retains one ingress response credit. Ordinary egress uses
the remaining pool elastically: an inactive peer imposes no quota, while every
returned credit wakes one active endpoint which is actually waiting. Per-VIF
FIFOs preserve publication order and the physical datapath scheduler owns
cross-VIF frame fairness.

## Extension rules

- Extend AP only through its peer Wi-Fi role; never place AP policy inside STA.
- Add ESP32-C5 as a peer chip backend. Extract shared code only after both
  chips demonstrate the same semantic operation; matching offsets are not an
  abstraction.
- Add BLE/802.15.4/coexistence only with a real owner graph and scheduler.
- Keep protocol decisions above LMAC; keep deadlines, DMA and hardware status
  below it.
- Keep `embassy-net-driver` in the driver and full `embassy-net` in apps/HIL.

## Acceptance

- host unit tests cover protocol and mailbox/lifecycle transitions;
- target checks build station, monitor and HIL with the pinned toolchain;
- examples contain no unsafe/ISR/PAC/DMA code;
- driver safety audit finds unsafe only in listed leaves;
- observer-free performance HIL measures UDP/TCP/ICMP transport independently
  from correctness images and explicitly named diagnostics images.
