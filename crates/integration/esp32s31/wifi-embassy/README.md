# ESP32-S31 Wi-Fi / Embassy integration

This crate owns the reusable async composition between ESP32-S31 Wi-Fi
hardware owners and a bounded Embassy network adapter. It does not own board
startup, task placement, credentials, sockets, traffic generation or HIL
reporting.

The application-facing station entry points are in `station`:

- `Esp32s31Station` owns the finite scan/join/connected/reconnect lifecycle;
- `Esp32s31StationController` publishes ordered reconnect/disconnect/stop
  requests;
- `run_esp32s31_connected_station_epoch` stops the radio only at a safe
  transaction boundary;
- `stop_esp32s31_connected_task_group` returns all spawned-task ownership or a
  distinct reset-required outcome under one deadline.

Run the host-side composition example with:

```text
cargo run -p open-esp-radio-esp32s31-wifi-embassy --example station_service
```

The example deliberately supplies a small deterministic lifecycle backend. A
real RISC-V board application binds the same facade to
`Esp32s31StaAttemptTargetPort`, its static DMA/network storage, its executor
spawners and the ESP-HAL interrupt adapter. Those board resources cannot be
made portable by hiding them in the driver or in global HIL state.
