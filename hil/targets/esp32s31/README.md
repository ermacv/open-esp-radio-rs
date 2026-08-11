# ESP32-S31 HIL target

This workspace owns board boot, PSRAM/link placement, Embassy executors,
`embassy-net`, UART transport and qualification workloads. Radio behaviour
belongs in `driver/`; HIL uses the public production constructor and runner.

- `bootstrap`: flash entry and PSRAM handoff;
- `runtime`: role-neutral control plane and runtime-dispatched workloads;
- `telemetry`: qualification-only counters;
- `board`: board boot resources.

There are four images: `boot-smoke`, one universal `qualification` image, and
two explicitly diagnostic images. Qualification contains UDP/TCP
RX/TX/bidirectional workers simultaneously; `SessionConfig` selects work at
runtime. STA/AP roles and workload direction never change image identity.

Large socket buffers and task arenas live in PSRAM. DMA-visible storage, CPU
stacks, critical data and ISR text remain in internal SRAM. Every build audits
placement, compiler stack frames (warning above 8 KiB, rejection above 32 KiB)
and reviewed owner futures. Runtime evidence requires at least 25% free stack
on both cores.

Initialization ends at `WifiIdle`. Credentials arrive only with a STA/AP role
request; scan and monitor do not require a temporary STA. One persistent
`embassy-net` device follows the active role: DHCP/static STA policy is
restored after AP, while AP uses the request's isolated static address. The
target never owns scenario criteria, expected hashes or stored lab secrets.
