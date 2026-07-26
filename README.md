# open-esp-radio-rs

Source-only Rust radio stack for Espressif chips, initially targeting
ESP32-S31.

The repository is designed to be consumed directly by `no_std` Rust
applications. It does not link `esp-wifi-sys`, a vendor Wi-Fi archive, or a
Wi-Fi/radio ROM ABI. The silicon boot ROM is outside this definition; the
strict boundary concerns radio ownership and runtime calls.

Current workspace layers:

- `open-esp-radio-pac-esp32s31`: register access and peripheral ownership;
- `open-esp-radio-hal-esp32s31`: finite radio transactions and async boundary
  traits;
- `open-esp-radio-mac-esp32s31`: allocation-free descriptor, RX/TX ownership
  and interrupt primitives;
- `open-esp-radio-phy-esp32s31`: Rust-owned cold PHY/calibration state
  machines;
- `open-esp-radio`: application-facing facade.

The PHY port is still experimental. Its state machines and source-only link
gate are usable, while the temporary register leaf module is progressively
being moved down into HAL/PAC.

The ESP32-S31 HAL now binds the integration layer's singleton peripheral token
to `Radio<P, Owned>`. Only `Radio<P, Powered>` exposes the register capability
used by finite PHY target bindings. The current `Owned -> Powered` transition
is intentionally `unsafe` until the source-owned clock, reset, power-domain
and 40 MHz prerequisites are implemented and tested by the hardware
integration.

The complete former hybrid Rust workset is retained under
`migration/esp32s31-hybrid-runtime` as non-buildable migration source. It is
not an application dependency and is excluded from the Cargo workspace.
Modules leave that directory only after their blob/ROM boundary has been
removed and their source-only replacement is tested.

Hardware integration belongs in a separate application workspace. The
`esp32s31_rust` HIL project may depend on this repository for the open driver
and on `esp-wifi-sys` for a closed-driver comparison profile; neither driver
depends on the other.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in this repository.
