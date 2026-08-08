# Driver source tree

This directory is the product boundary of `open-esp-radio-rs`. Code needed by
normal firmware belongs here; board qualification, command protocols, traffic
generators and vendor comparison fixtures do not.

```text
driver/
├── radio/                  public facade and feature selection
├── common/
│   └── dma/                shared audited DMA ownership primitives
├── wifi/                   portable Wi-Fi protocol code
│   ├── ieee80211/          frame handling and current HMAC mechanisms
│   ├── softmac/            executor-independent SoftMAC/backend contract
│   ├── sta/                STA MLME, scan/reconnect and power policy
│   └── wpa2/               WPA2 protocol, transactions and cryptographic state
├── chips/
│   └── esp32s31/           ESP32-S31 hardware backend
│       ├── pac/            generated register API
│       ├── registers/      handwritten register transactions
│       ├── hal/            semantic hardware operations
│       ├── phy/            RF/baseband state machines
│       └── wifi/
│           ├── dma/        audited DMA representation and chip placement
│           ├── mac/        safe Wi-Fi MAC backend: IRQ, queues and TX/RX policy
│           └── sta/        executor-independent S31 station composition
└── adapters/               reusable runtime and ecosystem adapters
```

Dependencies point down this list of responsibilities:

```text
application
    -> facade / adapters
    -> portable Wi-Fi policy and chip Wi-Fi MAC backend
    -> chip PHY and semantic hardware operations
    -> register transactions
    -> generated register API
```

The hardware directory names now follow their responsibilities:

- `chips/esp32s31/pac` is the generated PAC in conventional embedded-Rust terms;
- `chips/esp32s31/registers` is a handwritten register-transaction layer, not the
  generated PAC;
- `chips/esp32s31/wifi/dma` owns the S31 descriptor representation, RX ring/storage
  and the target linker-placement primitive needed by the qualified hot path;
- `chips/esp32s31/wifi/mac` is the safe chip-specific MAC backend above
  that leaf;
- `chips/esp32s31/wifi/sta` owns S31 station composition that has no executor or
  network-stack dependency, including Association PHY/power selection,
  associated-peer WMM/HT/HE/rate-control programming and the platform ports
  plus the unique epoch, persistent channel owner and cooperative
  `RadioRegisters` facade consumed by STA transactions. The facade covers RX,
  TX, keys, peer programming and BlockAck; it is therefore named
  `CooperativeRadioHardware`, not after one datapath direction;
- `wifi/sta` owns role-specific STA MLME and policy, including beacon loss and
  the decision to enter or leave power save;
- `wifi/ieee80211` contains portable code, but still combines frame codecs and
  some common HMAC mechanisms.

The executable unsafe-code policy is documented in [`UNSAFE.md`](UNSAFE.md).
All protocol and hardware-policy crates forbid unsafe code. Generated MMIO,
pinned/DMA ownership and the minimal Embassy executor runtime are explicit
audited foundations rather than implicit exceptions in upper layers.

TX BlockAck now demonstrates the intended boundary: `wifi/ieee80211::block_ack`
owns the action codec and one generic agreement state machine, while
`chips/esp32s31/wifi/mac::tx_ampdu::block_ack` owns the vendor STA TID order, S31
completion-register normalization and fixed TX-slot batch. Neither layer owns
DMA backing; that authority remains in the chip DMA leaf.

The S31 TX A-MPDU path is split by execution phase under
`chips/esp32s31/wifi/mac/src/tx_ampdu/`:

- `request` defines typed frame layout, size and HE policy inputs;
- `capacity` performs read-only slot, backing, APEP and TXOP admission;
- `length` owns incremental and replayed aggregate byte accounting;
- `commit` is the only safe MAC-backend module that encodes the private S31 TX
  metadata prefix and advances per-slot metadata;
- `submission` validates the complete batch and produces typed HT/HE register
  programs without publishing DMA or touching hardware;
- `owner` couples that plan to the lower DMA crate's retained backing and
  descriptor authority;
- `hardware` is the safe register-transaction trait implemented by the S31
  register layer;
- `lifecycle` owns completion, timeout, reset and release transitions;
- `retry` owns detached-frame inspection and bounded retry compaction;
- `model` remains host-only qualification code and is absent from 32-bit
  production builds.

`tx_ampdu.rs` is the compatibility facade and storage declaration. New chip
backends may reuse portable BlockAck semantics, but must supply their own
metadata, queue geometry, register planning and DMA ownership adapters.

`wifi/softmac` owns only the portable boundary: VIF/channel-context identity,
implemented-role capabilities and normalized TX/RX plans/status. It does not
own a scheduler, DMA buffers or an executor. The current S31 station plan is a
real consumer of that VIF binding. S31 also materializes a standalone
normalized monitor over the promiscuous RX-ring owner. AP and concurrent
STA+monitor remain explicit unsupported capabilities rather than speculative
implementations. A role-neutral `rx_ring_owner` supplies the finite descriptor
type-state used by the distinct scan and monitor wrappers. The monitor's
chip-neutral `adapters/embassy/wifi` crate copies an accepted observation into
a bounded capture lease, queues only that lease and metadata, and returns
immediately; capture overflow is observation loss rather than radio
backpressure.
The adjacent S31 `monitor_service` combines that sink with the concrete
RX-ring and interrupt-route epoch. It is a finite future returning the same
hardware, RX, sink and route owners after stop; it does not spawn a hidden task
or acquire network-stack resources. Its future borrows the service, and a
fail-closed destructor synchronously stops IRQ then DMA on cancellation/scope
exit. Owner extraction is unavailable until that inactive state is proven.

The `radio` facade exposes a two-phase application configuration API.
`RadioConfig` selects Wi-Fi, Bluetooth and IEEE 802.15.4 subsystems, while
`WifiConfig` selects STA/AP owners and an optional monitor tap. Validation
against concrete `RadioCapabilities` produces a value-only `RadioPlan` before
peripheral or DMA ownership moves. Credentials, AP beacon/security policy,
executor handles and storage are deliberately supplied later to the created
subsystem services. The ESP32-S31 facade materializes that plan through
`start_esp32s31_radio`; its public start/result/error types are Wi-Fi-level and
do not expose the current internal location of the cold-start implementation
under the station composition crate.

The remaining `wifi/ieee80211` frame/HMAC split should change only together
with its public contracts. A directory-only rename would hide the coupling
instead of removing it. AP MLME will be a peer of `wifi/sta`, not another mode
inside the station owner.

The intended multi-chip, multi-protocol shape is chip-first for hardware and
protocol-first for portable logic. A future ESP32-C5 backend is therefore a
peer of `chips/esp32s31`, while portable BLE and IEEE 802.15.4 implementations are
peers of `wifi`. Shared RF power, clocks, calibration and radio arbitration
may be extracted only from concrete common behaviour. Wi-Fi, BLE and
IEEE 802.15.4 timing/MAC semantics remain separate.

`hil/`, `verification/` and `tools/` may depend on this tree. The driver must not
depend on them. In particular, HIL UART commands, raw telemetry strings,
benchmark limits, board credentials, vendor artifacts and artifact hashes are
not driver API.

## Next extraction order

The remaining large `adapters/embassy/esp32s31-wifi` crate is not yet a
clean adapter. Continue with dependency cuts, not bulk file moves:

1. split the large connected adapter files by mechanism: aggregation,
   control/BA/power events, RX DMA/staging, task lifecycle and network leases;
   channels, signals, task spawning, async timers and IRQ wakeups remain in
   `wifi-embassy`;
2. split frame codecs from common HMAC only when the new HMAC contract has a
   real STA consumer and a simulated test backend;
3. add AP MLME as a peer of `wifi/sta`, and monitor as a non-blocking MAC tap.

The WPA2 deadline/key-publication runner now lives in portable `wifi/wpa2`.
`adapters/embassy/esp32s31-wifi` retains only the Embassy clock plus the
concrete retained-RX and control-TX adapters; WPA2 replay, timeout and rollback
semantics no longer acquire an executor dependency through their source path.

Authentication/Association timing and retry sequencing now live in portable
`wifi/sta::join`; the Embassy adapter contributes only its clock and
the concrete S31 RX/TX port. The complete S31 pre-connected attempt ordering,
inputs and value-only report live in `chips/esp32s31/wifi/sta::attempt`.

The complete S31 scan ordering and cleanup contract now lives in
`chips/esp32s31/wifi/sta::scan`. Its dwell unit is executor-neutral; the
Embassy adapter supplies the one-millisecond timer plus concrete PHY, RX-DMA and
probe-TX owners. Host tests of mandatory RX stop and owner return therefore no
longer compile through the runtime adapter.
The concrete adapter is now explicit rather than hidden behind aliases:
`rx_ring_owner` owns the role-neutral DMA-ring phases; `scan_rx` owns scan
observation and `monitor_rx` owns standalone normalized publication. The
separate chip-neutral Wi-Fi/Embassy adapter owns independent capture leases
and the async consumer edge.
`scan_port` composes one finite channel visit and `scan_target` supplies the
RISC-V hardware bindings. Active-probe publication and passive fallback live
in `chips/esp32s31/wifi/sta::scan_tx` beside the ordinary/control TX owner.

Ordinary descriptor retry/deadline ownership, single-MPDU encoding, the
pre-connected control transmitter and active-scan TX now live together in
`chips/esp32s31/wifi/sta::{ordinary_tx,single_mpdu_tx,control_tx,scan_tx}`.
The former Embassy modules are compatibility re-exports; their host tests no
longer compile through Embassy.

The cooperative register facade, connected-control hardware contract and
complete finite control state machine now live in
`chips/esp32s31/wifi/sta::{cooperative_hardware,connected_control_hardware,
connected_control}`. BlockAck, beacon-loss and power-save transitions accept
one explicitly delivered event and a bounded reorder-command sink; they do not
depend on an executor. The Embassy crate retains mailbox/deadline scheduling
and compatibility exports, but no longer defines this protocol or PAC
behaviour.

The executor-independent Authentication/Association RX, control-TX,
observation and error contracts likewise live in `chips/esp32s31/wifi/sta::join`.
The local `sta_join_port` is now only their retained-DMA/control-TX binding.

The RX descriptor/buffer arena itself now lives in
`chips/esp32s31/wifi/dma::rx_storage`. `wifi-embassy::rx_dma_service` selects the
qualified large-RX dimensions and owns the asynchronous ring/staging epoch;
`network_rx` owns the network-stack sink and `control_mailbox` owns the
bounded semantic-event handoff. The RX DMA service no longer defines the chip DMA
memory representation or unrelated consumers.

The complete `chips/esp32s31/wifi/mac` crate, including its tests, forbids `unsafe`.
Necessary pointer and linker invariants terminate in the audited chip DMA leaf
(or, below it, in the generated PAC/runtime); safe ownership leases and typed
descriptors cross the boundary upward. MAC-backend tests use those same safe leases
instead of defining privileged mock allocations.

RX qualification now follows that boundary: `wifi-embassy` defines typed
`RxPipelineObservation` events and an optional observer interface, while the
atomic counters, IRQ correlation and report snapshots live only in
`hil/targets/esp32s31/telemetry`. Attaching no observer performs no diagnostic clock
reads and keeps qualification policy out of the shipping driver graph.

ESP32-C5 should be introduced as a peer backend before extracting any claimed
cross-chip HAL/PHY crate. Shared code is promoted only when both concrete
backends implement the same semantic operation; an equal register offset or
vendor function name alone is not sufficient evidence.

See [`docs/ARCHITECTURE.md`](../docs/ARCHITECTURE.md) for responsibility and
ownership details.
