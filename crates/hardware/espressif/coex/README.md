# Espressif coexistence policy

`oer-espressif-coex` is the coexistence policy the Espressif chips share,
recovered from esp-coex-lib `c758e7b5`: the 49-event priority table
(`CoexPtiTable`, equal in the pinned ESP32-S31 and ESP32-C5 archives) with
the IEEE 802.15.4 levels it resolves, the timer programming sequence, clock
conversion, event-to-timer mapping and software schedule state. It owns no
register: each chip's radio arbiter keeps its own copy of the table as
shared state, and a chip crate such as
[`oer-esp32s31-coex`](../../esp32s31/driver/coex/README.md) binds the
`CoexTimerHardware` and `CoexClockHardware` ports to its HAL.
`CoexTimerHardware::pti` reads the arbiter's table when a request is
programmed, and a priority change goes through the arbiter's lease. The
ESP32-S31 arbiter lends only timers 0 through 4: timer 5 carries its PHY
grant-protect request, so event 48 has no policy timer. The
[radio runtime](../../../runtime/esp32s31/radio/README.md) composes the
core and the schedule under the arbiter lease. This crate does not implement
RF grant notification. See the ESP32-S31
[source capabilities](../../esp32s31/driver/coex/FEATURES.md) for those
boundaries.

The recovered values cite the ESP32-S31 archive where they were reviewed;
the PTI table and the IEEE 802.15.4 level lookup were also reviewed against
the ESP32-C5 archive.

## Portable clients and priorities

The portable vocabulary is [`oer-radio-coex`](../../../radio/coex/README.md).
`CoexClient` converts from its `RadioClient` into the vendor request kind
(IEEE 802.15.4 has none), `CoexStatusType` into the status word a client
publishes, and `Ieee802154CoexLevel` from and into its `CoexPriority`
(`Idle`, `Low` = `Normal`, `Middle` = `Elevated`, `High` = `Critical`).

## Time-slice schedule

`CoexSchedule` is the recovered `coex_schm_env` of esp-coex-lib `c758e7b5`.
Each radio publishes status bits (`set_status_bits`, `clear_status_bits`);
the status words select one of the 107 recovered schemes of
`coexist_scheme.o`, exactly as `coex_schm_status_change` does. The
[vendor `coex` scenario](../../../../verification/esp32s31/README.md#coexistence-schedule-comparison)
compares the compiled schedule with the pinned `libcoexist.a` entries. A scheme
divides a period into phases; a phase lasts period × interval × share
microseconds and notifies Wi-Fi, Bluetooth or both. The schedule programs no
priority itself: a notified radio requests its own events through
`CoexCore`, and the hardware arbitrates by priority.

The phases loop only while at least two radio groups publish status. With
Wi-Fi scanning, connecting or keeping only a connectionless window, they loop
on the schedule's own timer; a connected Wi-Fi stops them at the last phase
and restarts them itself (`restart`) at its beacons. Each `CoexPhaseStep`
tells the runtime owner how long to arm the phase timer and whom to notify.
`CoexScheduleExecutor` turns each step into a phase-timer command with a
generation; an expiry of an older generation lost a race with a later phase
change and steps nothing. The radio runtime runs the timer and signals each
notified radio. Wi-Fi and Bluetooth LE publish their status through the
radio guard (`RadioGuard::set_coex_status_bits`); Wi-Fi reacts to its phases,
while Bluetooth LE does not yet wait for its own.

On the ESP32-S31, `oer_esp32s31_coex::CoexArbiterPorts` lends one arbiter
lease's timer bank, event priorities and clock to `CoexCore` as its timer and
clock ports.

## Event requests

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
descriptor walker or another protocol has stopped. The arbiter ports perform
the reviewed register sequence; their successful return must not be promoted
to whole-radio quiescence. Direct
timer access outside the core requires its own ownership and cleanup contract.

## Ownership boundary

The core holds values and cleanup obligations, while the supplied HAL owns
register access. Both must stay associated for their full control epoch.
The standalone Wi-Fi PHY maintenance capability is a separate HAL boundary;
a timer index or mailbox reply cannot be converted into that capability.

Three similarly timed operations remain separate contracts. Wi-Fi
peer-facing power save exchanges PM state with an AP and governs peer buffering.
PHY tracking runs under the radio arbiter lease. Physical RF/baseband/clock
close follows the last client's release and the shared PHY lifecycle. A coex
timer request grants none of those authorities, and an immediate RF close/wake
cycle is not connected modem sleep.

IEEE 802.15.4 is not an inferred third numeric `CoexClient`. Its MAC PTI fields
and reviewed vendor wrappers have a distinct interface. `CoexPtiTable`
resolves its four coexistence levels (high, middle, low, idle) from table
events 41 through 44, and the ESP32-S31
[IEEE 802.15.4 system](../../../composition/esp32s31/embassy/ieee802154/README.md)
publishes them per operation scene.
