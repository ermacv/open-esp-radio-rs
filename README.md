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
- `open-esp-radio-esp-hal-esp32s31`: optional singleton-token adapter for the
  `esp32s31-async-platform` branch of the `esp-hal` fork;
- `open-esp-radio-mac-esp32s31`: allocation-free descriptor, RX/TX ownership,
  interrupt primitives, with a compatibility re-export of the generic scan
  API;
- `open-esp-radio-phy-esp32s31`: Rust-owned cold PHY/calibration state
  machines;
- `open-esp-radio`: application-facing facade.

The PHY port is still experimental. Its state machines and source-only link
gate are usable, while the temporary register leaf module is progressively
being moved down into HAL/PAC.
The maintained vendor function inventory and behaviour comparison are in
[`docs/phy/README.md`](docs/phy/README.md).

Cold source-only PHY initialization, open promiscuous RX, active/passive scan,
open authentication, WPA2 association, the four-way handshake, protected
pairwise/group traffic, DHCP, HT40 and HE20 A-MPDU/BlockAck have passed on
ESP32-S31 without vendor radio initialization. The current open HE SU TX path
has exercised MCS0 through MCS9 with the datasheet's 0.8-, 1.6- and 3.2-us
guard intervals. [`ESP32S31_WIFI_FEATURE_STATUS.md`](docs/ESP32S31_WIFI_FEATURE_STATUS.md)
tracks these results and the remaining datasheet capabilities without
equating discovered oracle symbols with implemented features.

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

The low-level PAC is generated from `svd/esp32s31-radio.svd` by a portable
Rust `xtask` using pinned `svd2rust 0.37.1`; run `cargo pac-gen` to regenerate
it or `cargo pac-gen --check` to verify it. Python and shell code generation
are not used. The existing `Register32`/`Field32` surface is a temporary
compatibility facade while HAL and MAC move to the native generated register
API. The current unsafe/MMIO inventory and non-radio clock/power dependencies
are recorded in [`PAC_AND_UNSAFE_AUDIT.md`](docs/PAC_AND_UNSAFE_AUDIT.md).
The blob/ROM debug-symbol audit and its MMIO-versus-descriptor classification
are recorded in
[`esp32s31-debug-oracles.md`](docs/esp32s31-debug-oracles.md).

The former upper MAC/STA/AP/security migration archive has been retired.
Source-owned primitives live in the buildable IEEE 802.11, MAC, WPA2, PAC and
integration crates above; vendor ABI glue and superseded duplicates were
deleted. The end-to-end HIL application still contains reusable runtime
orchestration that is being extracted into these crates.
[`MIGRATION_COMPLETION.md`](docs/MIGRATION_COMPLETION.md) records the original
archive decision, and
[`ESP32S31_RUST_INTEGRATION_AUDIT.md`](docs/ESP32S31_RUST_INTEGRATION_AUDIT.md)
tracks the remaining application-to-driver transfer. Git history preserves the
exact pre-cleanup archive.

Board policy, task spawning, linker placement and flashing belong in a
separate application workspace. Reusable ESP-HAL singleton/trait wiring lives
in the optional adapter crate above. The `esp32s31_rust` HIL project may
depend on this repository for the open driver and on `esp-wifi-sys` for a
closed-driver comparison profile; neither driver depends on the other.

No vendor ELF, static library, disassembly dump, generated proprietary header,
or extracted binary table belongs in this repository.
