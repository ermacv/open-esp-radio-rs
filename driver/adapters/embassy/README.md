# Embassy bindings

These crates bind Embassy facilities to driver contracts. Chip Wi-Fi and
Bluetooth execution lives in the [radio runtime domain](../../runtime/README.md).

| Location | Responsibility |
| --- | --- |
| `esp32s31/runtime/src/{executor,time_driver}.rs` | Platform executor wake ABI and Embassy timer queue; applications supply interrupt and timer capabilities |
| `ieee80211/src/monitor/` | Bounded capture and injection handoffs |
| `ieee80211/src/{connected_tasks,station_network,stack_boundary}.rs` | Task shutdown, association-scoped network ownership and explicit polling boundary |
| `esp32s31/ieee802154/src/` | Acknowledged IRQ token queue and cancellation-safe operation/DMA owners |
| `esp32s31/coex/src/` | Request/reply mailbox and the sole task-side coexistence owner |
| `esp32s31/ieee80211-compat/src/` | Compatibility network endpoint bound to chip radio execution |

An adapter can retain state required by its external contract. IEEE 802.15.4
queues already-acknowledged events, whereas Wi-Fi can coalesce notifications
of durable work. Their overflow and cancellation contracts remain distinct.
The coexistence mailbox serializes requests to one task-side owner; its async
loop is part of that binding and does not imply another radio lifecycle.

`esp32s31/runtime` means the Embassy platform runtime: executor wake-up and
timer-queue ABI. It has no radio policy or PHY initialization. Concrete PHY
time bindings live in the radio packages as explicit `time::phy` leaves, while
chip PHY remains executor-independent.

Final memory profiles, static claims, IRQ binding and whole-radio lifecycles
belong to [integration](../../integration/esp32s31/embassy/). The generic Wi-Fi
adapter depends on radio-facade contracts and does not own the product
supervisor. The [driver map](../../README.md) defines the ownership direction.
