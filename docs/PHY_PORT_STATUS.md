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
- a generated recovered-register PAC and safe ownership HAL;
- a shrinking temporary raw frontier in `radio_hal.rs` for identities that
  have not yet been qualified into the PAC.

## Verified cold-init chain

The source-only HIL now executes the complete top-level sequence without
calling the vendor PHY initializer:

1. the power/clock owner enables the radio domain;
2. `PhyRegisterTransition` reproduces the prelude of `register_chipv7_phy`;
3. `PhyRfInitPrefixTransition` initializes PHY-I²C, RFPLL, RC/SAR calibration,
   crystal duty, and the hardware frequency tables;
4. `PhyBbInitTransition` runs the Wi-Fi calibration/baseband parent and its
   channel-11 initialization;
5. `PhyChipChannelTransition` selects the requested channel;
6. the source-only MAC publishes its RX descriptor ring and enables
   promiscuous receive.

This ordering was checked against local copies of
`esp32s31_rev0_rom.elf` and `libphy.a`; neither oracle is linked into the
firmware. On hardware, the open path received a real channel-6 frame on four
consecutive boots (the first fix-validation boot plus three reset runs).

The final blocker was in `phy_freq_i2c_data_write` translation:

- `phy_get_freq_mem_param(2)` has distinct I²C-memory and RF-record bases
  (`0x12` and `0x20`);
- descriptor 10 is a kind-zero three-copy command at addresses 0, 3, and 6;
- command words are little-endian `[block, register, data]`, represented as
  `data << 16 | register << 8 | block`.

The old translation reversed the last two bytes (`0x07f46703` instead of the
oracle's `0x07f40367`), so the hardware channel switch could not publish the
correct RFPLL SDM value. After correction, the channel-6 SDM register changed
from `0x06` to the oracle value `0x05`, and RX passed.

`phy_bt_tx_gain_init` is intentionally excluded from the Wi-Fi-only
`phy_bb_init` port. This is not inferred from the function name: relocation
and call-graph dataflow in `libphy.a` shows that its results stay on the
Bluetooth side:

- BT TXDC and PWDET use only `phy_param` rows `0x104`, `0x10c`, and `0x114`;
- BT power control uses only rows `0xf8`, `0xfb`, and `0xfe`;
- the final table is published through `phy_set_tx_gain_mem_new(mode = 1)`;
- the Wi-Fi path instead uses the `0xa8`-based TXDC rows and publishes gain
  memory with `mode = 0`.

The shared RFPLL/TXDC/PWDET leaf routines are implementation reuse inside the
vendor library, not shared calibrated state. Adding Bluetooth support later
must restore this child as a separate BT capability; Wi-Fi cold init and Wi-Fi
TX do not depend on it.

## Verified open passive scan

The allocation-free scanner that previously existed only in
`migration/esp32s31-hybrid-runtime` is now part of the live
`open-esp-radio-mac-esp32s31` crate. It owns a bounded 32-record BSS table,
parses beacon and probe-response information elements, de-duplicates by BSSID,
and retains the strongest complete observation. Channel timing and RX-ring
ownership remain with the caller.

The HIL application performed a cold open-PHY start and a 100 ms passive dwell
on every 2.4 GHz channel from 1 through 13. It did not call vendor PHY, MAC, or
Wi-Fi initialization. The hardware result was:

```text
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=20 raw_frames=23 dropped=0 ring_epochs=0
```

The run decoded real SSID, BSSID, RSSI, channel, RSN, HT, and HE data. Between
channels the driver now clears and confirms `RX_ENABLE` before rebuilding the
descriptor list, retunes through `PhyChipChannelTransition`, and republishes
the ring. This makes the DMA ownership edge explicit instead of relying on
temporal assumptions.

Hardware inspection also corrected the live RX extraction contract:

- the descriptor's available bytes after the public RX prefix may be two bytes
  shorter than `sig_len`;
- the copied 802.11 MPDU is `sig_len - 4`, because the FCS is not exposed;
- the recovered dump-length field observed on S31 is `sig_len + 4`.

The old migration copy was removed after qualification; Git history preserves
it. New scan work must use the live MAC module.

Planned split:

- `open-esp-radio-pac-esp32s31`: typed register descriptions;
- `open-esp-radio-hal-esp32s31`: ownership and finite register transactions;
- `open-esp-radio-phy-esp32s31`: cold calibration and channel state machines;
- higher MAC/802.11/WPA crates with no inward dependency on timers or crypto.
