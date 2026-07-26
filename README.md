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
- `open-esp-radio-mac-esp32s31`: allocation-free descriptor, RX/TX ownership,
  interrupt primitives, and a bounded passive-scan table/parser;
- `open-esp-radio-phy-esp32s31`: Rust-owned cold PHY/calibration state
  machines;
- `open-esp-radio`: application-facing facade.

The PHY port is still experimental. Its state machines and source-only link
gate are usable, while the temporary register leaf module is progressively
being moved down into HAL/PAC.

Cold source-only PHY initialization, open promiscuous RX, and a passive scan
across channels 1 through 13 have passed on ESP32-S31 hardware without vendor
radio initialization. Management TX, association, and encrypted data traffic
remain unqualified in the live crates.

The ESP32-S31 HAL binds the integration layer's singleton peripheral token to
`Radio<P, Owned>`. Its finite `power_up` transition reproduces the
source-owned modem reset, PMU publication, clock-source, PHY frontend and
PHY-I²C prerequisites and verifies nine readable checkpoints. Only the
resulting `Radio<P, Powered>` exposes the register capability used by finite
PHY target bindings. Wi-Fi MAC clocks remain outside this transition and
belong to the later MAC start state.

The not-yet-ported upper MAC/STA/AP/security workset is retained under
`migration/esp32s31-hybrid-runtime` as a non-buildable source archive. It is
not a Cargo crate or an application dependency. Qualified PHY and passive-scan
copies have been removed from the archive; their only maintained
implementations are the live crates above. The archive's
[`PORTING_MAP.md`](migration/esp32s31-hybrid-runtime/PORTING_MAP.md) records
what remains and the criteria for moving each piece.

Hardware integration belongs in a separate application workspace. The
`esp32s31_rust` HIL project may depend on this repository for the open driver
and on `esp-wifi-sys` for a closed-driver comparison profile; neither driver
depends on the other.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in this repository.
