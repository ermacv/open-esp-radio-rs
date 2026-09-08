# ESP32-S31 HIL target

The separate `bluetooth-dtm` image selects `bluetooth-hil`, the production
Bluetooth composition and the framed HIL control protocol without a Wi-Fi
network feature. Its USB owner translates bounded DTM requests into typed HCI
commands; radio execution remains in production crates. The same image exposes
the bounded [peripheral diagnostic commands](../../protocol/README.md) for
starting a single-channel connectable advertisement and observing execution.
They require a fresh boot and host reset after the attempt; they do not qualify
a connection or ACL traffic. See the
[Bluetooth fixture and RF scenario](../../host/linux-bluetooth/README.md).

The peripheral diagnostic explicitly configures software window widening with
a 500-ppm local-clock bound through `BluetoothColdStartConfig::with_peripheral_connection`.
Cold start retains and verifies selection of the main XTAL. The bound assumes
that the board meets BLE clock requirements; it is the broadest permitted
bound, not a measurement from PHY calibration. ESP-IDF's `BT_LE_LL_SCA` default
is 60 ppm. Oscillator accuracy and connection readiness remain unqualified.
Other cold-start callers retain the default of unavailable recurring timing
until they supply their board's clock policy.

The diagnostic also supplies an explicit development Controller identity for
LL Version Exchange: Core version 5.4, unassigned company value `0xffff` and
subversion 1. Production callers supply their own identity through
`PeripheralConnectionRuntimeConfig::with_version_information`; none is
inferred from the ESP32-S31 or its PHY calibration.

This workspace selects the shared board boot and memory profile and owns Embassy
executors, network stacks, UART transport and HIL workloads. `cargo hil image
build/flash`, `run` and `run-all` accept `--network upstream-xarxa` (default),
`patched-xarxa`, `upstream-smoltcp` or `owned-xarxa`. Effective dependency locks
are archived beside each image. The [network implementation guide](../../../docs/network-implementations.md)
explains the crates, source policy, memory and UDP admission differences.

`runtime/src/product_hil/network` owns stack setup, IPv4 configuration, socket
API bindings and diagnostic wrappers. All implementations use the same traffic
workers and public production radio constructor. Radio behaviour belongs in
`crates/`.

Standalone HT AP scenarios accept a runtime scheduling comparison:

```console
cargo hil run diagnostic-ap-mixed-tx-work --network owned-xarxa --ap-scheduler rr
cargo hil run diagnostic-ap-mixed-tx-work --firmware-from <RR_RUN_ID> --ap-scheduler deficit
```

Both commands use the same firmware and network implementation. Initialization
carries the selected policy; the immutable scenario snapshot and command records
retain `rr-ht-response24` or `deficit-ht-response24`. Omitting the option preserves
the ordinary unmetered RR default. The shared experiment uses a 3000-us quantum,
100-us minimum and HT aggregate admission against the issued grant. Terminal
cost is modelled PPDU time plus 10-us SIFS and a 32-byte OFDM24 response per
unicast publication; group publications add no response. The same response
envelope deliberately applies to ordinary ACK and aggregate BlockAck exchanges.
It is not measured response PHY, airtime, CCA waiting, protection or hidden MAC
retry time. Unknown PPDU work or saturated receipts fail closed.

This comparison requires `owned-xarxa` and a standalone HT20/HT40 AP workload.
Other network images reject the policy during initialization; simultaneous
STA+AP is rejected while it is selected. The fixture configuration owns the
channel; set its AP channel to 13 with HT40-below for the channel-13 comparison.
Changing policy requires a fresh initialization, not a firmware rebuild. The
driver owns candidate selection, grants and cancellation; HIL supplies only
the explicit comparison model and workload.

`diagnostic-ap-balanced-airtime` offers 130 Mbit/s to each of two AP clients for
one 12-second window. It accepts the same `--ap-scheduler rr|deficit` comparison.
This workload does not force different peer PHY rates: AP downlink selection
uses each station's advertised receive capabilities. An OpenWrt transmit MCS
mask controls the opposite direction and is not evidence of downlink asymmetry.
Multi-flow UDP uses one socket per flow slot, with independent pacing and
sequence counters. Source ports are 4324 and 4325; UDP 4325 is distinct from the
TCP service on that port. Both sockets are bound before readiness, and each host
receiver confirms its own reverse path before Start. The OpenWrt fixture forwards
both UDP ports. A pending publication skips only its own flow for the current
poll. The producer parks on socket events or the earliest future pacing/session
deadline; it yields after a bounded quantum of successful publications.
Terminal send errors stop that flow and remain in its transport evidence.

Per-socket storage is separate, while stack packet pools and driver admission
remain shared resources of the selected integration. Their pressure can still
limit both flows. Offered rates alone do not prove continuous driver backlog
or airtime fairness between unequal peers.

With a scheduling comparison enabled, `WifiAirtimePeer` records and a final
`WifiAirtimeReport` precede the correlated AP stop event. They count the whole AP
epoch, including its ordinary traffic and teardown, rather than only the UDP
window. The target accumulates at most eight distinct association/group keys;
it retains old generations and reports dropped events or saturation explicitly.
No per-packet diagnostic text is emitted. The runner requires complete records
and reconciliation of reservation counts and budgets, with no outstanding work
at successful stop. A negative balance is permitted: it is modelled service
debt, not leaked packet ownership. The raw typed records remain in `protocol.jsonl`.

The released Embassy/smoltcp and owned Embassy/Xarxa compositions enable
`auto-icmp-echo-reply` explicitly for the HIL ping workload because these
dependencies disable default features. Echo response is independent of DHCP,
UDP/TCP sockets and the radio adapter.

- `runtime`: role-neutral control plane and runtime-dispatched workloads;
- `telemetry`: HIL-only diagnostic observers;

[Shared platform](../../../platform/esp32s31/README.md) owns the board profile,
bootstrap, linker scripts and stage-two entry used by HIL and examples.

Diagnostic AP evidence retains `first_rx_protocol_rejection` for each AP epoch.
It identifies the exact data/replay/fragmentation failure or internal
reorder/output-capacity refusal, with radio monotonic time and available
transmitter, frame-control, sequence-control, TID and CCMP PN/key-ID metadata.
Retained reorder frames report their own metadata at the time of rejection.
No payload or key material is stored; subsequent failures do not overwrite the
first record. The aggregate counter retains its existing scope. The typed stop event
carries the record independently of diagnostic text delivery. A null record
means no recorded reason, not proof that an aggregate rejection count is zero.

Diagnostic AP stop evidence also carries `tx_retention`: counts of admitted
software owners dropped by active, unicast power-save or group power-save
storage exhaustion. These counters span the AP epoch and survive traffic
window resets. They are separate from on-air delivery failures. The AP gate
reports these specific losses before evaluating aggregate delivery evidence.
Without a driver observer this field is absent (`null`), rather than zero.

Images with `driver-observation` include `OAMPW`: terminal aggregate exchanges,
actual software publications, submitted PSDU bytes and MPDUs including selective
retries, nominal data serialization time, unestimated publications and saturated
receipts. It uses the same frozen observation window as other aggregate text.
An exchange contributes at its terminal completion/abort edge, so an in-flight
exchange crossing the window boundary is not split between windows. Ordinary
MPDU TX/fallback is excluded. These fields are diagnostic text, not qualification
criteria or a typed traffic-result extension. `nominal_data_us` is not measured
airtime; its scope is defined in the [egress contract](../../../docs/wifi-egress.md).
Delivery and exchange wall time remain separate counters.
`OTXW kind=ampdu|ordinary` reports separate terminal work banks: publications,
PSDU bytes, MPDUs, modelled PPDU microseconds, unknown PPDU publications and
saturated receipts. Ordinary includes connected-STA service and AP network
MPDUs, including fallback and buffered/group release; it is not a census of
all management/control or autonomous hardware responses. Both banks use the
same frozen observation interval. `ppdu_us` is incomplete when `unknown_ppdus`
is nonzero and never claims measured airtime. ACK/BlockAck, protection and
contention belong to the explicit [cost model](../../../docs/wifi-egress.md),
not implicit multipliers in these counters.
`OTXC kind=ampdu|ordinary` uses the same terminal window to report summed
programmed AIFSN, selected backoff slots and publications without contention
evidence. The counts exclude SIFS and do not measure waiting, CCA freezes or
internal hardware retries. They inherit the corresponding `OTXW` saturation
flag. A zero backoff with zero unknown publications is a known selection.
The `diagnostic-tx-architecture` image omits this observer; use a scenario with
`diagnostic-task-poll` to collect the aggregate work receipt.

`performance` contains no driver observer or scheduler instrumentation.
RF calibration details are retained only by the PHY `registration-diagnostics`
feature, selected by HIL `driver-observation`. Ordinary role owners keep the
compact registration result instead of carrying RF measurements through every
role transition. Diagnostic images log the retained PLL results after startup.
`correctness` adds value-only driver observations, while the `diagnostic-*`
images add one explicitly named hot-path probe. `diagnostic-mac-irq` and `diagnostic-tx-wait`
extend the SRAM-closed ISR graph to sample publication-to-IRQ and
IRQ-to-bottom-half latency. UDP/TCP RX/TX/bidirectional
workers remain runtime-selected; STA/AP roles and workload direction do not
change image identity. The task-poll diagnostic accepts both data-plane
placements and retains the ordinary 16-KiB CPU1 owner stack in either case.

The task-poll overlay also counts per-interface stack/driver transfers and
polls without a transfer. For single-flow UDP TX, post-workload `hil-net`
records separate send-future poll residence, suspension and explicit workload
pacing. Cancellation at the session deadline retains the last blocked send.
These extra measurements affect diagnostic throughput. A poll without a
transfer can still do internal protocol work; suspension includes executor
scheduling latency and is not CPU time. The measurements do not timestamp
individual packets inside the radio queue. The MAC IRQ image emits `hil-irq`
entry classification for UDP TX, including interrupts with no pending status,
alongside its existing sampled publication/IRQ/service timings. The interval
ends with the traffic window; summaries are printed after traffic.

MAC IRQ diagnostic TX workloads also emit `hil-tx-ingress` RX progress from
before traffic through the final diagnostic snapshot. These records separate hardware
completion, protocol processing, reorder release and network publication;
`pool_exhausted` counts the subset of publication drops caused by allocation
failure. Incoming ARP replies are required for UDP egress to an unresolved
peer, so RX diagnostics remain relevant in a TX-only workload. The host saves
`delivery-progress.json` before burst qualification, including for zero or
partial delivery. Socket acceptance is separate from host reception, and
unavailable diagnostic counters remain unknown rather than zero.

UDP RX uses a continuous observation window for both single- and multi-flow
sessions. The first data packet starts the configured duration, with at most
two seconds of startup allowance; without a packet, the window starts when
that allowance expires. Neither socket errors, 750-ms silence nor an early
terminal marker ends the window. A separate 750-ms terminal grace excludes
late payload from throughput. Every flow uses the full duration as denominator,
including zero-delivery flows.

The reliable `ORX_WINDOW` record reports startup delay, socket errors, late
datagrams and delayed deadline service. `ORX_SILENCE` reports silence gaps,
including the offset of the longest gap and the trailing gap; `ORX_SOCKET`
identifies the last receive error when errors occurred.
Silence means no data reached the UDP consumer; it does not identify an RF,
stack or driver cause. `ORX_POOL` reports shared Xarxa allocation refusals;
`ORX_RESOURCES` reports compatibility RX/TX free and queued slots plus cumulative
`rx_queue_full` publication refusals. Both records name the selected STA/AP
interface; unavailable monitors remain absent rather than reporting zero. These observations can be
read even when the radio executor is blocked. Slot snapshots include neither
held tokens nor a claim of atomic observation across all queues. RX task-poll
observation closes at the window boundary, before terminal grace and reporting.

UDP TX task-poll intervals close at the end of the measured workload, before
report output. Aggregate evidence closes at the final diagnostic snapshot;
text and structured reports share the same frozen aggregate snapshot. Waiting
for diagnostic output capacity therefore cannot extend these intervals.

`udp-tx-ht40-mac-wait-diagnostic` uses `diagnostic-tx-wait` to investigate slow
transmissions, including busy-channel conditions. It has no idle-channel
admission limit or throughput floor; a pass means the diagnostic workload
completed, not that the link met a performance target. The separate
`udp-tx-ht40-mac-irq-diagnostic` retains its idle-channel limit and throughput
floor. Publication-to-IRQ samples include hardware waiting and interrupt
entry latency; IRQ-to-service samples measure the subsequent executor handoff.
The wait image combines MAC IRQ and task-poll observations with an explicitly
intrusive `tx-wait-probe` feature. While an aggregate remains published, the
station owner adds observation deadlines at 5, 10, 20 and 40 ms after each
publication. These deadlines preserve the ordinary completion/abort policy;
late wakes skip missed sample points. Each observation reads the typed queue
snapshot before consuming a completion and records both publication age and
observation-deadline lateness. A pending completion can therefore be identified
before normal service acknowledges it.

`hil-tx-wait` records retain the first eight observations in each observed-age
band: below 10 ms, 10–20 ms, 20–40 ms and at least 40 ms (upper bounds excluded).
Independent budgets prevent common short waits from hiding rare long waits.
Additional samples in a full band are counted as dropped. Output is grouped
by age band; timestamps recover chronological order across bands.
The producer uses an atomic append-only buffer;
text is emitted after UDP TX. Queue CCA fields are control settings, not a live
channel-busy measurement. Timer lateness includes executor and interrupt delays
and does not by itself measure how long interrupts were disabled. Ordinary
MAC IRQ, correctness and performance images do not enable this probe.
The shared `mac_active` field retains the reviewed MAC activity encoding;
it does not identify CCA/NAV or timestamp the beginning of a transmission.
Queue and activity fields are sequential reads, not an atomic hardware capture.
Hang/panic fields are cumulative hardware counters from the reviewed RX
statistics decoder, not live receiver-state or channel-busy measurements.
RX MPDU, signal, end, FCS-error and abort counters record receiver progress;
their deltas use wrapping 16-bit subtraction. They are global to the MAC,
not attributed to the queued aggregate or exclusively to the connected BSS.

Large socket buffers, task arenas and ordinary task stacks live in PSRAM.
DMA-visible storage, dedicated trap/interrupt stacks, critical data and ISR
text remain in internal SRAM. Every build audits
placement and compiler stack frames: frames above 8 KiB require an explicit
reviewed allowance, and every frame is rejected above the 50-KiB hard limit.
The separate compiler move limit is 4 KiB. These limits are configured in
`stack.toml`; runtime evidence independently enforces its absolute per-core
headroom. Network endpoint construction also has a limit derived from the
linked CPU1 stack size minus its call-chain reserve. Each core arms a hardware
write watchpoint on the bottom word of its task stack when no debugger owns
the watchpoint. Fatal CPU exceptions report the hart, faulting instruction,
fault address and saved return address through the ROM console. Watchpoints
and stack painting complement the frame audit; individual frame sizes alone
cannot prove the maximum depth of nested or indirect calls.

`data_plane` is selected by the startup command, not by rebuilding. Every
repository scenario selects the production `split-radio-network` topology: it
retains radio and RX protocol on CPU0 and moves only `embassy-net` plus sockets
to CPU1. The protocol still names the CPU0-local composition so a deliberately
constructed external diagnostic can isolate placement, but it is not a catalog
scenario or a default. This is an ownership-preserving executor placement, not
a second radio datapath. Upstream devices retain bounded RX/TX queues and
use Xarxa's global packet pool. Pool exhaustion drops and accounts RX frames
because upstream has no public pool-release notification. One RX drain is
bounded by the endpoint queue depth so a continuously refilling radio cannot
monopolize the network executor. The shared radio still owns the final SRAM
TX allocation and scheduling.

Initialization ends at `WifiIdle`. Credentials arrive only with a STA/AP role
request; scan and monitor do not require a temporary STA. Permanent STA and
AP `embassy-net` devices retain distinct IP/link/RX state while sharing one
tagged physical TX fabric. Role transitions publish link state instead of
reconstructing either device. The target never owns scenario criteria,
expected hashes or stored lab secrets.

Session admission reads functional station link and Block-Ack state from the
production status owner. Diagnostic aggregate counters are observations only
and never authorize traffic or change role behaviour.

## Application boundaries

The `runtime` binary is the HIL application of the shared stage-two boot
contract. `product_hil` owns the radio/network composition
and its persistent observation resources; its `traffic`, `ieee802154` and
`rx_qualification` children own workload and observation duties. The value-only
`rx_statistics` child owns RX counter deltas and wire-evidence conversion.
`network` owns storage for the selected stack/driver, per-interface IPv4
configuration and HIL-only checksum-cost policy. Shared socket workloads use
backend-specific UDP and TCP bindings. UDP RX retains 16 datagrams: Xarxa
selects this depth through its configuration feature, while smoltcp uses
explicit metadata and byte rings. Xarxa packet-pool capacity remains a
separate limit from the socket queue depth.
TX pacing is a workload policy, not a claimed socket queue capacity. TCP buffer
sizes remain application-owned. The upstream global packet pool uses its
default capacity.

`console` retains the coupled UART/session admission, logger serialization
and emergency writer lifecycle. Its `radio` child supplies the product-facing
startup, session and Wi-Fi completion endpoints; memory-only images exclude
that child while retaining the common command admission rules.
The asynchronous USB owner waits for writer release; cancelling that wait
releases its priority. Emergency output remains nonblocking. Diagnostic images
with `driver-observation` reserve 32 text records for synchronous role-transition
bursts; other images reserve eight. Both queues remain bounded SRAM storage.
Overflow warnings distinguish queue exhaustion, immediate-writer contention
and USB write errors, and retain the first queue rejection. Required reports
await admission outside measured work; ordinary radio logging never waits for
USB capacity. Extra diagnostic queue storage costs 9,408 SRAM bytes and does
not change radio pools. Arbitrary sustained logging can still overflow and is
reported, rather than treated as complete diagnostic evidence.

The `diagnostic-memory-benchmark` image excludes `product_hil` and its radio
and network tasks at compile time. It keeps the shared two-core boot,
executors, stack placement and HIL console; CPU1 publishes its spawner but
does not start a network task. The memory task alone claims AXI-GDMA and its
dedicated buffers. Image capability advertisement belongs to `capabilities`,
so reporting memory support does not retain the product owner graph. Its
4,096-byte maximum payload describes the per-frame benchmark command policy,
independently of the product TCP buffer size.

HIL retains its workload-specific `stack.toml` and diagnostic observers.
Board initialization, relocation and interrupt-stack mechanics belong to the
shared platform; application images use the same mechanism through `cargo xtask
build firmware`. A hardware scenario verdict remains a separate HIL responsibility.

## Memory copy measurements

`diagnostic-memory-benchmark` exposes `ProbeMemoryBenchmark` before radio
initialization. Run the `memory-copy-benchmark` scenario through `cargo hil`;
normal radio initialization is unsupported in this image. The task owns
AXI-GDMA channel 0 and dedicated static allocations. This image is separate
from the startup GDMA/SG probe used by `diagnostic-tx-architecture`.

Each request selects CPU copy, blocking GDMA or asynchronous GDMA, an SRAM or
PSRAM source, 1–4096 payload bytes per frame, 1–32 frames and 1–64 measured
iterations. Total payload per iteration is bounded to 49,152 bytes. Four
verified warmup iterations precede the measurements. All modes use separate
frame slots with a common stride: `round_up_64(36 + payload_bytes + 1)`.
Sources start on isolated cache-line boundaries; each internal-SRAM destination
starts at offset 36 with guards before and after its payload. The extra byte
ensures a suffix guard even when payload plus offset ends on a cache line.

Each source and destination arena contains 52,352 bytes, covering the maximum
payload plus bounded per-frame placement overhead. The task owns one source
arena in SRAM, one in PSRAM and one destination arena in SRAM; it reuses these
across requests. Two SRAM descriptor arrays retain 64 items each, 2,048 bytes
in total. GDMA uses the platform's 32-byte burst setting and descriptor builder;
these are experiment policies, not claimed hardware limits.

Every iteration writes the source, poisons each destination payload with the
source's complement and fills guards before timing.
This is a CPU-written source condition, not a cold-cache measurement. The
CPU mode copies the frame slots in a loop. GDMA mode builds one segment list
and submits one descriptor chain per iteration. The measured operation includes
segment-list construction, GDMA preparation, per-segment cache writeback,
publication, completion and cleanup. Source conditioning, full payload/guard verification,
between-iteration yields and UART reporting are outside the interval. Each
reported total sums only measured iterations. The counter boundaries use
compiler memory barriers and RV32 high/low/high reads of the 64-bit cycle and
instruction counters. Monotonic elapsed microseconds remain separate from
cycles; counters do not establish CPU utilization or energy consumption.

A memory fence drains source/destination conditioning before timing starts.
All modes end their measured operation with the same `fence rw, rw` and
compiler barriers, so CPU-copy return and GDMA completion share the boundary
for publishing SRAM data to another memory owner. This final fence is also
included in asynchronous foreground cleanup. It does not replace the explicit
PSRAM cache writeback performed by GDMA preparation.

Foreground counters cover the entire synchronous operation. For asynchronous
GDMA they cover preparation/start, calls to the transfer's `poll`, and cleanup.
They exclude executor and IRQ work outside those windows, while interrupts
inside a window remain included. Sampling overhead is included and can matter
for small copies. The diagnostic does not instrument the DMA ISR separately.

Asynchronous transfer waiting has a 100-ms timeout; the blocking baseline has
a finite 100,000-poll budget. These do not bound synchronous HAL cache
preparation: the pinned HAL waits for cache synchronization under its shared
lock. The host applies a separate 15-second command deadline. A stuck cache
operation requires board reset; it cannot produce a target timeout response.
A returned transfer, data or guard failure quarantines the static allocations
and rejects subsequent benchmark commands until reset. Correctness includes
all measured iterations, not only descriptor completion.

The single-frame and batch scenarios compare the same image, placement and
conditioning with different frame counts. The batch measures AXI-GDMA
scatter/gather staging; it does not measure direct Wi-Fi DMA into PSRAM,
scatter/gather within a Wi-Fi MPDU or an integrated native radio datapath.
