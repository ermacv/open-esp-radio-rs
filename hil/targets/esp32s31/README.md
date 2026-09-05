# ESP32-S31 HIL target

This workspace selects the shared board boot and memory profile and owns Embassy executors,
`embassy-net`, UART transport and HIL workloads. Radio behaviour
belongs in `driver/`; HIL uses the public production constructor and runner.

- `runtime`: role-neutral control plane and runtime-dispatched workloads;
- `telemetry`: HIL-only diagnostic observers;

[Shared platform](../../../platform/esp32s31/README.md) owns the board profile,
bootstrap, linker scripts and stage-two entry used by HIL and examples.

`performance` contains no driver observer or scheduler instrumentation.
`correctness` adds value-only driver observations, while the `diagnostic-*`
images add one explicitly named hot-path probe. `diagnostic-mac-irq` alone
extends the SRAM-closed ISR graph to sample publication-to-IRQ and
IRQ-to-bottom-half latency. UDP/TCP RX/TX/bidirectional
workers remain runtime-selected; STA/AP roles and workload direction do not
change image identity. The task-poll diagnostic accepts both data-plane
placements and retains the ordinary 16-KiB CPU1 owner stack in either case.

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
a second radio datapath. Network
ingress is time-bounded at 250 us and owns
one dedicated TX credit per permanent endpoint beyond the 64 application
credits. Saturated egress therefore cannot prevent either STA or AP from
receiving the paired RX/TX-token handoff required by `embassy-net`. Application
credits are otherwise elastic: a standalone role may use the complete pool,
and real concurrent contention is resolved by waking one active waiting VIF on
each returned credit instead of applying a permanent per-role quota.

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
`console` retains the coupled UART/session admission, logger serialization
and emergency writer lifecycle. Its `radio` child supplies the product-facing
startup, session and Wi-Fi completion endpoints; memory-only images exclude
that child while retaining the common command admission rules.

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
