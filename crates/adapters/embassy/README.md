# Embassy bindings

These crates bind Embassy facilities to driver contracts. Chip Wi-Fi and
Bluetooth execution lives in the [radio runtime domain](../../runtime/README.md).

| Location | Responsibility |
| --- | --- |
| `esp32s31/executor/src/{executor,time_driver}.rs` | Platform executor wake ABI and Embassy timer queue; applications supply interrupt and timer capabilities |
| `radio/src/` | Mailbox and role-epoch actor binding the portable radio service port |
| `esp32s31/ieee802154/src/` | Acknowledged IRQ token queue and cancellation-safe operation/DMA owners |
| `esp32s31/coex/src/` | Request/reply mailbox and the sole task-side coexistence owner |

An adapter can retain state required by its external contract. IEEE 802.15.4
queues already-acknowledged events, whereas Wi-Fi can coalesce notifications
of durable work. Their overflow and cancellation contracts remain distinct.
The coexistence mailbox serializes requests to one task-side owner; its async
loop is part of that binding and does not imply another radio lifecycle.

`esp32s31/executor` is the Embassy platform binding: executor wake-up and
timer-queue ABI. It has no radio policy or PHY initialization. The concrete
PHY time binding is the [PHY runtime](../../runtime/esp32s31/phy/), while chip
PHY remains executor-independent.

Final memory profiles, static claims, IRQ binding and whole-radio lifecycles
belong to [integration](../../composition/esp32s31/embassy/). Portable Wi-Fi
execution primitives (monitor handoffs, task shutdown, station network
ownership) are runtime code in [`runtime/ieee80211`](../../runtime/ieee80211/),
not executor bindings. The radio adapter depends on the executor-free `oer-radio` service;
the service never depends on an adapter. The [driver map](../../README.md) defines the ownership direction.
