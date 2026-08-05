# Driver source tree

This directory is the product boundary of `open-esp-radio-rs`. Code needed by
normal firmware belongs here; board qualification, command protocols, traffic
generators and vendor comparison fixtures do not.

```text
driver/
├── radio/                  public facade and feature selection
├── wifi/                   portable Wi-Fi protocol code
│   ├── ieee80211/          frame handling and current HMAC mechanisms
│   ├── lmac/               executor-independent HMAC/LMAC contract
│   ├── sta/                STA MLME, scan/reconnect and power policy
│   └── wpa2/               WPA2 protocol, transactions and cryptographic state
├── esp32s31/               ESP32-S31 hardware backend
│   ├── pac/                generated register API
│   ├── registers/          handwritten register transactions
│   ├── hal/                semantic hardware operations
│   ├── phy/                RF/baseband state machines
│   └── wifi/
│       ├── lmac/           Wi-Fi LMAC: DMA, IRQ, queues and TX/RX
│       └── sta/            executor-independent S31 station composition
└── integration/            reusable runtime and ecosystem adapters
```

Dependencies point down this list of responsibilities:

```text
application
    -> facade / integration
    -> portable Wi-Fi policy and chip Wi-Fi LMAC
    -> chip PHY and semantic hardware operations
    -> register transactions
    -> generated register API
```

The hardware directory names now follow their responsibilities:

- `esp32s31/pac` is the generated PAC in conventional embedded-Rust terms;
- `esp32s31/registers` is a handwritten register-transaction layer, not the
  generated PAC;
- `esp32s31/wifi/lmac` is the chip-specific LMAC implementation;
- `esp32s31/wifi/sta` owns S31 station composition that has no executor or
  network-stack dependency, including Association PHY/power selection,
  associated-peer WMM/HT/HE/rate-control programming and the platform ports
  plus the unique epoch and persistent channel owner consumed by STA
  transactions;
- `wifi/sta` owns role-specific STA MLME and policy, including beacon loss and
  the decision to enter or leave power save;
- `wifi/ieee80211` contains portable code, but still combines frame codecs and
  some common HMAC mechanisms.

`wifi/lmac` owns only the portable boundary: VIF/channel-context identity,
implemented-role capabilities and normalized TX/RX plans/status. It does not
own a scheduler, DMA buffers or an executor. The current S31 station plan is a
real consumer of that VIF binding; unsupported AP and monitor roles remain
explicit capabilities rather than speculative implementations.

The remaining `wifi/ieee80211` frame/HMAC split should change only together
with its public contracts. A directory-only rename would hide the coupling
instead of removing it. AP MLME will be a peer of `wifi/sta`, not another mode
inside the station owner.

The intended multi-chip, multi-protocol shape is chip-first for hardware and
protocol-first for portable logic. A future ESP32-C5 backend is therefore a
peer of `esp32s31`, while portable BLE and IEEE 802.15.4 implementations are
peers of `wifi`. Shared RF power, clocks, calibration and radio arbitration
may be extracted only from concrete common behaviour. Wi-Fi, BLE and
IEEE 802.15.4 timing/MAC semantics remain separate.

`hil/`, `validation/` and `tools/` may depend on this tree. The driver must not
depend on them. In particular, HIL UART commands, raw telemetry strings,
benchmark limits, board credentials, vendor artifacts and artifact hashes are
not driver API.

## Next extraction order

The remaining large `integration/esp32s31/wifi-embassy` crate is not yet a
clean adapter. Continue with dependency cuts, not bulk file moves:

1. split the large connected adapter files by mechanism: aggregation,
   control/BA/power events, RX DMA/staging, task lifecycle and network leases;
   channels, signals, task spawning, async timers and IRQ wakeups remain in
   `wifi-embassy`;
2. split frame codecs from common HMAC only when the new HMAC contract has a
   real STA consumer and a simulated test backend;
3. add AP MLME as a peer of `wifi/sta`, and monitor as a non-blocking LMAC tap.

The WPA2 deadline/key-publication runner now lives in portable `wifi/wpa2`.
`integration/esp32s31/wifi-embassy` retains only the Embassy clock plus the
concrete retained-RX and control-TX adapters; WPA2 replay, timeout and rollback
semantics no longer acquire an executor dependency through their source path.

Authentication/Association timing and retry sequencing now live in portable
`wifi/sta::join`; the integration crate contributes only its Embassy clock and
the concrete S31 RX/TX port. The complete S31 pre-connected attempt ordering,
inputs and value-only report live in `esp32s31/wifi/sta::attempt`.

The complete S31 scan ordering and cleanup contract now lives in
`esp32s31/wifi/sta::scan`. Its dwell unit is executor-neutral; the Embassy
integration supplies the one-millisecond timer plus concrete PHY, RX-DMA and
probe-TX owners. Host tests of mandatory RX stop and owner return therefore no
longer compile through the runtime adapter.
The concrete adapter is now explicit rather than hidden behind aliases:
`scan_rx` owns DMA-ring phases, `scan_tx` owns active-probe publication,
`scan_port` composes one finite channel visit and `scan_target` supplies the
RISC-V hardware bindings.

The executor-independent Authentication/Association RX, control-TX,
observation and error contracts likewise live in `esp32s31/wifi/sta::join`.
The local `sta_join_port` is now only their retained-DMA/control-TX binding.

The RX descriptor/buffer arena itself now lives in
`esp32s31/wifi/lmac::rx_storage`. `wifi-embassy::rx_backend` selects the
qualified large-RX dimensions and owns the asynchronous ring/staging epoch;
`network_rx` owns the network-stack sink and `control_mailbox` owns the
bounded semantic-event handoff. The RX backend no longer defines the chip DMA
memory representation or unrelated consumers.

RX qualification now follows that boundary: `wifi-embassy` defines typed
`RxPipelineObservation` events and an optional observer interface, while the
atomic counters, IRQ correlation and report snapshots live only in
`hil/esp32s31/telemetry`. Attaching no observer performs no diagnostic clock
reads and keeps qualification policy out of the shipping driver graph.

ESP32-C5 should be introduced as a peer backend before extracting any claimed
cross-chip HAL/PHY crate. Shared code is promoted only when both concrete
backends implement the same semantic operation; an equal register offset or
vendor function name alone is not sufficient evidence.

See [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) for responsibility and
ownership details.
