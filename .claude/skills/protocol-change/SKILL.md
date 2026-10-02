---
name: protocol-change
description: Use when changing or adding portable protocol logic or a radio port in this repository — crates/protocols/ (IEEE 802.11 MAC/STA/AP/RSN, lower/upper MAC, Bluetooth LE/HCI, IEEE 802.15.4), crates/services/, crates/radio/, crates/time/, or the Clock/Timer, LeRadioPort, Ieee802154RadioPort or Ieee80211LowerMacPort contracts. Not for chip register or driver work (driver-or-hardware-change).
---

# Change a protocol, service or radio port

Read first (about 3k tokens):
[sans-IO, executors and time](../../../docs/architecture.md#sans-io-protocols-executors-and-time),
[layer dependencies](../../../docs/architecture.md#layer-dependencies) and,
for a port, [radio ports](../../../docs/architecture.md#radio-ports).
Find the owning package in the
[source map](../../../crates/README.md#source-map) and read its README only.

## Checklist

1. **Place the logic.** Decisions software makes (retry, rate, contention,
   reordering, sequence/packet numbers, scanning, power-save policy) live
   above the port in portable code; work the hardware does autonomously lives
   below it. A state machine belongs in a `protocol` package, the loop that
   waits on ports in a `service` package.
2. **Stay sans-IO.** A `protocol` package has no `async` body and no `.await`:
   frames, completions and `oer_time::Instant` enter as values; actions and
   the next deadline leave as values. No `embassy-time`, executor or HAL.
3. **Time.** Lower layers read and wait on time through the `oer-time`
   `Clock` and `Timer` ports; tests drive the virtual clocks of
   `oer-time-virtual`.
4. **Change every caller.** No compatibility aliases or shims: update every
   backend, service, runtime and composition that uses the changed type in
   the same change (grep the type name across `crates/`, `hil/`, `examples/`).
5. **Names.** Follow [protocol naming](../../../docs/protocol-naming.md):
   `ieee80211` in package and module names, `Wifi*` only in the facade.
6. **Test.** A focused test beside the module for each behavior: a host model
   or virtual time, never a second implementation of the protocol.
7. **Docs.** Update the package README or rustdoc that describes the changed
   contract; a capability whose support changed keeps its
   `// CAPABILITY:` anchor in step (`qualification-entry` skill).

## Commands (all `run_in_background: true`)

```console
cargo test -p <package>
cargo xtask check changed
cargo xtask check architecture   # when dependencies, features or layers changed
cargo xtask doc                  # when public API docs changed
```

Then commit with a scoped subject and push with the `push-and-ci` skill.
