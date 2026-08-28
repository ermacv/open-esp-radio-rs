# ESP32-S31 Core0 RX load reduction

Date: 2026-08-28.

This record describes the production architecture changes which reduced the
saturated STA RX radio-task load from approximately 95% of Core0 to less than
40%, without turning packet loss, a smaller air workload, or intrusive
profiling into an apparent CPU improvement. It also records the rejected
alternatives so the same experiments are not repeated.

This is an engineering record for the integrated source tree. The canonical
qualification status remains the machine-checked target specification and its
sealed HIL bundles.

## Result

The starting profile, run `1787887802948-0032fe68`, delivered 107.152 Mbit/s
and measured 3,671,037,068 radio-poll cycles in 12.062768 seconds at 320 MHz:
95.10% Core0 residence. A later low-overhead control on the same old
architecture, run `1787891588059-00344fe3`, measured 92.185% task residence
at 106.340 Mbit/s and 32,350 radio cycles per DMA MPDU. The old owner performed
52,763 DMA calls for 109,127 units, only 2.068 MPDUs per call.

The final architecture was checked by two independent measurement methods:

| Measurement | Run | Delivered RX | Core0 radio load |
| --- | --- | ---: | ---: |
| coarse `mcycle` profile | `1787921744079-003e5e1c` | 102.026 Mbit/s | 1,390,549,881 cycles, 35.97% of one 320 MHz core |
| top-level task residence only | `1787922238628-003e7dc9` | 103.836 Mbit/s | 4.486043 / 12.066368 s, 37.18% |

The coarse run measured 104,856 DMA units in 8,760 DMA calls, or 11.97
MPDUs per call. It received only 406 real RX IRQ publications, approximately
258 MPDUs per IRQ. The radio task did not become cheaper by receiving less:
the independent monitor decoded 106,418 unique BlockAck-covered MPDUs in the
residence-only run, while the target received 106,397 benchmark datagrams.
The host route was the wired `enp0s20f0u2u4c2` interface with source
`192.168.178.129`; laptop WLAN was not the data route.

The two final measurements use different instrumentation and independent
ELFs. Their agreement at 36--37% rules out a coarse-profiler accounting
artifact. `Task residence` means time spent inside the top-level radio future
poll, including memory stalls and synchronous child work. It is not a claim
that the ALU executed useful instructions for exactly that fraction of time.

Throughput is lower in these two particular runs than in the earlier
107--109 Mbit/s controls because the AP supplied less traffic on air. The
target delivered essentially every independently observed BA-covered MPDU.
Therefore 102--104 Mbit/s is not evidence of a new target CPU ceiling.

## Root cause

The old production lifecycle already had the intended high-level shape:

1. a real RX interrupt masks the RX source and wakes the Core0 owner;
2. the owner drains one frozen DMA completion frontier;
3. the source is unmasked only after a proven terminal drain;
4. a completion which races the unmask remains latched by the level route.

The defect was the continuation policy inside the masked epoch. Recycling and
appending descriptors can produce `RecycledAppendPending`: the hardware has
accepted more descriptor storage, but the next completion frontier may still
be arriving. There is no guaranteed new interrupt merely because descriptors
were appended. The old code therefore posted an immediate software wake after
every such result.

At saturated HT40 reception the software wake was normally earlier than the
rest of the PPDU completion. Core0 repeatedly crossed the complete
scheduler/DMA/protocol boundary for about two newly visible MPDUs even though
the AP was transmitting almost full BA16 aggregates. Source IRQ moderation
had already removed the hard-IRQ storm, but the immediate software
continuations recreated a high-frequency bottom-half polling loop. Fixed
runner setup, accepted-list observation, protocol entry and executor costs
were paid many more times than required.

The causal correction is a bounded delayed continuation only for the
recycled-append state. The RX source stays masked and the same owner keeps the
drain epoch, but the executor may service other work while a short timer lets
the next physical DMA frontier accumulate. Terminal completion, direct BASE
repair and budget-exhaustion continuations remain immediate. An idle-to-active
frame is still interrupt-driven and is never delayed.

This changed the service geometry from approximately 2.07 to 11.97 MPDUs per
DMA call. The approximately sixfold batching increase amortizes the fixed
costs and is the primary reason the Core0 load fell by more than a factor of
2.5.

## Optimization sequence and evidence

### 1. RX source moderation

Connected STA now enables the same source-moderation lifecycle which was
already present for AP operation. The ISR acknowledges and masks the source;
only the terminal bottom-half transition unmasks it.

The exact same-image A/B reduced real RX IRQ posts from 1.0000 to 0.0168 per
MPDU, a 98.32% reduction, and reduced retired instructions per MPDU by 4.90%.
Radio cycles changed by +0.23%, within a tie. This fixed a real ISR/publication
defect, but it did **not** cause the 95-to-40% reduction. It established that
the remaining high load was made of software continuations, not hard IRQs.

The current final run goes further because delayed continuation holds one
masked drain epoch across many frontiers: 406 real IRQ publications covered
104,856 DMA units.

### 2. Bounded recycled-append coalescing

The main optimization replaces the unconditional immediate software repost
after `RecycledAppendPending` with a timer-backed continuation. Controlled
delay sweeps were performed before choosing production policy.

For runtime ELF SHA-256 `2e29ff06efbb...`:

| Recycled-append delay | Core0 radio residence | Delivered RX |
| ---: | ---: | ---: |
| immediate | 11.296303 s | 106.166 Mbit/s |
| 64 us | 11.417832 s | air-limited control |
| 128 us | 10.628572 s | air-limited control |
| 256 us | 8.221583 s | air-limited control |
| 256 us repeat | 8.238542 s | air-limited control |

The 64 us result is important: adding any timer was not sufficient. The
window had to be long enough to change the DMA frontier geometry. Later 512
us and 1024 us/adaptive steps reduced residence further while preserving the
delivered MPDU frontier.

The production policy is adaptive and bounded by observed batch geometry:

- an empty recycle-only confirmation preserves the established level in the
  current masked epoch;
- long-frame traffic ramps through 128, 256, 512 and 1024 us, reaching the
  largest window only after a proven burst;
- traffic averaging less than 1024 bytes per completed unit uses a bounded
  512 us policy rather than the long-frame ramp;
- any progress result other than recycled-append continuation resets the
  coalescing state;
- terminal, frontier-repair and budget paths never wait for this timer.

This is interrupt moderation plus bottom-half coalescing, not a sleep inserted
in the ISR and not an unbounded poll loop.

### 3. Larger physical ring with bounded retained ownership

The DMA ring was increased from 64 to 96 descriptors. Upper-layer retained
ownership remains capped at 32, so at least 64 buffers remain in the radio
ownership domain. This extra physical capacity lets the masked coalescing
window absorb burst completion without reducing the existing upper-layer
backpressure bound.

The distinction between `radio-domain` and `DMA-armed` is deliberate. The
96/32 split does not mathematically guarantee 64 armed descriptors at every
instant. Current accepted-list telemetry measures the real hardware frontier:
all 8,760 samples in the final coarse run reported at least 49 remaining
accepted-list entries. There were no accepted-list exhaustion episodes, DMA
`BUFFER_FULL`, FIFO overflow, pool block, software drop, or BA gap event.

BA remains 16. Descriptor depth is hardware buffering; BA width is protocol
reordering. The ring growth supplies several bounded BA16 bursts without
changing the negotiated agreement.

### 4. Native RV32 ring bookkeeping

Ring bookkeeping was changed to support 96 entries without adding avoidable
RV32 hot-path cost:

- the observed membership mask is three native `u32` words instead of a
  software-emulated `u64` mask;
- observed-prefix discovery uses word chunks and trailing-zero scans instead
  of rescanning every retained descriptor;
- wrap addition and ring distance use bounded branch arithmetic rather than
  `% 96` in the hot path;
- pending-tail state uses an index-plus-one sentinel instead of a multiword
  `Option<usize>` representation;
- the next unobserved descriptor is derived from the already computed prefix;
- accepted-list pressure is exposed as telemetry without promoting a
  projected address to an ownership proof.

These changes make the larger ring and wider DMA batches practical. No exact
individual cycle saving is assigned to them because they were not each
isolated in a same-ELF HIL A/B.

### 5. Synchronous direct dispatch for the ordinary RX case

The normal in-order protected QoS frame can now cross BA ingress and Ethernet
publication synchronously in one Core0 protocol batch. It is accepted only
when all of these conditions are already proven:

- the sequence is immediately deliverable and does not open or advance a
  reorder gap;
- the frame is not A-MSDU;
- exactly one Ethernet frame results;
- the destination has immediate bounded staging credit.

The old asynchronous path remains the correctness fallback for gaps,
reordering, A-MSDU, fragmentation and capacity pressure. The preflight is
non-mutating on rejection, so fallback observes the original state.

This optimization is useful but not the main load reduction. With the exact
same ELF SHA-256 `6d9a5ae4b427...` and the same 512 us continuation policy,
direct dispatch used 6.130518 seconds of radio residence versus 6.503700
seconds for forced asynchronous dispatch: approximately 5.7% less. Direct
dispatch without delayed continuation still used about 11.3 seconds, proving
that it did not solve the original problem by itself.

### 6. One scheduler preflight per direct batch

After direct dispatch was proven, stop/control generation and deadline checks
were hoisted out of the per-frame ordinary loop. They run once before a
synchronous direct batch. If a frame requires asynchronous fallback, the full
checks are restored before crossing that scheduling boundary.

The latest deep-profile A/B reduced between-frame-to-dequeue cost from 965.9
to 779.9 cycles per MPDU (-19.25%), protocol-poll work from 8265.5 to 8035.3
cycles per MPDU (-2.79%), and complete runner work from 13243.6 to 13042.4
cycles per MPDU (-1.52%). Ordering semantics were not weakened: stop and
control remain higher-priority boundaries, and the direct batch cannot await.

### 7. Affine SPSC and Core1 bounded polling

The physical Core0-to-Core1 packet handoff uses a bounded affine SPSC instead
of an Embassy channel. Earlier same-path profiling measured the transition at
approximately 2,824 rather than 3,696 cycles per MPDU, saving about 872 cycles
per MPDU and about 1,010 cycles in the complete runner. The queue has separate
producer and consumer capabilities so the retained producer can survive a STA
reconnect without creating a second endpoint.

The Embassy network fork separately bounds Core1 ingress polling to 32 packets
against the 64-packet HIL socket reserve. This prevents the network future from
starving the UDP task. It is not credited with reducing Core0 load, but it is
required to make low radio residence meaningful: packets must still reach the
application.

Final production-style packet-rate control
`1787922473246-003e8c12` delivered 40.001 Mbit/s with 512-byte UDP payloads.
The host sent 117,189 datagrams and the target received 117,185 through the
terminal tail with zero internal sequence gaps at approximately 9.77 kpps.

### 8. Selective SRAM placement retained from earlier work

Small CCMP/hot validation helpers remain explicitly placed in internal hot
text. Earlier intrusive measurements reduced the affected data-view work from
approximately 2,329 to 1,660 cycles per MPDU and protocol-poll work by about
8.3%. Broadly moving the radio or network stack into SRAM was rejected: it
would hide layout sensitivity, consume scarce memory and did not address the
measured continuation-frequency problem. SRAM is used for compact,
latency-sensitive helpers and descriptor metadata, not as a substitute for
the batching architecture.

## Current production ownership model

Core0 owns Wi-Fi DMA, 802.11 MAC processing, CCMP and BA/reorder. Core1 owns
IP, UDP and sockets. This split is retained deliberately:

```text
RX level IRQ
  -> mask + acknowledge
  -> Core0 frozen DMA frontier
  -> Core0 MAC / CCMP / BA
  -> direct ordinary publish or bounded fallback
  -> affine SPSC
  -> Core1 IP / UDP / socket

RecycledAppendPending
  -> keep IRQ masked
  -> bounded adaptive timer
  -> drain a wider frontier

Drained
  -> ordered unmask
```

Descriptor ownership is still retained with the packet lease. This coupling
is bounded by the 32-entry retained limit and was not a performance bottleneck
in the measured workload. The ordinary path remains zero-copy across the
radio protocol owner; selective copies are used only when protocol lifetime
requires buffering such as reorder/reassembly.

## Why this differs from the Ethernet example

The sibling ESP32-S31 Ethernet example uses 56 RX descriptors and six TX
descriptors, fixed 1536-byte internal-SRAM buffers, an IRAM ISR and synchronous
same-core `RxToken::consume`. The GMAC owner walks until the first DMA-owned
descriptor, rearms it immediately after the synchronous stack callback, and
then unmasks its interrupt. Its raw ring reaches 870--888 Mbit/s and a path
with one complete CPU data pass reaches 217--239 Mbit/s; full TCP is lower.

The useful transferable principle was not that Ethernet DMA is automatically
cheaper. It was that a finite owner should amortize interrupt and service
boundaries across a real burst. Wi-Fi cannot copy the exact descriptor
lifetime because BA/reorder, CCMP, the accepted-list/LAST hardware contract
and a cross-core packet lease exist. The new Wi-Fi design therefore keeps its
typed retained ownership but adopts bounded interrupt/drain coalescing.

An earlier experimental early-unmask policy produced approximately 54,000 RX
IRQs in 12 seconds. That was a rejected A/B, not the current production
behavior. Current production keeps the source masked through the drain epoch
and needed only 406 real IRQ publications in the final coarse run.

## Measured non-solutions

The following paths were tested and are not part of the performance design:

- **Dynamic replacement/page-pool RX.** The tested implementation added about
  1,684 cycles per MPDU and reduced throughput to 94--96 Mbit/s versus a
  101.724 Mbit/s retained control. It solved no measured starvation.
- **Copy plus immediate descriptor release.** It did not improve ceiling
  throughput and increased Core0 cycles by 22.3%.
- **Immediate same-turn re-probing.** It generated mostly empty DMA probes and
  changed total cycles per frame by only -0.025% while increasing DMA work by
  21.52%.
- **Reduced scheduler continuation after a cooperative yield.** It removed
  local scheduler work but reduced DMA batch width; total cycles per frame
  remained effectively unchanged.
- **Source moderation alone.** It removed 98% of IRQ publications but did not
  reduce cycles, which is why the software continuation was investigated.
- **Direct dispatch alone.** It saved approximately 5.7% with coalescing but
  remained near the old 11.3-second residence without it.
- **Broad cache-layout or SRAM relocation.** Cache layout affects individual
  ELF results, but no layout-only change explains the measured transition
  from approximately two to twelve MPDUs per DMA call. It is not the basis of
  the production fix.
- **Moving the IP stack to Core0.** Core0 was the saturated owner in the old
  architecture; adding IP/socket work there would worsen the proven boundary.
- **Larger BA.** BA16 already produces full aggregates and the target receives
  nearly every BA-covered MPDU. BA32 changes protocol/credit risk, not the
  identified software continuation cadence.

## Correctness invariants and final evidence

The load result is accepted only with all of the following facts:

- the initial RX edge is never timer-delayed;
- IRQ remains masked until a proven drain, and terminal/repair/budget work is
  reposted immediately;
- ordinary direct dispatch cannot await or mutate state before fallback;
- ring size is 96 and upper retained ownership is bounded at 32;
- BA remains 16 and the observed traffic is HT40/MCS7;
- the final coarse run had 104,676 direct frames and only 180 asynchronous
  frames, with no protocol drop;
- all 8,760 accepted-list samples retained at least 49 entries;
- `BUFFER_FULL`, FIFO overflow, pool block and software-drop counters were
  zero;
- independent capture and target delivery agreed at the MPDU/datagram
  frontier;
- the separate 512-byte control had no internal UDP sequence loss.

The current HIL startup report prints the effective shipping policies
(`direct-immediate` and `adaptive-probe`) rather than the ignored diagnostic
selector defaults. This prevents a production ELF from being mislabeled in a
future record.

## Source map

The production changes are concentrated in:

- `driver/adapters/embassy/esp32s31-wifi/src/datapath/service.rs` -- masked
  continuation lifecycle and adaptive deadline;
- `driver/adapters/embassy/esp32s31-wifi/src/datapath/mod.rs` -- adaptive
  policy and owner state;
- `driver/adapters/embassy/esp32s31-wifi/src/roles/station/rx_protocol/` --
  synchronous ordinary dispatch and batch preflight;
- `driver/chips/esp32s31/wifi/dma/src/rx_ring.rs` -- 96-entry native-word ring
  bookkeeping and accepted-list frontier;
- `driver/adapters/embassy/esp32s31-wifi/src/composition/resources.rs` and
  `datapath/rx/dma.rs` -- qualified 96/32 storage geometry;
- `driver/adapters/embassy/esp32s31-wifi/src/datapath/rx/frontier/` -- typed
  frozen-frontier and recycle lifecycle;
- `hil/protocol`, `hil/host/runner` and `hil/targets/esp32s31/runtime` --
  same-ELF selectors, independent residence/cycle reports and route/air
  evidence.

## Remaining work

Core0 is no longer the measured full-size RX ceiling at 102--104 Mbit/s. The
remaining approximately 36--37% is real DMA/MAC/CCMP/BA and orchestration work,
but this record does not claim every remaining cycle is irreducible. Further
optimization should begin with normalized cycles per MPDU and DMA batch width,
not throughput alone.

The next qualification should cover the final integrated ELF across cold
boot/reconnect, TX, full duplex, mixed small/large RX and latency-sensitive
control traffic. A future change is a regression if it restores narrow DMA
batches, delays terminal ownership, consumes the 96/32 reserve, or loses the
Core1 512-byte fairness control even when headline throughput looks unchanged.
