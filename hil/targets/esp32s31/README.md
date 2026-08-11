# ESP32-S31 HIL target

This private target workspace owns board boot, PSRAM/link placement, Embassy
executors, `embassy-net`, UART transport and qualification workloads. It must
exercise the public production radio composition; chip behavior belongs in
`driver/`.

Layout:

- `bootstrap`: flash entry and external-memory handoff;
- `runtime`: target control plane and network workloads;
- `telemetry`: qualification-only counters;
- `board`: board-specific boot resources.

The required image profile runs ordinary code/data, RX handoff/reorder and
monitor capture from PSRAM. Allocations actually addressed by Wi-Fi DMA,
the current stack and ISR text stay in internal SRAM. Runtime checks validate
section probes; the host ELF memory policy validates the production owners.

Use the host runner as the only command surface:

```text
cargo hil scenarios
cargo hil doctor
cargo hil build <scenario>
cargo hil flash <scenario>
cargo hil station reconnect|ap-loss|ap-absence
cargo hil wifi stop|start|scan|monitor|roundtrip
cargo hil traffic rx|tx|bidirectional|tcp-rx|tcp-tx|tcp-bidirectional|icmp ...
```

Every `cargo hil build` emits LLVM stack-size metadata for both images and
writes `runtime-stack.txt` and `bootstrap-stack.txt`. Frames above 8 KiB are
reported for review; a frame over 32 KiB fails before packing. Each UDP/TCP
session also carries typed CPU0/CPU1 high-water evidence and fails on the host
below 25% free stack.

Credentials, IPv4 policy and retained calibration are sent over the typed HIL
protocol and never select firmware source. Scenario manifests contain only
non-secret build policy. Dated results belong under
`qualification/targets/esp32s31/records/`.

The product HIL starts the same public constructor and sole runner as an
application. Qualification hooks observe that path; HIL contains no second
radio, station, DMA or ISR composition. A future low-level layer probe must be
a separate target package and must not compose station or `embassy-net`.
