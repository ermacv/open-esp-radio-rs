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

LE modules are private implementation namespaces; existing root exports retain
the caller contract. Paths provide protocol context without repeating the role
in each filename. Publication, cancellation, reset and quiescence remain
explicit lifecycle terms. Shared controller/IRQ/scheduler code is not owned by
one LE role.

The complete boot/controller loops stay with their state owners. Module splits
must preserve hardware handoff order, effective cfg, task cancellation points
and terminal quarantine. Unit suites remain in child files beside those owners.
The separate [`memory`](memory/) crate retains controller-SRAM codecs.

See [FEATURES.md](FEATURES.md) for supported and incomplete paths; structural
organization does not extend hardware qualification.
