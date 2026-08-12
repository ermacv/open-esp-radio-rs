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
and reviewed owner futures. Runtime evidence enforces the absolute per-core
headroom declared in `stack.toml`.

`data_plane` is selected by the startup command, not by rebuilding. The
default `single-core` topology keeps radio, RX protocol, `embassy-net` and
socket workloads on CPU0. `split-radio-network` retains radio and RX protocol
on CPU0 and moves only `embassy-net` plus sockets to CPU1; it is a comparative
last-resort optimization. Network ingress is time-bounded at 250 us and owns
one TX credit beyond the 64 application credits, so saturated egress cannot
prevent the RX/TX-token handoff required by `embassy-net`.

Initialization ends at `WifiIdle`. Credentials arrive only with a STA/AP role
request; scan and monitor do not require a temporary STA. One persistent
`embassy-net` device follows the active role: DHCP/static STA policy is
restored after AP, while AP uses the request's isolated static address. The
target never owns scenario criteria, expected hashes or stored lab secrets.
