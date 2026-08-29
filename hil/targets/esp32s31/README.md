# ESP32-S31 HIL target

This workspace owns board boot, PSRAM/link placement, Embassy executors,
`embassy-net`, UART transport and HIL workloads. Radio behaviour
belongs in `driver/`; HIL uses the public production constructor and runner.

- `bootstrap`: flash entry and PSRAM handoff;
- `runtime`: role-neutral control plane and runtime-dispatched workloads;
- `telemetry`: HIL-only diagnostic observers;
- `board`: board boot resources.

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
placement, compiler stack frames (warning above 8 KiB, rejection above 32 KiB)
and reviewed owner futures. Runtime evidence enforces the absolute per-core
headroom declared in `stack.toml`.

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
