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
    │ Wi-Fi STA/HMAC │ portable frame, MLME and WPA2 logic
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
- `chips/esp32s31/wifi/{dma,mac,sta}`: ESP32-S31 Wi-Fi backend.
- `adapters/embassy/esp32s31-wifi`: internal concrete runtime implementation.
- `integration/esp32s31/embassy-wifi`: production composition and the only
  place applications enter the current ESP32-S31 station/monitor service.
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
              Idle -> Monitor -> Idle
```

`WifiIdle`, `WifiStation` and `WifiMonitor` are affine. Starting consumes idle;
stopping consumes the role and returns idle only after the runner has masked
the source, drained pending IRQ work, completed or quarantined TX, stopped RX,
detached queues and published link down. If quiescence cannot be proved, the
runner enters a terminal fault and no reusable idle owner is fabricated.

`WifiIdle::scan` is a finite operation: it consumes idle, actively scans the
requested channels, returns a bounded value-only report and restores idle.
It cannot associate. Station owns its separate candidate scan plus
authentication, association, WPA2, connected and reconnect policy. Monitor is
an exclusive capture role. AP, BLE, Bluetooth, IEEE 802.15.4 and coexistence
are not implemented and therefore have no placeholder public owner types.

ISR handlers are private backend details: they record pending work and wake the
runner. Examples contain no ISR, PAC, DMA or register assembly.

## Safety

Safe code above the generated PAC, DMA leaves and minimal platform runtime
cannot manufacture register, descriptor or interrupt ownership. See
[`UNSAFE.md`](UNSAFE.md). No public software-reset escape hatch exists.

## Extension rules

- Add AP as a peer Wi-Fi role, not inside STA.
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
- UDP/TCP/ICMP HIL qualification guards the existing STA datapath.
