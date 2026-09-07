# ESP32-S31 access-point transactions

This crate composes portable AP policy with S31 key slots and frame publication.
It is independent of the executor and network stack.

| Module | Responsibility |
| --- | --- |
| `engine` | A bounded protocol epoch, peers, security and power-save decisions |
| `hardware` | Borrowed key, receive-policy and TSF authority required by that epoch |
| `transaction` | One finite protocol/TX transaction, retaining its owner until completion |
| `tx`, `ampdu`, `rx` | S31 frame preparation, aggregate admission and receive processing |
| `security` | Key installation and pairwise receive ownership |
| `beacon`, `profile` | Beacon construction and implemented local capability claims |

The transaction owner composes these parts; the hardware contract does not
acquire another radio. The runtime supplies timers, DMA/IRQ progress and
network handoffs. Publication is not successful transmission: resources are
released only through the existing completion or terminal recovery path.
