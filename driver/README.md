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

Current source paths still reflect earlier fine-grained crates. Their public
meaning is the boundary above; directory movement must not introduce another
adapter API.

- `radio`: public requests and typed role lifecycle.
- `wifi/{ieee80211,softmac,sta,wpa2}`: portable Wi-Fi protocol logic.
- `chips/esp32s31/{pac,registers,hal,phy}`: chip RF/register implementation.
- `chips/esp32s31/wifi/{dma,mac,sta,ap}`: ESP32-S31 Wi-Fi backend.
- `adapters/embassy/esp32s31-wifi`: internal concrete runtime implementation.
- `integration/esp32s31/embassy-wifi`: production composition and the only
  place applications enter the current ESP32-S31 station/AP/monitor service.
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
              Idle -> Monitor -> Idle
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
an exclusive capture role. The production STA API is deliberately always
awake. AP owns one validated HT20 or HT40 ERP/HT BSS, a bounded peer table,
WPA2-PSK/CCMP, per-peer Block Ack and pairwise HT A-MPDU unicast Ethernet.
Each aggregate width and guard interval is bounded by the BSS geometry and the
associated peer's observed HT capabilities. It does not claim AP+STA,
group-data TX, HE or power save.
BLE, Bluetooth, IEEE 802.15.4 and coexistence are not public runtime features
and have no placeholder public owner types. Internal typed PAC/HAL/LMAC
transactions exist for the reviewed Wi-Fi-side PTI/request leaves and COEX
hardware timers; they deliberately remain below the public capability boundary
until scheduler/lifecycle ownership and joint-radio hardware evidence exist.

ISR handlers are private backend details: they record pending work and wake the
runner. Examples contain no ISR, PAC, DMA or register assembly.

The default resource profile fits a direct-to-flash internal-SRAM image.
`high-throughput` selects the qualified 64-stage/40-RX/32-TX/32-A-MPDU
envelope and requires product-owned initialized PSRAM placement for CPU-only
state. DMA-visible and latency-critical owners remain in internal SRAM.

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
data validation/decapsulation lives in the chip Wi-Fi crate; bounded Ethernet
batch retention and `WdevServiceSet` live in the Embassy WDEV adapter. The
integration `radio_resources` module owns the one network/TX/A-MPDU allocation
borrowed by mutually exclusive role epochs. AP/STA peer state, security,
Block-Ack negotiation, rate policy and lifecycle remain separate owners.

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
- UDP/TCP/ICMP HIL qualification independently guards STA and AP datapaths.
