# Network implementation

This document describes the IP stack the Wi-Fi composition uses and what it
changes at the Wi-Fi boundary. [Wi-Fi network integration](wifi-egress.md)
defines packet ownership and radio execution; component READMEs describe their
own APIs.

## The owned Xarxa/Embassy stack

The repository has one network implementation: **owned Xarxa/Embassy**. It is
the default of the product library, the examples and every HIL image.

| Selection | Value |
| --- | --- |
| Product Cargo feature | `owned-network` (default of `oer-esp32s31-ieee80211-system`) |
| Facade feature | `owned-xarxa` |
| Firmware and HIL selector | `--network owned-xarxa` (the default and only value) |
| Adapter | `oer-embassy-net-owned` |

| Crate | Source |
| --- | --- |
| `embassy-net` | The [owned Embassy fork](https://github.com/ermacv/embassy/tree/4868fd9acfd4191173cd6531b537d6009d3ae5d0) |
| `xarxa`, `xarxa-driver` and its packet pool | The [owned Xarxa fork](https://github.com/ermacv/xarxa/tree/9e0e3293724c30c5892e6af56150c849f5231eb2); one revision pins both crates |

These Git revisions are reviewed pins, not tracking branches. Each fork
follows upstream by periodic merges into its `oer/main` branch, never by
rebasing; the `UPSTREAM.md` at each fork's root names the last merged upstream
commit and the upstream changes the fork does not take. Dependency
aliases such as `embassy-net-owned` in application manifests name the package
`embassy-net`; the alias helps Rust source identify the contract and is not a
separate published crate. `cargo xtask check network` rejects any other network
stack source in the product, example and HIL graphs.

`embassy-net` is upstream's multi-interface API: `Stack::new` takes the
stack storage, a seed and the general `PacketBufAllocator`, and the Wi-Fi
device is lent to it with `add_iface_borrowed`; IP configuration is set on
the returned interface. The forks expose explicit RX/TX packet pools,
packet-owner handoff to the driver, credit-return wakes, bounded polling and
construction in resource storage. A UDP or raw send blocked on the device is
woken when its interface has room, and one blocked on an empty pool when the
pool has a buffer again; the stack never busy-retries a pool. RX capacity is
not reserved separately from the shared pool.

Every build and measurement exercises the stack the product ships. A HIL run
archived under another network name keeps that name, and a build that
selects it is rejected.

The [product composition](../crates/composition/esp32s31/embassy/ieee80211/README.md)
selects the adapter and static resources; the shared radio runner is
`oer-esp32s31-ieee80211-runtime`. The [network source map](../crates/network/README.md)
and [driver map](../crates/README.md) locate these packages. Applications own
sockets and IP policy; the adapter does not acquire independent PHY/DMA owners.

## Platform source overrides

Cargo's `[patch]` is a source-selection mechanism; it does not necessarily mean
that library code has been modified. The [Cargo reference](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section)
defines workspace-root overrides and their transitive application.

| Source selection | Reason and scope |
| --- | --- |
| Owned Embassy support crates mapped to registry | Reuse released `embassy-futures`, `embassy-sync` and `embassy-time` alongside the maintained network crates |
| `esp-hal` family fork | Supply S31 radio ownership, clock/time, memory startup and interrupt handoff support required by the platform; the fork's `UPSTREAM.md` records its delta and last upstream merge |

Hardware pins and their exact responsibilities are owned by the
[esp-hal dependency boundary](../crates/adapters/esp-hal/esp32s31/README.md#dependency-boundary)
and [platform manifest](../platform/esp32s31/Cargo.toml). This platform PAC is
separate from this repository's [radio PAC](../crates/hardware/esp32s31/pac/README.md).

HIL image builds accept local checkouts of `esp-hal`, the owned Embassy fork and
the owned Xarxa fork as dependency overrides; the
[HIL target guide](../hil/targets/esp32s31/README.md) describes them.

## Workloads

Station composes DHCP and UDP echo on port 4321. AP composes a DHCP server and
UDP/TCP echo services on port 7. HIL UDP/TCP RX, TX and bidirectional workloads
share one session protocol, pacing, timeouts, payload validation and result
reporting. TCP starts accepting before HIL publishes `SessionReady`, including
when the stack enters listen state on the first poll of `accept()`; the same
accept future is retained through readiness publication and connection
completion.

A UDP send completes after the stack hands its packet to the driver or queues it
for neighbor resolution. Host delivery and terminal drain therefore matter when
reading TX results; an API completion alone is not delivery on air. HIL requests
16 queued RX datagrams; Xarxa retains packet owners through its pool rather than
per-socket byte rings.

The task-poll image observes the owned driver contract: Xarxa exposes separate
readiness and fallible owner-publication operations. Source support and
packaged firmware do not establish that every scenario or performance gate
passes; run bundles and qualification retain that authority.

## Multi-peer AP egress

An AP serving two stations is the central workload for the owned TX boundary.
Interleaved destinations require peer/TID selection before scarce SRAM
admission so that one peer's frames can form an A-MPDU. The owned adapter
classifies complete packets into Ethernet-destination queues over one shared
owner pool. The AP selects a destination before removing aggregate members,
leaving other destinations at the source. The adapter does not ask Xarxa to
construct a packet for a selected peer: pool capacity and the packets the stack
publishes limit its choices. Inside a selected destination, it round-robins
classified TCP/UDP FIFOs while preserving each flow's order. Readiness counts
the whole destination so different flows can fill the same aggregate. This
covers TID 0; classification limits, fragment handling and memory ownership are
described in the [egress contract](wifi-egress.md#owned-tx-path). Power-save
retention and failed physical-admission rollback remain radio-owned.

The outer AP destination selector defaults to round-robin; an explicitly enabled
experimental mode selects peers using modelled airtime deficits. It reserves
preparation budgets and settles terminal publication work with a
caller-supplied cost model. The [egress contract](wifi-egress.md) describes its
ownership and limits; the [HIL target guide](../hil/targets/esp32s31/README.md)
documents the RR/deficit modes. This does not establish measured airtime
fairness: equal throughput or aggregate counts cannot demonstrate equal airtime
when peers use different rates or retries. AP evidence must distinguish per-peer
aggregate fill, delivered traffic, service gaps and airtime cost.

Compare identical roles, channel/bandwidth, traffic shape, executor placement
and diagnostic image class. Throughput, packet loss, pending polls, task
residence, stack headroom and memory use answer different questions; fewer
polls alone are not a measurement of total CPU utilization. Readiness remains
the [qualification](verification-and-qualification.md) authority. Results belong
in run bundles and commit descriptions, not in this architecture reference.
