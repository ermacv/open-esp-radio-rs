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

The required image profile runs ordinary code/data from PSRAM while keeping
stacks, DMA arenas and ISR code in internal SRAM. The runtime validates these
ranges before enabling the radio.

Use the host runner as the only command surface:

```text
cargo hil scenarios
cargo hil doctor
cargo hil build <scenario>
cargo hil flash <scenario> --port /dev/ttyACM0
cargo hil station reconnect|ap-loss|ap-absence|stop --serial /dev/ttyACM0
cargo hil traffic rx|tx|bidirectional|tcp-rx|tcp-tx|tcp-bidirectional|icmp ...
```

Credentials, IPv4 policy and retained calibration are sent over the typed HIL
protocol and never select firmware source. Scenario manifests contain only
non-secret build policy. Dated results belong under
`qualification/targets/esp32s31/records/`.

Current migration constraint: `runtime/src/radio_hil/` still contains a
second target composition for custom resource profiles and instrumentation.
It is not a production API and must be deleted after equivalent typed
qualification hooks cover cold-start evidence, traffic counters and injected
faults. New radio behavior must not be added there.
