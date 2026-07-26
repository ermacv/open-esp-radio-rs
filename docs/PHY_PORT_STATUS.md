# ESP32-S31 PHY port status

The source-only PHY implementation now lives in the public
`open-esp-radio-rs` workspace.

The crate has no dependency on `esp-wifi-sys`, vendor static archives, or
radio/Wi-Fi ROM symbols. The audited PHY transition files are physically
owned by this repository.

Current layers:

- Rust-owned cold-PHY and baseband state machines;
- finite MMIO/I²C/PBus bindings;
- explicit async timer/readiness actions;
- the temporary register-level HAL in `radio_hal.rs`.

Planned split:

- `open-esp-radio-pac-esp32s31`: typed register descriptions;
- `open-esp-radio-hal-esp32s31`: ownership and finite register transactions;
- `open-esp-radio-phy-esp32s31`: cold calibration and channel state machines;
- higher MAC/802.11/WPA crates with no inward dependency on timers or crypto.
