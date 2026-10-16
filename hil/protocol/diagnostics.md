# Platform HIL diagnostics

## Platform reset and watchdog

`BluetoothDtmEvidence::reset_reason` classifies the current platform boot as
software reset, MWDT1 reset or another cause. HCI Reset does not change this
value. Archived captures retain their original wire schema and evidence layout.

`GetBootStatus` returns `BootEvidence` for the boot identified by the envelope.
`ResetReason` is a platform value shared by all images; querying it does not
issue HCI Reset or change radio state. `BootEvidence::platform_panic` is the
record the platform's panic entry left (`panic::take_previous`, taken once
per boot): the panic's hart, whether it ran on the interrupt stack and the
interrupted PC, and the end of its file path, line and column.

`system::InjectPanic` is served only by the panic-reset image, whose panic
entry has no HIL report: the image acknowledges with `PanicInjected` and
panics in thread context, the entry records the panic and resets the chip
through `esp_hal::system::software_reset`, and the next boot reports a
software reset and the record.

`system::DisableUsb` is served only by the `diagnostic-usb-jtag-off` image
(the system watchdog image with the agent's `usb-jtag-off` feature): it
acknowledges with `UsbDisabled`, then drops the USB Serial/JTAG's D+ and D-
pull-ups and takes it off its pads. Off its pads alone, the device stops
answering but the host keeps it enumerated; without the pull-up the host sees
it disconnect. Only a reset of the chip brings it back. The host arms
`SerialCapture::expect_detach` before the request, so the end of the stream
that follows closes the capture without a failure.

`system::WatchdogTest` is served only by the dedicated watchdog image.
The dedicated radio-free image's one-second engineering budget exercises completion, a synchronous
poll that never returns, cancellation, missing completion and late restoration
of the SoC deadline service. The correlated event acknowledges the selected
mode; only `Complete` acknowledges disarming. Other modes must produce a new
boot and MWDT1 reset reason. This does not inject a PHY failure or measure RF
cessation. The separate DTM reset scenario retains its independent peer gates.

## Event trace

An image that links `oer-trace` keeps typed trace entries, and optionally
snapshot slots of register windows, in reset-retained memory. `telemetry::ControlTrace`
reports the trace's `TraceStatus`, starts it over with a channel mask or
changes the mask while it runs; `telemetry::GetTraceEntries` pages out up to 16 complete
entries from a storage slot, and `telemetry::GetTraceSnapshot` one slot's snapshot in
pages of 64 words. Entries travel raw, with their event kind; the host decodes
their words with the same event types the target records.

A hang or a panic freezes the trace before the reset, as can a driver's own
trigger. The next boot then holds the frozen trace, reported as
`holding_previous`, until the host has paged it out and starts recording, or
for 30 s at most; a clean boot records at once on every channel. Reading the
trace does not change radio state or consume retained results. An image
without a trace reports `installed: false`.

## Program-counter profile

No image samples the program counter: the sampler (`pc-profile`) and its
agent modules were deleted (#762, tag `legacy-wifi-2026-10-16`). The `telemetry` profile
messages (`ProfileControl`, `ProfileStatus`, `GetProfileSamples`) remain in
the protocol with no producer.

Sampling perturbs timing: a profiled run is diagnostic evidence, never a
throughput or timing measurement.

## Stack measurements

`StackUsage` carries task watermarks and optional dedicated IRQ watermarks for
both harts; a network session's evidence reports it. A hart that runs no
executor reports `None`. Images with shared task/IRQ stacks report `None` for
both dedicated IRQ fields.

`system::GetInterruptStacks` reports the dedicated IRQ watermarks in a short
response. Bluetooth reports its CPU0 dedicated IRQ watermark and
`None` for its inactive CPU1. The secure GATT workload checks this response at
its boundaries.

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
