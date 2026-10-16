# Radio execution

`esp32s31/bluetooth` (`oer-esp32s31-bluetooth-runtime`) contains
concrete radio execution as executor-independent `async` code; `ieee80211` (`oer-ieee80211-runtime`) holds the
chip-independent Wi-Fi execution primitives (the station service, the
lower-MAC owner glue, monitor handoffs), which no chip runtime uses yet, and `bluetooth`
(`oer-bluetooth-runtime`) the service loop that joins the in-process HCI
transport, the portable LE Controller core and any `LeRadioBackend`, which
the protocol package `oer-bluetooth-radio` declares, as the body of the
backend's port. Directory boundaries describe the execution
responsibility of each package.

## Execution and time contract

A runtime exposes futures and never spawns, names or requires an executor; any
executor that polls them is valid, and the architecture check rejects executor
dependencies below adapters and compositions. The portable primitives are
`embassy-sync` (bounded mailboxes and signals over a caller-chosen raw mutex)
and `embassy-futures` (select/join). Time is the [`oer-time`](../time/src/lib.rs)
contract: a runtime reads and waits on time only through an `oer_time::Clock`
or `Timer` value that it owns or receives from its caller, never through
`embassy-time`, which the architecture check confines to adapters,
compositions and the facade, dev dependencies included. Compositions pass
`oer_time_embassy::EmbassyClock`, which reads the one monotonic timebase the
final image links; runtime tests pass the per-instance `VirtualClock` or
`SkipClock` of `oer-time-virtual`. A runtime does not assume which executor
wakes its timers. The portable drivers below the runtime (the `service`
packages under [`services/`](../services/), such as the station join, scan and
lifecycle drivers and the WPA2 handshake runners) take the same `Timer` port
from the runtime, which polls them. The
protocol state machines they drive never wait.

| Module | Responsibility |
| --- | --- |
| `ieee80211/src/{monitor,connected_tasks,station_network,stack_boundary}` | Portable capture/injection handoffs, task shutdown, association-scoped network ownership and explicit polling boundary |
| `bluetooth/src/` | The Controller service loop, which owns the port session: it enables the port, publishes queued packets, takes one command when the core is ready and Host ACL data while the connection has room for it, submits radio requests one at a time, feeds outcomes back and retries refused requests after a short delay. It stops serving on its owner's stop request, a closed or failed transport, an unreadable clock or an exhausted radio epoch (`ServeExit::EpochExhausted`), and then disables the port and feeds every outcome to the core until `Disabled` (`Served`) |
| `esp32s31/bluetooth/src/` | The Bluetooth LE radio role and its hardware under one async lock: receive-chain publication at install, request admission against a fresh controller-time sample, the scheduler driver that reports finished events and carries list transactions through their hardware waits and stops and resumes it around a test event, a `Disable` that stops the scheduler and ends every admitted event with what the hardware recorded (`Aborted` when a started item left no status), an uninstall of a disabled port only, a bounded queue of owned outcomes whose overflow reports the loss, the source-127 modem-timer task driver and the radio port of the Controller service loop, which states LE 1M roles and hardware Link Layer acknowledgement |

Hardware transactions and finite chip state remain below these packages. A
runtime retains their affine owners across borrowed waits, returns the same
resources on rejection and preserves terminal owners when quiescence is not
proven. A composed owner alone does not establish hardware qualification.

BLE and IEEE 802.15.4 disable sinks return `ControlFlow`: a refused terminal
comes back by value and stops delivery immediately. Both runtimes publish
`TerminalRejected` and finish their hardware stop normally; uninstall returns
`Uninstalled::TerminalsQuarantined`, which keeps only the undelivered
terminals, their queue and pending permits quarantined. Neither path drains later terminals into a
consumer that has refused one.

The portable Wi-Fi station service uses `MsduQueue` for application commands
as well as network frames. Each queue has one affine sender and receiver;
waiting for readiness borrows its endpoint exclusively without removing a
value. The service handles a ready port event before taking a command, so a
simultaneous port fault leaves that command owned by its queue.

Owned timers follow the owner graph: each runtime carries the clock of its
own deadlines. The platform executor/time ABI remains in
[adapters](../adapters/embassy/README.md).

The [integration layer](../composition/esp32s31/embassy/) chooses memory budgets,
claims static resources and owns protocol lifecycle composition within the
[documented capability boundaries](../hardware/esp32s31/driver/FEATURES.md).
No product composition depends on a second runtime owner hidden in a task or
network handle.

Portable protocol policy cannot depend on this execution domain. The
architecture audit follows transitive normal/build dependencies to enforce
that boundary; portable packages cannot depend on these concrete ESP32-S31
runtimes.
