# HIL memory observations

## RX DMA ownership measurements

`cargo hil run tcp-rx-dma-ownership --network upstream-smoltcp` and
`cargo hil run tcp-tx-dma-ownership --network upstream-smoltcp` select the
separate `diagnostic-rx-ownership` image. They use the HT40 split-core TCP
workload and retain the ordinary throughput and stack checks. TCP TX measures
RX allocation lifetimes for incoming peer traffic, not TX-buffer residence.
Ordinary performance images do not contain this observer.

The DMA arena emits physical-buffer identities at detach, final lease Drop,
software append completion and stopped-ring reclamation. No clock or recorder
belongs to the radio: HIL supplies the cross-core Embassy monotonic clock and
bounded storage. The observer follows rotated descriptor bindings and counts
out-of-order returns independently from ring-order republication.

`ORX_OWN_TIME` reports sample count, summed microseconds and observed maximum
separately for detach-to-return and return-to-software-append. `ORX_OWN` retains
outstanding held/returned counts, their peaks, owners crossing the start of the
window (`carry`) and stopped reclamations. Cross-boundary samples retain their
whole lifetime; they are not clipped into falsely shorter intervals.
`ORX_OWN_CARRY` reports the subsets of hold/append samples that started before
the window, so startup residence is not mistaken for steady traffic residence. Stopped
reclamation is not an append sample. An append timestamp does not prove that
hardware fetched the descriptor or settled a reload.

The target rejects a successful TCP verdict if observation is invalid or has
no completed hold/append samples. Recorder contention, missing transitions,
clock reversal and arithmetic overflow invalidate evidence, with errors sticky
across windows. Reports are emitted after the traffic measurement through the
bounded reliable logger. These observations perturb execution and describe the
instrumented workload, not worst-case timing or a reason by themselves to
remove the upstream adapter's Ethernet copy.

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

## IRQ stack evidence

The shared platform paints dedicated SRAM IRQ stacks before interrupt admission.
HIL samples each on its own hart in thread mode. The CPU1 sampler also returns
its task watermark, so the console does not inspect live foreign stack storage.
Both IRQ measurements accompany Wi-Fi session evidence and stack queries.

The [stack policy](stack.toml) sets `runtime_irq_minimum_free_bytes`; its current
reserve is an engineering guard, not a qualified nesting bound. A sampling
request that cannot complete fails explicitly. Read the
[wire contract](../../protocol/diagnostics.md#stack-measurements) for absence semantics
and the instrumentation's effect on interrupt latency.
