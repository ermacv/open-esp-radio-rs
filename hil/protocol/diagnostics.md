# Platform and memory HIL diagnostics

## Platform reset and watchdog

`BluetoothDtmEvidence::reset_reason` classifies the current platform boot as
software reset, MWDT1 reset or another cause. HCI Reset does not change this
value. The `bluetooth_watchdog_reset` capability identifies the dedicated
DTM fault-injection image; it is mutually exclusive with automatic
`bluetooth_phy_maintenance`. Each reset scenario requires its exact mechanism.
Archived captures retain their original wire schema and evidence layout.

`GetBootStatus` returns `BootEvidence` for the boot identified by the envelope.
`ResetReason` is a platform value shared by all images; querying it does not
issue HCI Reset or change radio state.

`SystemWatchdogTest` requires the independent `system_watchdog` capability.
The dedicated radio-free image's one-second engineering budget exercises completion, a synchronous
poll that never returns, cancellation, missing completion and late restoration
of the SoC deadline service. The correlated event acknowledges the selected
mode; only `Complete` acknowledges disarming. Other modes must produce a new
boot and MWDT1 reset reason. This does not inject a PHY failure or measure RF
cessation. The separate DTM reset scenario retains its independent peer gates.

## Memory copy benchmark

`ProbeMemoryBenchmark` runs one pre-initialization CPU, blocking GDMA or async
GDMA copy from SRAM/PSRAM into SRAM. A request specifies 1..=4096 payload bytes
per frame, 1..=32 frames and 1..=64 measured iterations. Each iteration copies
at most 49,152 payload bytes, excluding storage padding and guards. The CPU
copies frames in a loop; GDMA uses one scatter-gather chain per iteration.
`MemoryBenchmarkCompleted` echoes the request and retains completed iterations,
terminal correctness status and separate elapsed/foreground counter scopes.
Completed iterations account for entire batches whose payloads and guards
passed verification; a partially completed batch does not add an iteration.
Foreground means the whole CPU/blocking operation, or async prepare/start,
poll and cleanup windows. IRQs inside those windows remain included. These
values do not measure CPU utilization. The host imposes a per-case response
deadline; a target stalled inside synchronous hardware preparation may need
reset. Feature discovery identifies images implementing this diagnostic.

## Stack measurements

`StackUsage` carries task watermarks and optional dedicated IRQ watermarks for
both running Wi-Fi harts. CPU1 samples its own stacks through a bounded
request/response task; CPU0 never scans its live stack storage. A missing
response fails rather than publishing a cached measurement. Images with shared
task/IRQ stacks report `None` for both dedicated IRQ fields.

`QueryInterruptStackUsage` provides a separate short response without enlarging
Bluetooth peripheral snapshots or the wire-frame limit. Bluetooth reports its
CPU0 dedicated IRQ watermark and `None` for its inactive CPU1. The runner checks
this response after peripheral commands, including physical retirement. Wi-Fi
accepts this query under the same idle-session conditions as `QueryStackUsage`.

IRQ sampling runs in thread mode with local interrupts masked during the SRAM
scan. It therefore adds a short interruption to RF servicing; these diagnostic
runs are not a timing baseline with instrumentation removed. The image's stack
policy supplies the nonzero required reserve. These are observed boot-lifetime
watermarks, not worst-case bounds or evidence of every possible nested IRQ.

The separate `bluetooth-trouble-secure-gatt-timing` scenario scans IRQ watermarks
only before and after traffic, retaining the same pairing/restart/fault-close
checks. Its `irq_sampling = boundary-only` evidence is an observer comparison,
not a replacement for the ordinary `every-snapshot` memory-stress scenario.
Protocol queries and task-stack measurements still add observer overhead.
