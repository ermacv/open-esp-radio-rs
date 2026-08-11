# HIL host

`runner/` owns the typed CLI, scenario catalog, build/flash orchestration and
UART evidence. `linux-net/` contains only privileged fixture operations.

Public commands:

```console
cargo hil doctor
cargo hil scenario list
cargo hil scenario validate [id]
cargo hil image build|flash <boot-smoke|qualification|diagnostic-task-poll|diagnostic-rx-delivery>
cargo hil device status
cargo hil run <scenario-id>
cargo hil run-all [--tag qualification]
```

Scenarios are versioned TOML files in `hil/scenarios`; they contain workload,
isolation and acceptance criteria, never serial paths or secrets. Machine-local
device, STA/AP and OpenWrt values live only in mode-0600 `hil/local.toml`.

`run-all` groups scenarios by image class, so changing UDP/TCP direction or
rates does not rebuild or reflash firmware. Independent scenarios reset the
target. A future multi-cell workload may opt into one-boot `matrix-session`;
ordinary scenario files must use `reset`.

AP scenarios lease the laptop WLAN as one managed WPA2 client, without a
gateway, and restore NetworkManager on every return path. Lifecycle, ICMP,
UDP and TCP are independent workloads over the same qualification image.
Each records per-cycle AP evidence in `access-point-report.json`; AP IP policy
belongs to HIL, not to the radio driver request.

Each run owns one directory under `target/hil/esp32s31/runs/<id>/` containing
`resolved-scenario.json`, `result.json`, `uart.log`, `protocol.jsonl` and its
workload report. Reconnect stores UART/protocol pairs per boot. Runner output
is JSON on stdout; diagnostics and progress belong on stderr.

`boot-smoke` intentionally precedes the radio protocol and proves only runtime
relocation plus one Embassy timer wake. It uses its single fixed PASS record;
all radio, lifecycle and traffic qualification is typed protocol evidence.
