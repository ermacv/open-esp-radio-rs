# `esp32s31_rust` integration audit

Audit date: 2026-07-30.

The `esp32s31_rust` HIL application is not merely an example yet. Its
`open_radio_phy_prelude_hil.rs` module is 11,000+ lines and still contains
reusable PHY, MAC, STA and executor policy. Treating that module as the final
home of end-to-end logic would leave the source-only driver incomplete and
would force every application to reproduce recovered vendor ordering.

This audit distinguishes reusable driver behaviour from board wiring and HIL
policy. Blob/ROM behaviour is promoted into the driver only with the same
source provenance used by the existing crates; HIL-only probes and credentials
must not be copied into the driver.

## Already promoted in this audit

The complete ordinary RX ownership transaction now lives in
`open-esp-radio-mac-esp32s31::rx_pool`:

1. consume one non-`Copy` `RxCompletedDescriptor`;
2. copy its received length into a 32-by-1,700-byte kind-7 staging pool;
3. rearm and append exactly that completed descriptor;
4. complete the ROM-derived reload/base-repair transaction;
5. publish a non-`Clone` `NetworkRxFrame`.

The application retains only the unsafe adapter that borrows bytes from its
concrete static DMA array and restores that array's sentinels. The ordering is
no longer duplicated there.

The fixed FIFO of staged ownership tokens is also
`open-esp-radio-mac-esp32s31::rx_pool::RxFrameQueue`; the application no longer
implements a second queue. Its bounded `IrqSink` publishes the RX Embassy
signal before the TX signal from the same interrupt snapshot. `IrqState`
remains available for a task-side dispatcher, but is intentionally not
enqueued and compare-exchange-drained inside the hard ISR: that experiment
added enough ISR work to produce 50 `BUFFER_FULL` events in one otherwise
healthy bidirectional run.

Primary sources are complete
`_oracles/libpp.a[wdev.o]::{wDev_IndicateFrame,wDev_DiscardFrame,
wDev_AppendRxBlocks}` and
`_oracles/libpp.a[lmac.o]::lmacRxDone`. A reset-separated
`psram-code-psram-data` HE20/MCS9 HIL run delivered 10.036-Mbit/s RX plus
67.942-Mbit/s TX, performed 5,147 RX-priority handoffs during TX preparation,
and reported zero `BUFFER_FULL` and zero `FIFO_OVERFLOW`.

## Reusable logic that still must move

### 1. PHY target executor

Application symbols from `PreludePort` through its `PhyRegisterPort`
implementation contain the exhaustive execution of `PhyRegisterExternalBinding`
and all nested RF/baseband/I2C/PBus bindings. The actual application policy is
small: an Embassy microsecond timer, operation limits and optional diagnostic
hooks.

Move the binding traversal into
`open-esp-radio-phy-esp32s31::target_executor`. Keep it executor-independent by
injecting an async microsecond-delay trait and optional no-op observation hook.
The application should implement only those two traits. Do not add an Embassy
dependency to the PHY core.

### 2. ESP-HAL platform adapter

The former 742-line `esp32s31_rust::open_radio_platform` implementation of the
S31 power/clock, PHY-I2C, temperature, baseband and MAC cold-start traits now
lives in the optional `open-esp-radio-esp-hal-esp32s31` crate. This keeps the
core HAL independent of one framework while avoiding a new copy in every
Embassy application. Interrupt handler functions and logging remain
application-owned. A post-transfer `psram-code-psram-data` run delivered
10.046-Mbit/s RX plus 67.544-Mbit/s TX with zero `BUFFER_FULL` and zero
`FIFO_OVERFLOW`.

### 3. MAC runtime TX owner

The following application work is generic radio behaviour rather than a
benchmark:

- `TxStorage` EDCA, retry and completion state;
- `transmit_encoded_frame` and `transmit_encoded_unicast_with_retry`;
- `transmit_protected_ethernet_frame`;
- `append_protected_ethernet_ampdu_frame`;
- `append_protected_ethernet_amsdu_ampdu_frame`;
- `SelectedAmpduTxConfig`;
- `transmit_protected_ethernet_ampdu`, including retained BlockAck retry.

Move this into a new MAC runtime module built over the existing `TxSlot`,
`HtAmpduTxStorage`, rate-control and key-slot types. The caller supplies
Ethernet frames, negotiated peer policy and an async TX-completion edge.
Environment-variable matrix selection, synthetic payload generation and HIL
report formatting stay in the application.

### 4. MAC bottom-half and priority

`ConnectedRxStagingQueue`, the RX-first outer `select`, and
`yield_to_pending_rx_bottom_half` reconstruct the vendor
FIQ/PP-task priority:

- complete `wDev_ProcessFiq` services RX success before TX completion;
- complete `lmacRxDone` posts PP event 17;
- long A-MPDU preparation must yield after a finite MPDU unit when RX work is
  already pending.

The fixed token queue belongs in the MAC runtime. Embassy `Signal`/`select`
wiring belongs in the future Embassy adapter. The core API should expose
durable `RxPending`/`TxComplete` edges and the required RX-before-TX poll
order, not import Embassy directly.

### 5. STA link runtime

`authenticate_target`, `associate_target`, `await_wpa2_message_1`,
`await_wpa2_message_3`, protected-EAPOL transmission, key installation,
BlockAck negotiation and reconnect deadlines form a reusable STA state
machine. Wire encoders/parsers already live in `open-esp-radio-ieee80211`, and
WPA2 phase/replay logic already lives in `open-esp-radio-wpa2`; the missing
piece is their radio-owner orchestration.

Create an allocation-free STA runtime above IEEE 802.11, WPA2 and the S31 MAC
runtime. It should own explicit states such as
`Scanning -> Authenticating -> Associating -> FourWayHandshake -> Connected`
and return typed actions/deadlines. It must not know the test SSID, UART,
Embassy sockets or benchmark addresses.

## Code that should remain in `esp32s31_rust`

- board resource selection, linker sections and the chosen memory profile;
- SSID/passphrase, static IP and test-peer configuration;
- Embassy task spawning, UART logging and `xtask` qualification markers;
- raw-MAC, UDP/iperf, HE matrix, DCM, Trigger and power HIL scenarios;
- synthetic packets and throughput accounting;
- diagnostic raw-MMIO snapshots that have no runtime effect.

Raw diagnostic addresses are evidence, not reusable access APIs. When a probe
establishes a stable register/field meaning, that identity should move to the
SVD/PAC with blob/ROM/HIL provenance; the application should then use a typed
diagnostic snapshot or delete the obsolete raw read.

## Transfer order

1. RX ownership transaction and non-copyable completion token — complete.
2. Fixed RX token queue and ISR work ordering — complete; executor-neutral
   bottom-half scheduling remains.
3. PHY target executor with injected delay/observation traits.
4. MAC TX runtime and protected A-MPDU retry owner.
5. STA link runtime.
6. Optional `esp-hal` adapter — complete; Embassy executor adapter remains.
7. Shrink the HIL file to configuration, test scenarios and reporting.

Each stage first receives host tests in `open-esp-radio-rs`, then the
application is changed to consume it, then the duplicated application logic is
deleted. Hardware qualification follows structural transfer; it must use the
same `psram-code-psram-data` profile and strict RX statistics gate.
