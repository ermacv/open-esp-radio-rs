# ESP32-S31 Bluetooth Controller

This target composes the production ESP32-S31 Bluetooth Controller exactly as
an application must own it:

- the ESP-HAL platform singletons and the restricted radio PAC are each taken
  once;
- cold start claims the static BLE-PHY and DTM graphs;
- the sole hardware runner owns command, timer, and interrupt progress;
- a standard `bt-hci::Controller` read loop is polled concurrently with typed
  Host commands.

The default board smoke sequence uses only the upstream `bt-hci` 0.10.1 typed
command API. It performs:

1. HCI Reset;
2. LE Receiver Test v2 on test channel 0, LE 1M, standard modulation index;
3. a bounded one-second receive dwell followed by LE Test End;
4. LE Transmitter Test v2 on the same channel and PHY with a 37-byte PRBS9
   payload;
5. a bounded one-second transmit dwell followed by LE Test End.

The receive Test End packet count is printed as an observation; it may be zero
when no peer transmitter is present. The transmit Test End packet count is
required to be zero, as defined by HCI, and any other value fails the smoke
sequence. Command execution, the Host read pump and the hardware runner remain
concurrently polled, so command responses can make progress. The example uses
`Controller::alloc_buf` for the read side and does not encode raw HCI opcodes or
implement a local HCI codec.

Every command prints a `submitted` marker before it crosses HCI and a
`running` or `complete` marker after the typed response. A two-second command
timeout turns a lost Controller response into a bounded failure, so a board run
can distinguish command-intake, event-start and Test End liveness failures.
Before commands, `application entered`, `executor starting`, and
`Bluetooth Controller cold start submitted` markers distinguish application,
executor and radio startup; `Bluetooth Controller ready` closes cold start.

This is a board smoke sequence, not recorded HIL evidence. Meaningful RF
validation still requires a suitable peer or tester, controlled RF conditions
and the repository's HIL evidence process.

Build and flash the complete application from the repository root:

```console
cargo xtask build firmware bluetooth-controller
cargo xtask build firmware bluetooth-controller --flash --monitor --port /dev/ttyACM0
cargo xtask build firmware bluetooth-controller --features advertising-smoke
```

The [shared platform](../../platform/esp32s31/README.md) initializes PSRAM,
relocates the application and supplies SRAM interrupt stacks. Image and stack
audits run before packaging or flashing. A plain `cargo build` inside this
workspace produces the stage-two ELF, which requires the shared bootstrap.

This feature replaces the DTM commands. After initial Reset, it exercises
nonconnectable `ADV_NONCONN_IND`, then connectable `ADV_IND`, each with a static
random address, 100 ms intervals, all three advertising channels and the local
name `open-radio`. Each case configures address/parameters/data, enables for one
second, disables, re-enables for one second, resets while enabled, reconfigures,
enables for one second and finally disables. Run without a connecting peer:
accepted connections exercise a separate, incomplete peripheral lifecycle.

Commands print `advertising <command> submitted` and `complete` markers, with
the command name in failures and two-second timeouts. Case and dwell markers
identify the last reached step. These observations establish HCI lifecycle
progress only: a successful Enable response or elapsed dwell does not prove
scheduler RUN, repeated events or RF transmission. Use scheduler evidence and a
BLE observer for those claims. Add `--flash --monitor --port /dev/ttyACM0` to the `xtask` advertising command
to flash this variant and open the monitor.
