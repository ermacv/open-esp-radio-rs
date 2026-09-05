# ESP32-S31 Bluetooth ownership

This crate owns chip hardware sequencing and affine radio publication states.
Portable HCI policy and LE Link Layer codecs live in
[`driver/bluetooth`](../../../bluetooth/). Concrete Embassy waiting and session
execution live in the [runtime](../../../runtime/embassy/esp32s31/bluetooth/),
and final storage and hardware composition live in
[integration](../../../integration/esp32s31/embassy/bluetooth/).

| Module under `src/` | Responsibility |
| --- | --- |
| `le/dtm` | Direct Test Mode commands, payloads, event timing, scheduler reservations and active/stopping transitions |
| `le/advertising/legacy` | Legacy advertising preparation, timing, completion and recurring execution |
| `le/advertising/connectable` | Connectable advertising activation, completion and recurring sequence/HCI/state |
| `le/scanning/passive` | Passive scanning activation, recurring execution and completion |
| `le/peripheral` | First HCI handoff, start, connection owner and peripheral completion |
| `scheduler` | Shared scheduler resources and single-item completion |
| `controller` | Shared controller bootstrap and hardware lifecycle |
| `interrupt` | Chip interrupt state and hardware handling |

LE modules are private implementation namespaces; public root exports define
the caller contract. Paths provide protocol context without repeating the role
in each filename. Publication, cancellation, reset and quiescence remain
explicit lifecycle terms. Shared controller/IRQ/scheduler code is not owned by
one LE role.

Boot/controller loops retain their state owners through hardware handoff,
waits and terminal quarantine. Feature gates apply to both the owners and
their unit suites in adjacent child files.
The separate [`memory`](memory/) crate retains controller-SRAM codecs.

See [FEATURES.md](FEATURES.md) for supported and incomplete paths; structural
organization does not extend hardware qualification.
