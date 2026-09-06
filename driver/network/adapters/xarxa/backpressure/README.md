# Xarxa UDP backpressure integration

This composition uses the driver contract and queue implementation from
[`../upstream`](../upstream/README.md), the original Embassy network wrapper,
and a narrowly patched Xarxa stack. It is a Cargo source selection, not a
second packet queue or a copy of the radio adapter.

[cargo-config.toml](cargo-config.toml) pins the published Xarxa revision.
That revision depends on the original upstream `xarxa-driver`, preserving
`PacketBuf` identity. Embassy, PHY, DMA, radio queues and pool capacity are
unchanged. The build checks the resolved lock catalog and rejects any other
package pin change. Effective locks accompany the resulting firmware;
tracked lock catalogs retain the upstream control.

## Selection

From the repository root:

```console
cargo hil image build performance --network upstream
cargo hil image build performance --network udp-backpressure
cargo hil run udp-tx-ht40-task-poll-diagnostic --network udp-backpressure
cargo xtask build firmware station --network udp-backpressure
cargo xtask build firmware access-point --network udp-backpressure
cargo xtask check network-backpressure
```

HIL defaults to `upstream`. In IP examples, `--network` selects the
`upstream-network` API contract and disables the default maintained-fork
contract; use `--network upstream` for the original control. Without the flag,
examples retain their declared Cargo defaults. Monitor and Bluetooth have no
IP-stack selection. A replay uses its archived firmware and cannot also accept
`--network`. Local dependency overrides cannot be combined with the patch.
Build and scenario artifacts remain under their ordinary owner directories;
image reports name the network selection, and run manifests retain the command
and effective dependency locks.

For an external application using the original Xarxa driver contract, apply the
`[patch]` table from `cargo-config.toml` at its workspace root. Cargo features
alone cannot select two different implementations of the same transitive crate
inside one graph.

## Wakeup boundary

A UDP send rejected because its device is full records the destination on that
socket. Stack polling resolves its current route and wakes the sender when the
selected interface can accept a packet, or when the route disappears so the
sender can observe the error. An unrelated ready interface does not release
this wait. Binding, closing or starting another send clears the old wait.
The driver's ordinary capacity notification schedules the stack when TX space
returns. No timer, artificial delay, queue enlargement or packet copy is added.

Global packet-pool exhaustion still uses the upstream retry behavior: the
original `PacketBuf` API has no release event, and applications may release
buffers outside any driver operation. Suppressing that retry without adding
an appropriate notification could leave a sender asleep forever. Raw sockets
also retain upstream behavior. This integration therefore addresses the UDP
**device-capacity** feedback loop; it does not claim to eliminate all polling
under resource exhaustion or to establish a hardware throughput limit.

The host regression holds the production TX queue full, requires the sender
and stack to become quiescent, then returns capacity and checks send completion
and packet disposal. The ordinary upstream test exercises the same recovery
without requiring quiescence. Firmware qualification requires HIL evidence.
