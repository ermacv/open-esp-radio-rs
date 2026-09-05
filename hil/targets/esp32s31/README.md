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

The stage-two `runtime` name and binary identity are part of the
bootstrap/relocation contract. `product_hil` owns the radio/network composition
and its persistent observation resources; its `traffic`, `ieee802154` and
`rx_qualification` children own workload and observation duties. The value-only
`rx_statistics` child owns RX counter deltas and wire-evidence conversion.
`console` retains the coupled UART/session admission, logger serialization
and emergency writer lifecycle. Separating these owners requires an explicit
state handoff, not a folder or type-name rewrite.

Board/linker/bootstrap/stack policy stays with this concrete HIL composition.
The standalone examples use a different boot/linker contract; see the station
example's documented placement limitation before treating source checks as a
flashable production image.
