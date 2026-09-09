# Coexistence timer control

This crate owns the recovered timer programming sequence, clock conversion,
PTI lookup and software schedule state. It does not implement RF grant
notification or a joint Wi-Fi/Bluetooth/IEEE 802.15.4 runtime. See
[source capabilities](FEATURES.md) for those boundaries.

`CoexCore::request_wifi` and `request_bluetooth` return a programmed timer
identity. Latency and duration are source parameters converted into timer
targets; they are not guaranteed RF start/end times. `CoexStatus::active_timers`
reports requests retained by software, not current RF ownership.

## Failed hardware transactions

The hardware trait permits an error after a register write. Before programming
a timer, the core records an uncertainty bit. It clears this bit only after
configuration, both clock conversions/target writes and enable have succeeded.
Errors from any of those steps retain the affected timer for cleanup, even if
it never entered `active_timers`.

While `uncertain_timers` is nonzero, further requests return `RecoveryRequired`
without issuing hardware operations. Calling `enable` cannot clear this
condition. `release` or `disable` executes the backend withdrawal transaction;
failure retains uncertainty for a subsequent explicit recovery attempt.
Successfully retired timers are removed from software bookkeeping.

`disable` visits both successfully programmed and uncertain timers. It changes
the core to disabled only after all required withdrawal transactions succeed.
The Embassy adapter therefore cannot return `Stopped` after a failed shutdown;
its owner remains available to report status and accept a recovery command.
No automatic retry loop or synthetic RF grant is introduced.

This is accounting for hardware transactions, not proof that a frame,
descriptor walker or another protocol has stopped. The concrete HAL currently
exists only for validation and performs the reviewed register sequence; its
successful return must not be promoted to whole-radio quiescence. Direct
timer access outside the core requires its own ownership and cleanup contract.

## Ownership boundary

The core holds values and cleanup obligations, while the supplied HAL owns
register access. Both must stay associated for their full control epoch.
The standalone Wi-Fi PHY maintenance capability is a separate HAL boundary;
a timer index or mailbox reply cannot be converted into that capability.

IEEE 802.15.4 is not an inferred third numeric `CoexClient`. Its MAC PTI fields
and reviewed vendor wrappers have a distinct interface, currently uncomposed.
