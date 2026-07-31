# ESP32-S31 driver-repository HIL integration audit

Audit date: 2026-07-31.

The working HIL application, bootstrap, linker layout and Rust host runner now
live in this repository under `hil/esp32s31`. Its copied `radio_hil.rs` module
is still 11,000+ lines and contains reusable PHY, MAC and STA policy. Treating
that module as the final home of end-to-end logic would leave the source-only
driver incomplete and force every application to reproduce recovered vendor
ordering. The neighboring `esp32s31_rust` copy remains only until crate/HIL
parity makes its deletion mechanical.

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

The network-owned TX allocation and S31 A-MPDU descriptor owner are now joined
by `open_esp_radio::esp32s31::embassy_tx::ReferencedHtAmpduBatch`. It moves a
non-cloneable pinned `embassy-net` lease into the descriptor batch before the
descriptor can become hardware-owned, retains the lease through BlockAck retry
and detach, and returns it only after the DMA owner is free. Its bounded
two-MSDU A-MSDU operation follows complete
`_oracles/libnet80211.a[ieee80211_output.o]::ieee80211_encap_amsdu`: the second
cache allocation is copied into the first and recycled before the first is
published to A-MPDU DMA. The HIL application no longer owns this unsafe
cross-crate lifetime proof.

`open_esp_radio::esp32s31::cooperative_tx::CooperativeTxHardware` now owns the
other reusable part of this boundary. It lends the application's unique
`RadioRegisters` owner to one finite synchronous TX transaction at a time,
without retaining a PAC borrow while the asynchronous aggregate waits for a
completion edge. That lets the same task service RX before polling TX again,
matching complete `_oracles/libpp.a[wdev.o]::wDev_ProcessFiq`.

The application also no longer derives three independent runtime values from
the negotiated HT A-MPDU capability byte. `HtPeerAmpduParameters` keeps the
peer density, S31 protection spacing and `0x1fff/0x3fff/0x7fff/0xffff`
aggregate limit in one MAC-owned value sourced from complete
`_oracles/libpp.a[trc.o]::rcUpdateAMPDUParam`. `AmpduTxConfig` likewise owns
the shared HT/HE retained-retry geometry mutation; the duplicate
application-local enum was deleted.

The join between the recovered rate-control schedule and the actual S31 TX
format is also MAC-owned now. `StaTxRatePolicy` combines association width,
the peer-qualified HE 800-ns LTF choice, payload-LDPC support and the explicit
HT certification override; `StaRateControlAssociation::{tx_rate,
ampdu_tx_rate}` selects the ordinary or independent A-MPDU schedule and
fails back to a typed HT/legacy rate for the still-proprietary Long Range
arena. The application no longer decodes overlapping Dot11N/Dot11Ax rate
bytes, adds LDPC, or updates ACK-SNR directly. Environment variables only
construct the value-only policy and remain HIL configuration.

The post-transfer HE regression used the current standard
`psram-code-psram-data` image against the channel-1 FRITZ HE20 peer. Four
consecutive rounds each completed MCS0..9 at 2xLTF/0.8 us, 2xLTF/1.6 us and
4xLTF/3.2 us: 30 profiles times 64 real A-MPDU submissions, zero failed
profiles and zero terminal retries. Connected-path snapshots retained zero RX
`BUFFER_FULL`/`FIFO_OVERFLOW`, MIC failures and parser/policy rejections. The
new rate-policy owner therefore preserves both the HE formatter matrix and
the RX-priority/DMA boundary.

A following current-tree DCM regression associated with the controlled Linux
HE20 peer advertising BPSK DCM receive. The HE S-MPDU oracle completed, then
44 consecutive rounds each exercised MCS0 DCM at 2xLTF/0.8 us,
2xLTF/1.6 us and 4xLTF/3.2 us. Every profile submitted 64 real A-MPDUs with
zero errors, terminal retries or failed rounds. Application-owned
profile-local telemetry bracketed each aggregate with the complete
blob-derived RX statistics snapshot and counted RX IRQ/DMA staging separately;
all profiles and four independent ten-second intervals retained zero
`BUFFER_FULL` and zero `FIFO_OVERFLOW`. This telemetry remains HIL policy, not
driver runtime, while the statistics decoder and RX ownership transaction
remain driver-owned.

The connected DCM path also exposed an ownership error at the application/
driver boundary. The application removed two pinned network leases before
checking the ROM-derived HE APEP limit. At MCS0 DCM, the 1,850-byte limit
admits one full-size protected Ethernet MPDU but not two; the rejected second
lease escaped through the generic 54-Mbit/s legacy spill path.
`ReferencedAmpduIngressPolicy` now owns this decision next to
`ReferencedHtAmpduBatch`: HT may prefetch the pair required by this adapter,
whereas HE begins with one lease and claims every subsequent lease only after
`can_push_he` validates the exact APEP/TXOP and allocation capacity. The
application retains only the HIL traffic source and its optional offered-load
pacing.

The reset-separated post-transfer `psram-code-psram-data` qualification used
BCC DCM MCS0, 2xLTF/0.8 us and simultaneous traffic. It delivered
1.002 Mbit/s downlink and a conservative 0.749-Mbit/s uplink floor with
`spill=0`, zero `BUFFER_FULL` and zero `FIFO_OVERFLOW`. Fifteen consecutive
Linux station-statistics samples independently decoded the uplink as
4.3-Mbit/s HE-MCS0 DCM. The immutable contract and artifact hashes are in
`docs/hil/2026-07-31-he20-dcm-connected.md`.

The separately qualified LDPC DCM MCS0 cell passed the same connected
contract at 1.001 Mbit/s downlink plus a 0.749-Mbit/s uplink floor, again with
`spill=0` and zero RX DMA starvation. The strict device evidence required both
`he_dcm=1` and `he_ldpc=1`; Linux independently decoded all fifteen sampled
uplink vectors as 4.3-Mbit/s HE-MCS0 DCM. No application policy was promoted
for this cell because the typed constructor and both peer capability gates
already lived in `HeDcmRate` and `StaTxRatePolicy` before the run. Details are
in `docs/hil/2026-07-31-he20-dcm-ldpc-connected.md`.

The executor-neutral BlockAck decision is now also driver-owned as
`open-esp-radio-mac-esp32s31::tx_runtime::AmpduRetryState`. Both the internal
DMA A-MPDU path and the referenced `embassy-net` path use the same bounded
owner for 12-bit sequence-number wrap, cumulative acknowledged/attempted MPDU
accounting, partial-BlockAck retry masks, retained-sequence compaction, the
four-attempt limit and the qualified HT-versus-HE one-missing-MPDU policy.
The application still performs the deliberately separate operations: queue
detach, DMA compaction, EDCA backoff selection, interrupt waiting and
individual-MPDU transmission.

Eight host tests cover sequence wrap, stale BlockAck bits on a nonzero TX
status, HT/HE single-MPDU divergence, the terminal attempt limit and
DMA/state frame-count disagreement. They also cover the complete vendor
Trigger-flow terminal predicate. The status jump table in
`_oracles/libpp.a[lmac.o]::lmacProcessTxComplete` maps status five to
`lmacProcessAckTimeout`; both retry leaves call `lmacProcessTBSuccess` only
when the queue Trigger-flow bit is set and the applicable primary/secondary
packet-count sum is zero. `TxCompletion` now decodes those raw fields and
`AmpduRetryState` returns the distinct `FinishTriggerFlow` decision before
BlockAck accounting. It therefore releases aggregate ownership without
fabricating an ACK or adding an ordinary MPDU attempt. This behavior is
source-implemented and host-tested; it is not HIL-qualified until an external
AP sends a valid Trigger to the open STA.

A following current-tree
`psram-code-psram-data` DCM HIL run completed forty full three-profile rounds,
64 real A-MPDUs per profile. Profiles that encountered partial BlockAck
reported 65 or 66 aggregate attempts for 64 submissions, proving the new
driver state exercised retained retry on hardware; every profile retained
zero retry failures and RX `BUFFER_FULL`. Four independent ten-second
snapshots retained zero `FIFO_OVERFLOW`, MIC failure and RX/TX panic.

A post-transfer `psram-code-psram-data` HT40/MCS7/SGI run sustained
98.752--107.090 Mbit/s application uplink with 31--32 MPDUs and approximately
225 us of preparation per 48,448-byte aggregate. The first concurrent
25-Mbit/s downlink recheck delivered 26.2--26.4 Mbit/s downlink and
62.1--77.3 Mbit/s uplink, but also observed 26 hardware `BUFFER_FULL` events.
The cause was application instrumentation, not the transferred DMA contract:
the direct-RX and concurrent-TX reporters called the synchronous ROM
`ets_printf` path in the packet latency window. Moving periodic metrics to the
existing bounded asynchronous USB logger eliminated the stall.

The final reset-separated strict rerun offered 25.001 Mbit/s from the host and
measured 25.006 Mbit/s median direct receive. Because the independent
five-second TX windows are not timestamp-aligned with the host interval, the
qualifier now reports their conservative minimum instead of a misleading
median; the fully overlapping TX window delivered 68.276 Mbit/s, for a
93.282-Mbit/s RX-median-plus-TX-floor. The final hardware delta reported zero
`BUFFER_FULL`, zero `FIFO_OVERFLOW`, zero pairwise MIC failures and zero true
parser/policy rejections. The ownership transfer is therefore build-,
one-way-, and bidirectional-HIL-qualified. Diagnostic records may be dropped
under pressure by design; radio ownership may no longer block behind logging.

## Reusable logic that still must move

### 1. PHY target executor

Application symbols from `PreludePort` through its `PhyRegisterPort`
implementation contain the exhaustive execution of `PhyRegisterExternalBinding`
and all nested RF/baseband/I2C/PBus bindings. The actual application policy is
small: an Embassy microsecond timer, operation limits and optional diagnostic
hooks.

The first target-executor slice now lives in
`open-esp-radio-phy-esp32s31::target_executor`: all ten shared PHY-I2C
completion loops and the TX-calibration PBUS completion loop use the injected
`PhyAsyncDelay` trait and one driver-owned 10,000-sample bound. The application
implements only a zero-sized Embassy timer adapter for these operations. A
post-transfer `psram-code-psram-data` HE20 run delivered 10.049-Mbit/s RX plus
65.764-Mbit/s TX with zero `BUFFER_FULL` and zero `FIFO_OVERFLOW`.

The remaining `complete_*` RF/baseband composition must move into the same
module next. Keep diagnostics behind an optional no-op observation hook and do
not add an Embassy dependency to the PHY core.

The HIL now at least consumes the existing canonical `run_phy_register`
driver loop. Its former application copy of `step_local -> lower -> await ->
advance_external` was deleted, while operation ordinals and crash stages moved
to the real `PreludePort::complete` hardware boundary. This change reduced the
encoded application from 1,129,104 to 998,912 bytes. A reset-separated
PSRAM/PSRAM HE20 run then passed at 10.012-Mbit/s RX plus a 65.814-Mbit/s
concurrent TX floor with zero strict DMA-starvation failure. `PreludePort` and
its nested `complete_*` graph are still the remaining application copy; the
rejected split-future warning below therefore continues to apply.

The final registration PHY-I2C edge has also moved into
`target_executor::complete_final_i2c`. Its read-only action set, fixed
10,000-edge bound and deadline completion are now driver policy; the HIL arm
is one injected Embassy-delay call. The following PSRAM/PSRAM HE20 run passed
at 10.010-Mbit/s RX plus a 65.386-Mbit/s concurrent TX floor with zero DMA
starvation.

The guarded whole-port transfer is now complete. `TargetPhyRegisterPort` owns
the nested RF/baseband/channel completion composition as one private async
graph; only the complete registration port and channel lifecycle functions are
public. `PhyTargetObserver` preserves HIL crash stages, ROM comparisons and raw
MMIO evidence without letting diagnostics determine a transition result. The
application-local `PreludePort` and 1,206 lines of completion logic were
deleted. The target placement audit passed with a 998,912-byte encoded image,
and a reset-separated PSRAM/PSRAM HE20 run passed at 10.008-Mbit/s RX plus a
69.758-Mbit/s concurrent TX floor with zero DMA starvation.

A 2026-07-30 experiment copying the nested RX/TX calibration composition into
separate public cross-crate `async fn`s was rejected. In the identical
`psram-code-psram-data` image it moved the runtime text end from
`0x500c1faa` to `0x500c248e`; consecutive reset-separated HE20 runs reported
40 and 12,097 RX `BUFFER_FULL` events. Restoring the then-current single
application port restored the exact `0x500c1faa` frontier and passed at 10.047-Mbit/s RX
plus 66.169-Mbit/s TX with zero DMA starvation. The correlation is proven by
the HIL runs; the suspected cause is changed nested-future/state layout, not
yet a proven stack-overflow diagnosis. The completed transfer followed the
resulting constraint: it moved one private `TargetPhyRegisterPort` composition
instead of exposing the nested completions. The placement audit and strict
simultaneous RX/TX qualification are the enforced frontier guards; a directly
nameable compile-time RPITIT future size remains unavailable on stable Rust.

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

- the production half of `TxStorage`: TX power, negotiated A-MPDU parameters,
  EDCA retry state and completion ownership (the counters remain HIL policy);
- `transmit_encoded_frame` and `transmit_encoded_unicast_with_retry`;
- `transmit_protected_ethernet_frame`;
- `append_protected_ethernet_ampdu_frame`;
- `append_protected_ethernet_amsdu_ampdu_frame`;
- `transmit_protected_ethernet_ampdu`, including retained BlockAck retry;
- the policy-neutral part of
  `transmit_referenced_protected_ethernet_ampdu`.

Move this into a new MAC runtime module built over the existing `TxSlot`,
`HtAmpduTxStorage`, rate-control and key-slot types. The caller supplies
Ethernet frames, negotiated peer policy and an async TX-completion edge.
Environment-variable matrix selection, synthetic payload generation and HIL
report formatting stay in the application.

The frame/descriptor lifetime half of the last item is already promoted as
`ReferencedHtAmpduBatch`; `AmpduRetryState` now owns the shared BlockAck
decision and retained-sequence transition. Aggregate construction, EDCA
mutation around the returned decision, individual-retry execution and the
completion-edge abstraction are the remaining halves. Schedule-to-format
selection and ACK-SNR completion observation are already promoted through
`StaTxRatePolicy`. Do not move the current function verbatim: it directly
reads Embassy signals, timers, HIL counters and compile-time environment
selections. Split each remaining driver-owned finite state transition from
the executor adapter and report sink.

### 4. MAC bottom-half and priority

`RxFrameQueue`, `IrqState`, the RX-first outer `select`, and
`yield_to_pending_rx_bottom_half` reconstruct the vendor FIQ/PP-task
priority:

- complete `wDev_ProcessFiq` services RX success before TX completion;
- complete `lmacRxDone` posts PP event 17;
- long A-MPDU preparation must yield after a finite MPDU unit when RX work is
  already pending.

The fixed token queue already lives in `open-esp-radio-mac-esp32s31`.
`EmbassyMacIrqRuntime` now joins the executor-neutral `IrqState` to two
coalescing Embassy wakes, owns the RX/TX interrupt classification and counts
RX publications. The application no longer implements `IrqSink` or maps raw
MAC bits itself. Its concurrent wait polls `wait_rx` before the TX future, as
required by the adapter contract.

The remaining application-owned part is the connected-frame protocol
dispatcher invoked after an RX wake. Its Trigger, NDPA, AddBA, CCMP and
`embassy-net` routing policy belongs in the future STA runtime rather than in
the interrupt adapter.

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

The first scan-to-association slice is now driver-owned. The IEEE 802.11 crate
selects Automatic/HE20-preferred/forced-HT20 mode, validates the peer's HE20
MCS9 or HT40 geometry, produces the exact primary/center-channel plus rev0 CBW
tuple, and owns the one-second vendor response deadline and finite 160-ms
Association compatibility schedule. `associate_target` now has one scheduled
TX branch instead of separate initial/retry copies, and its local PHY/channel
selectors were deleted. A post-transfer remote-only
`psram-code-psram-data` HE20 run delivered 10.011-Mbit/s RX plus 68.788-Mbit/s
concurrent TX with zero `BUFFER_FULL` and `FIFO_OVERFLOW`. Frame/RX ownership,
WPA2 action dispatch and the final connected dispatcher remain to move.

Open Authentication protocol ownership has now moved too.
`StaAuthenticationRuntime` consumes exactly one non-QoS sequence number per
attempt and owns the three-attempt limit, complete vendor 1,000-ms response
deadline, selected-peer Authentication/Deauthentication classification,
retry reason and terminal result. The HIL is only its hardware executor: it
arms and recycles the RX ring, submits the encoded request, extracts management
frames and advances the Embassy timer. Three host tests cover timeout
exhaustion and sequence wrap, selected-peer success, disconnect retry and
status rejection. The reset-separated PSRAM/PSRAM HE20 regression then passed
at 10.005-Mbit/s RX plus a 66.460-Mbit/s concurrent TX floor with zero DMA
starvation. Association RX ownership, WPA2 action dispatch and the final
connected dispatcher remain to move.

The post-response peer join is now driver-owned too.
`StaPeerScanPolicy` retains the scan-derived association PHY, typed HT A-MPDU
parameters, effective HE BSS color and atomically validated WMM policy.
`StaPeerAssociationPlan` accepts only a successful response, applies a valid
response WMM set as one all-or-nothing override, parses the HE20 peer once,
and produces the peer QoS bit, link metric and initialized
`StaRateControlAssociation` from one consistent view. The HIL deleted its
duplicate raw capability-byte, WMM-to-TXOP, HE-operation and rate-control
construction. Three host tests cover the HE20 plan, response-WMM precedence
and rejection. The reset-separated PSRAM/PSRAM HE20 bidirectional regression
then passed at 10.012-Mbit/s RX median plus a 67.726-Mbit/s concurrent TX floor
with zero strict DMA-starvation failures.

The ordinary TX runtime slice is now driver-owned as well.
`StaTxRuntimePolicy` holds the negotiated HT A-MPDU parameters, six-bit HE BSS
color and all four mutable EDCA contention windows. `UnicastRetryState` owns
the bounded attempt count, exact recovered legacy/HT retry-rate choice and the
success, ACK/CTS-timeout and terminal CW transitions. The HIL no longer
contains that retry state machine; it supplies hardware RNG entropy, waits for
DMA completion and sets the Retry bit on the retained encoded MPDU. Three new
host tests cover the vendor defaults, exact rate/CW progression and terminal
reset. A standalone reset-separated PSRAM/PSRAM HE20 regression then passed at
10.012-Mbit/s RX median plus a 71.361-Mbit/s concurrent TX floor, with zero
`BUFFER_FULL` or `FIFO_OVERFLOW` failures.

## Code that should remain in the driver-repository HIL

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

## Old-repository retirement boundary

The old open-radio consumers in `esp32s31_rust` do not contain another
production driver implementation beyond the still-duplicated monolithic HIL:

- `open_radio_frontier`, `open_radio_power_hil` and the small prelude binary
  are board ownership, crash-record and qualification entry points;
- `open_radio_vendor_oracle_hil` has moved to the isolated
  `hil/vendor-oracle/esp32s31` workspace. It deliberately links or wraps
  vendor/ROM leaves and remains outside every source-only dependency graph;
- `wifi_scan` contains closed-driver comparison and raw oracle probes. Stable
  register identities discovered there belong in SVD/PAC, but the probe
  program itself stays in the application;
- the driver repository's `cargo hil` runner now owns build, flash,
  bidirectional traffic and vendor-oracle entry points. Remaining old xtask
  scenarios are deleted only after their strict gates are reproduced here;
- linker placement, the PSRAM/PSRAM bootstrap and the 10,900-byte SRAM ISR
  frontier remain board/application responsibilities.

Consequently the old repository is not the place for further driver work. The
next useful transfer is a typed MAC runtime state machine from the named
functions above, followed by one complete PHY target executor and finally the
STA link state machine. After those paths and the remaining strict HIL
scenarios have parity here, the duplicated radio module, vendor oracle and old
radio xtask support can be deleted from `esp32s31_rust` together.

## Transfer order

1. RX ownership transaction and non-copyable completion token — complete.
2. Fixed RX token queue and ISR work ordering — complete; executor-neutral
   bottom-half scheduling remains.
3. Pinned `embassy-net` frame/A-MPDU lifetime owner — complete.
4. Cooperative short-lived TX access to the unique PAC owner — complete,
   including strict zero-starvation bidirectional requalification.
5. PHY target executor with injected delay/observation traits — top-level
   `run_phy_register` is consumed by HIL; the one-piece target port and its
   nested RF/baseband composition remain.
6. MAC TX runtime and protected A-MPDU retry owner — BlockAck decision,
   retained-sequence state, peer TX/EDCA policy and ordinary individual retry
   complete; aggregate construction and executor/hardware completion adapters
   remain.
7. STA link runtime.
8. Optional `esp-hal` adapter — complete; Embassy executor adapter remains.
9. Shrink the HIL file to configuration, test scenarios and reporting.

Each stage first receives host tests in `open-esp-radio-rs`, then the
application is changed to consume it, then the duplicated application logic is
deleted. Hardware qualification follows structural transfer; it must use the
same `psram-code-psram-data` profile and strict RX statistics gate.
