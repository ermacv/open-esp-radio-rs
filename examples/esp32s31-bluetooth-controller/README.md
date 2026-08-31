# ESP32-S31 Bluetooth Controller

This target composes the production ESP32-S31 Bluetooth Controller exactly as
an application must own it:

- the ESP-HAL platform singletons and the restricted radio PAC are each taken
  once;
- cold start claims the static BLE-PHY and DTM graphs;
- the sole hardware runner owns command, timer, and interrupt progress;
- a standard `bt-hci::Controller` read loop is polled concurrently with typed
  Host commands.

The current smoke sequence issues typed HCI Reset and LE Test End while DTM is
idle. A successful Test End returns zero received packets and proves the real
Host-to-Controller-to-Host command path without a local HCI implementation.

Typed LE Receiver Test and LE Transmitter Test are intentionally not emitted
yet. The pinned `bt-hci` release does not define them, while its newer release
currently assigns the legacy Transmitter Test the wrong command code. The
example will add on-air start commands only after the upstream typed command is
correct; it does not bypass `bt-hci` with raw opcodes.

Build the target with:

```console
cargo build --release
```

With an ESP32-S31 connected, `cargo run --release` flashes it and opens the
serial monitor.
