# Network dependency selection

This directory owns Cargo source overrides for network implementations.
It contains no packet adapter or radio implementation. The
[implementation guide](../../../docs/network-implementations.md) owns the
user-facing choices, patch rationale and availability; the reusable original
Xarxa driver lives in [adapters/xarxa/upstream](../adapters/xarxa/upstream/README.md).

## Patched Xarxa

[xarxa-patched.toml](xarxa-patched.toml) replaces only `xarxa` from the original
Git source with a published, immutable revision. That revision retains the
original `xarxa-driver` source, so `PacketBuf` and the driver trait keep their
identity. Original Embassy and the broader owned-network forks are not replaced.

The shared builder in [tools/firmware](../../../tools/firmware/src/network.rs)
applies this config for `--network patched-xarxa`. It owns the workspace lock
catalog during resolution, rejects unexpected package-pin changes, archives
the effective lock and restores the tracked catalog. An overlapping build in
the same workspace fails before changing that catalog. Separate workspaces and
Cargo compilation retain their ordinary parallelism. HIL local dependency overrides
are supported only with `upstream-xarxa`.

For an external application using the original Xarxa contract, copy the
`[patch]` table to the application's workspace-root manifest, or pass the config
with `cargo --config PATH`. A dependency's own `[patch]` table does not propagate
to its consumer; see the [Cargo reference](https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html#the-patch-section). A Cargo feature selects the driver API; it cannot by itself
replace this transitive stack source. This config applies to the reviewed
original Git graph, not the released Embassy/smoltcp or owned-network graph.

Check the production adapter's device-capacity wait and recovery contract with:

```console
cargo xtask check network-backpressure
```

That command resolves the published pin, requires sender/stack quiescence while
the TX queue is full, then verifies recovery and packet disposal. The ordinary
upstream regression checks recovery without requiring quiescence. These host
checks do not establish hardware qualification.
