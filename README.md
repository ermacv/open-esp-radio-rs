# open-esp-radio-rs

Source-only Rust radio stack for Espressif chips, initially targeting
ESP32-S31.

The repository is designed to be consumed directly by `no_std` Rust
applications. It does not link `esp-wifi-sys`, a vendor Wi-Fi archive, or a
Wi-Fi/radio ROM ABI. The silicon boot ROM is outside this definition; the
strict boundary concerns radio ownership and runtime calls.

Current workspace layers:

- `open-esp-radio-ieee80211`: chip-independent, allocation-free 802.11
  management framing and scan observations;
- `open-esp-radio-pac-esp32s31`: register access and peripheral ownership;
- `open-esp-radio-hal-esp32s31`: finite radio transactions and async boundary
  traits;
- `open-esp-radio-mac-esp32s31`: allocation-free descriptor, RX/TX ownership,
  interrupt primitives, with a compatibility re-export of the generic scan
  API;
- `open-esp-radio-phy-esp32s31`: Rust-owned cold PHY/calibration state
  machines;
- `open-esp-radio`: application-facing facade.

The PHY port is still experimental. Its state machines and source-only link
gate are usable, while the temporary register leaf module is progressively
being moved down into HAL/PAC.

Cold source-only PHY initialization, open promiscuous RX, active/passive scan,
open authentication, WPA2 association, and the four-way handshake through an
acknowledged hardware-CCMP Message 4 have passed on ESP32-S31 without vendor
radio initialization. The protected TX run used the recovered QoS/CCMP
layout, PN 3, owned STA pairwise slot 4, and direct raw-q0 DMA. Protected RX,
GTK installation, controlled-port data, and ordinary network traffic remain
to be qualified in the live crates.

The ESP32-S31 HAL binds the integration layer's singleton peripheral token to
`Radio<P, Owned>`. Its finite `power_up` transition reproduces the
source-owned modem reset, PMU publication, clock-source, PHY frontend and
PHY-I²C prerequisites and verifies nine readable checkpoints. The eleven
register identities used by this transition are typed PAC values; the HAL
contains no raw addresses or volatile pointer access for this path. Only the
resulting `Radio<P, Powered>` exposes the register capability used by finite
PHY target bindings. Wi-Fi MAC clocks remain outside this transition and
belong to the later MAC start state.

The live MAC path also consumes typed PAC registers. Its initialization,
interrupt, RX, and TX transactions require a mutable borrow of the register
capability; only the PAC performs peripheral volatile access. The application
therefore hands the same `RadioRegisters` owner from completed PHY
initialization into MAC/RX, instead of constructing an independent raw-MMIO
adapter. DMA descriptors retain their own volatile cells because they are
owned memory shared with hardware rather than peripheral registers.

The former upper MAC/STA/AP/security migration archive has been retired.
Source-owned logic now lives in the buildable IEEE 802.11, MAC, WPA2, PAC and
integration crates above; vendor ABI glue and superseded duplicates were
deleted. [`MIGRATION_COMPLETION.md`](docs/MIGRATION_COMPLETION.md) records the
destination and deletion decision for each workset. Git history preserves the
exact pre-cleanup archive.

Hardware integration belongs in a separate application workspace. The
`esp32s31_rust` HIL project may depend on this repository for the open driver
and on `esp-wifi-sys` for a closed-driver comparison profile; neither driver
depends on the other.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in this repository.
