# ESP32-S31 vendor contract reference

These documents describe pinned public-source and authenticated-artifact
contracts used by this investigation. They explain hardware ordering, software
state, memory ownership and the limits of each observation. They are not run
results, work plans or product readiness declarations.

| Reference | Boundary |
| --- | --- |
| [Bluetooth controller](bluetooth-controller-boundary.md) | Platform lifecycle versus Controller software |
| [Bluetooth interrupts](bluetooth-interrupt-runtime.md) | Primary/NRT acknowledgement and deferred work |
| [Direct Test Mode](bluetooth-direct-test-mode.md) | Descriptor, timing, scheduler and recycle contracts |
| [Legacy advertising](bluetooth-legacy-advertising.md) | PDU memory, event publication and recurrence |
| [Passive scanning](bluetooth-passive-scanning.md) | RX graph, completion and report fields |
| [Peripheral connection](bluetooth-peripheral-connection.md) | Connection memory, anchor timing and ownership |
| [IEEE 802.15.4 lifecycle](ieee802154-lifecycle.md) | Clock/reset, MAC foundation and reviewed register semantics |
| [IEEE 802.15.4 dataplane](ieee802154-dataplane.md) | Static policy, frame storage and IRQ ownership |
| [IEEE 802.15.4 control](ieee802154-control.md) | Ordered state transitions, STOP and timers |

The [project](../README.md) owns input selection, scopes and comparison policy.
[Registers](../../../../../registers/README.md) owns reviewed hardware models
and publication. Production behavior belongs to [driver](../../../../../driver/README.md);
[qualification](../../../../../qualification/README.md) determines readiness
from its declared evidence. Source hashes and artifact identities are
provenance constraints, not a claim that the implementation is equivalent.
