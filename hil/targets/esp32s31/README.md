# ESP32-S31 HIL target

- [System and PHY watchdogs](watchdogs.md)
- [Stack measurements](memory.md)
- [Host setup and fixture installation](../../host/README.md)
- [Wire and evidence contracts](../../protocol/README.md)

This workspace selects the shared board boot and memory profile and owns the
Embassy executor, the USB Serial/JTAG transport and the HIL workloads of the
Bluetooth LE and radio-free system images. Effective dependency locks are
archived beside each image.

The Bluetooth images reach the radio only through HCI.
`agent/src/bluetooth.rs` starts the shared radio with
`oer_esp32s31_radio_system::start`, which spawns the coexistence schedule,
starts the production Bluetooth client on it, runs the radio runner, the HCI
Controller service and its own periodic PHY tracking task, which reports a
tracking failure as `reason=phy-tracking`, and serves the typed console. The `bluetooth-dtm` image (`bluetooth-hil`) serves the
`bluetooth-dtm-*` scenarios: `bluetooth/dtm.rs` resets the Controller and turns
each DTM operation into LE Transmitter Test v1 (channel 0, 37-byte PRBS9), LE
Receiver Test v1 (channel 0), LE Test End or HCI Reset, reporting the Test End
packet count. The `bluetooth-gatt` image serves `bluetooth-trouble-gatt`:
`bluetooth/gatt.rs` runs the Trouble Host and the plaintext GATT application
over the Host end of the transport. Both images use the development Controller
identity Core 5.4, company `0xffff`, subversion 1, and a 500-ppm sleep-clock
bound. The `bluetooth-secure-gatt` image serves the `bluetooth-trouble-secure-gatt*`
scenarios: `bluetooth/secure.rs` keeps one RAM bond store across Host epochs.
Each epoch runs the Trouble Host and the secure application beside the radio
runner and the HCI service. A requested restart resets the Host through HCI,
retires the drained Host end of the transport, stops the Controller on the
radio system, checks that the old Host end reports the transport closed and
starts the next Controller epoch with a fresh Controller core. An application
failure closes the Controller without restarting; an unconfirmed Reset keeps
every owner.

## Application boundaries

The agent binary is the HIL application of the shared stage-two boot
contract. Each image class selects one base feature: `bluetooth-hil`,
`bluetooth-gatt`, `bluetooth-secure-gatt`, `system-watchdog`,
`system-panic-reset` or `boot-smoke`. Board initialization, relocation and
interrupt-stack mechanics belong to the shared platform; application images
use the same mechanism through `cargo fw build`. A hardware scenario verdict
remains a separate HIL responsibility.

## Protocol families per image

Each image enables only the HIL protocol families it serves
([radio families](../../protocol/README.md#radio-families)), so its build, and
the evidence bound to its sources, reads only their protocol modules: the
Bluetooth images the `bluetooth` family and the radio-free `system-watchdog`,
`diagnostic-usb-jtag-off` and `system-panic-reset` images only the shared
core, `system` module included.

## Event trace

Every Bluetooth image keeps one `oer-trace` trace in RTC fast
memory beside the post-mortem record (`agent/src/trace.rs`, the agent's
`trace-journal` feature, `cfg(trace_journal)` in `build.rs`): 512 entries, and two 1024-word
snapshot slots only with the `trace-snapshots` feature. The agent's panic
hook, which the platform's panic entry calls through its
`panic-hook` feature, freeze it before the reset; every image but
`system-panic-reset` has that hook (the agent's `panic-hook` feature, which
each image's base feature enables), and that image runs the product panic
entry alone, so the `system-panic-reset` scenario exercises the product's
record-and-reset path. The hook and the exception entry format nothing: they record the post-mortem fault and the hart's trap
registers, the vector it took and the sources pending and routed to it
(`agent/src/fatal.rs`) in RTC fast memory and reset, and the next boot prints
`OPEN_RADIO_HIL runtime=PANIC boot=previous` or `runtime=EXCEPTION
boot=previous` with that state before anything else. A panic message with
arguments is not kept, only a static one in task context. An interrupt-context
panic omits location and message because compiler metadata may be in unavailable
PSRAM; the hook receives the platform's safe `PanicDetails`. The hook's code,
trace pointer/mask and diagnostic stage stay in SRAM, its records stay in RTC
memory, and its fault CRC uses no cached lookup table. The next boot also holds the frozen
trace for the host, which pages it out through the
[trace commands](../../protocol/diagnostics.md#event-trace). The `bluetooth-trace` feature records the Bluetooth LE Controller,
radio role and runtime events of
[`oer-bluetooth-trace`](../../../crates/protocols/bluetooth/trace/README.md)
through the composition's `trace` feature; the Bluetooth console serves the
trace commands in every Bluetooth image, whose trace holds only the agent's
own events without it. `diagnostic-bluetooth-gatt-trace` builds the GATT
image with it, and product images never do.

## Local dependency overrides

A git-pinned dependency of the firmware can be replaced by a local checkout
for one build or run. The variable names the checkout's root:

| Variable | Checkout | Packages patched |
| --- | --- | --- |
| `ESP_HAL_ROOT` | esp-hal | `esp-hal`, `esp-sync`, `esp-bootloader-esp-idf` |

The runtime build patches them in and resolves a private copy of its lock
file; the bootstrap takes only the esp-hal override, the one dependency it has.
A run's source snapshot archives each override checkout as its own source
role, so an untracked file there is named as `--source-include esp-hal:<path>`.
An override build binds evidence only when the checkout's commit is the pinned
revision in `hil/targets/esp32s31/Cargo.toml`; otherwise it is an experiment.

To find the dependency commit that changed a scenario's outcome, bisect in the
dependency's clone and build this checkout against each step. A build failure
skips the step (exit 125) instead of marking it bad:

```console
git clone https://github.com/ermacv/esp-hal.git ~/dev/esp-hal
cd ~/dev/esp-hal
git bisect start <bad-rev> <good-rev>
git bisect run sh -c 'cd ~/dev/<checkout> &&
  ESP_HAL_ROOT=~/dev/esp-hal cargo hil image build bluetooth-gatt || exit 125;
  ESP_HAL_ROOT=~/dev/esp-hal cargo hil run <scenario>'
git bisect reset
```

Each step takes its own lease.
