# Archived ESP32-S31 open-radio frontier log

> Historical paths and commands below refer to the former `esp32s31_rust`
> integration. The active HIL system is `hil/esp32s31` in this repository.

# ESP32-S31 open-radio frontier

`open-radio-frontier` is the first isolated firmware consumer of
`open-esp-radio-rs`. It establishes the dependency and ownership boundary
without pretending that the complete target PHY executor already exists.

The workload:

- consumes the unique `esp-hal` `WIFI` peripheral directly into
  `Radio<WIFI, Owned>`;
- verifies the current PAC radio address ranges;
- constructs the cold-PHY registration transition;
- lowers its first external operation to a typed source-owned MMIO binding;
- performs no radio-register write.

Build it with:

```console
cargo xtask app build open-radio-frontier
```

The `xtask` graph audit requires `open-esp-radio` and its HAL, PAC, PHY and MAC
packages. It rejects `esp-radio`, `esp-phy`, `esp-wifi-sys`,
`esp-radio-rtos-driver` and `esp-alloc`.

The generated image is safe to execute as an ownership/API probe:

```console
cargo xtask app run open-radio-frontier --port /dev/ttyACM0
```

A passing serial trace ends with:

```text
OPEN_RADIO_FRONTIER result=PASS next=target-executor
```

## Why this is not yet the PHY HIL

`PhyRegisterTransition` already represents the complete bounded cold-PHY
state machine, but the application still needs an exhaustive target executor
for every nested `PhyRegisterExternalBinding`. That executor must:

1. execute finite MMIO bindings while holding the unique `Radio` owner;
2. use Embassy timers for every delay binding;
3. turn I2C, PBus and measurement readiness into externally scheduled edges;
4. enforce deadlines without busy-waiting;
5. return the exact typed completion to the state machine.

Only after that executor is complete should a write-capable
`open-radio-phy-hil` be added. It will be compared against a separate
vendor-oracle image; the open and vendor drivers must never share one ELF
because both require exclusive radio ownership.

## Current ownership seam

The open HAL has two coarse type states:

```text
Radio<WIFI, Owned> -- unsafe prerequisites --> Radio<WIFI, Powered>
```

`Owned` proves that the `esp-hal` singleton and the open register capability
cannot be separated. Finite parent MMIO and reset-sample bindings require a
mutable borrow of `Radio<WIFI, Powered>` and therefore cannot execute through
the safe API while the radio is only `Owned`.

The transition is implemented as a finite 14-operation sequence followed by
nine read-back checkpoints. `open-radio-frontier` stops before that transition,
so its `writes=0` contract remains unchanged.

The separate write-capable image is deliberately build-only:

```console
cargo xtask app build open-radio-power-hil
```

It performs only modem-domain reset, PMU ICG publication, modem source/bus
clock selection and PHY frontend/calibration/I²C clock gates. It does not run
the PHY state machine and does not enable MAC clocks, RX, DMA or TX. `xtask`
rejects `app run open-radio-power-hil`; flashing requires a separately reviewed
HIL procedure with a hard-reset recovery path.

## First power-only hardware result

The bounded transition passed on 2026-07-26 on ESP32-S31 revision v0.0 with a
40 MHz crystal and the open-radio revision `f7cf811`. To avoid changing the
installed firmware, the HIL was linked entirely into SRAM:

```console
CARGO_TARGET_DIR=target/hil/open-radio-power-hil-ram \
  cargo build --locked --release \
  --manifest-path firmware/esp32s31/app/Cargo.toml \
  --bin open-radio-power-hil \
  --no-default-features \
  --target riscv32imafc-unknown-none-elf \
  --features open-radio-power-hil,ram-download

espflash flash --ram --no-stub \
  --chip esp32s31 --port /dev/ttyACM0 \
  --non-interactive --before usb-reset --after no-reset \
  --monitor --no-reset --log-format serial \
  target/hil/open-radio-power-hil-ram/riscv32imafc-unknown-none-elf/release/open-radio-power-hil

# After capturing one terminal PASS or FAIL record:
espflash reset --chip esp32s31 --port /dev/ttyACM0 \
  --non-interactive --before usb-reset --after hard-reset
```

The RAM ELF contained loadable segments only in the `0x2f...` SRAM ranges and
entered at `0x2f003de2`. `--no-stub` is required on the tested revision: the
flash stub timed out during `MemData`, while the ROM downloader completed and
produced:

```text
OPEN_RADIO_POWER_HIL schema=1 writes=prerequisites phy=0 mac=0
OPEN_RADIO_POWER_HIL result=PASS stage=powered
```

A second RAM-only run passed on 2026-07-27 after `RadioRegisters` was changed
to own the generated `svd2rust::Peripherals` singleton and the RISC-V
power/clock/reset path was moved off the compatibility
`Register32`/`read32`/`write32` executor:

```text
OPEN_RADIO_POWER_HIL schema=1 writes=prerequisites phy=0 mac=0
OPEN_RADIO_POWER_HIL result=PASS stage=powered
```

USB hard reset followed both bounded observations. The second run also made no
flash write. For the first run, the partition table, `otadata`, and the first
8 KiB of `ota_0` were additionally read before and after the test and compared
byte-for-byte; all three were unchanged. These results prove both the original
and native-`svd2rust` `Owned -> Powered` prerequisites and their nine
read-back checkpoints on that chip revision. They do not yet prove the
subsequent PHY registration state machine or any MAC operation.

## First PHY-prelude hardware result

The next bounded image uses the real async `run_phy_register` entry but stops
when it receives the first RF binding:

```console
cargo xtask app build open-radio-phy-prelude-hil

CARGO_TARGET_DIR=target/hil/open-radio-phy-prelude-hil-ram \
  cargo build --locked --release \
  --manifest-path firmware/esp32s31/app/Cargo.toml \
  --bin open-radio-phy-prelude-hil \
  --no-default-features \
  --target riscv32imafc-unknown-none-elf \
  --features open-radio-phy-prelude-hil,ram-download

espflash flash --ram --no-stub \
  --chip esp32s31 --port /dev/ttyACM0 \
  --non-interactive --before usb-reset --after no-reset \
  --monitor --no-reset --log-format serial \
  target/hil/open-radio-phy-prelude-hil-ram/riscv32imafc-unknown-none-elf/release/open-radio-phy-prelude-hil
```

Its port implementation can execute only top-level MMIO, Embassy timer, and
I²C-master reset-sample bindings. Receiving RF is the expected terminal
boundary; baseband, temperature, and final-I²C bindings are rejected. The
source audit also forbids direct volatile stores and vendor radio crates.

The RAM-linked image passed on the same revision v0.0 device:

```text
OPEN_RADIO_PHY_PRELUDE_HIL schema=1 writes=prelude rf=0 mac=0
OPEN_RADIO_PHY_PRELUDE_HIL result=PASS next=rf completed=12 mmio=7 delays=3 reset_samples=2
```

The three delays were awaited through the Embassy timer driver. Both reset
status registers were already idle, so no reset pulse was needed. The result
confirms the top-level PHY entry through force-TX/RX, frequency control,
I²C-master readiness, profile application, 40 MHz configuration, and
calibration-clock enable. It does not execute the first RF binding.

A USB hard reset followed the result, and the same partition-table, `otadata`,
and `ota_0` prefix comparisons remained byte-identical. The next HIL boundary
is therefore the first nested RF operation, not another ownership or power
prerequisite.

## Flash-backed PHY-prelude result

The same prelude was subsequently qualified through a real SPI-flash cold
boot. `ota_0` contained the working Wi-Fi STA image, so the HIL was written to
the inactive `ota_1` partition at `0x310000` and selected with the two-copy
`otadata` format:

```console
cargo xtask app build open-radio-phy-prelude-hil

espflash write-bin --chip esp32s31 --port /dev/ttyACM0 \
  --non-interactive --before usb-reset --after no-reset \
  0x310000 \
  target/xtask/esp32s31/apps/open-radio-phy-prelude-hil/image.bin

cargo xtask device select ota_1 --port /dev/ttyACM0
espflash monitor --chip esp32s31 --port /dev/ttyACM0 \
  --non-interactive --log-format serial

# Restore the normal boot selection after capturing the result.
cargo xtask device select ota_0 --port /dev/ttyACM0
```

The final 64 KiB-MMU image was 176224 bytes with SHA-256
`65ee1c5ff68ea72a3360c4210d78fddc9531fa922011f301c6af1f2d8528831d`.
Reading the same range back from `ota_1` produced a byte-identical file.
The second-stage bootloader reported:

```text
esp_image: segment 0: paddr=00310020 vaddr=40000020 ... map
esp_image: segment 4: paddr=00320020 vaddr=40010020 ... map
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_PRELUDE_HIL schema=1 writes=prelude rf=0 mac=0
OPEN_RADIO_PHY_PRELUDE_HIL result=PASS next=rf completed=12 mmio=7 delays=3 reset_samples=2
```

The first flash run exposed a pre-existing contract mismatch: the installed
bootloader used 64 KiB MMU pages while the application descriptor, linker, and
`espflash save-image` command used 32 KiB. The board specification now owns one
64 KiB value shared by all 26 application descriptors and host image tooling;
the XIP linker aligns text to `0x10000`, and `xtask` audits the resulting
binary descriptor. The repeated cold boot above contains no MMU-page warning.

The selector was read back after restoration with valid `ota_0` sequence 1.
The working Wi-Fi STA then booted from `0x10000`, associated, completed its
WPA2 handshake, and acquired IPv4. The HIL remains in `ota_1` for the next
flash-based RF boundary test.

## Full open-PHY and passive-scan result

The same workload now continues past the historical prelude boundary. With
`open-esp-radio-rs` pinned to `6a2b358b6dc3458d2f49d80ff14d20aa3d6fabab`,
it executes the complete cold-PHY and baseband transitions, selects channels
through the open PHY state machine, initializes the source-owned MAC receive
path, and owns a fixed 32-entry RX descriptor list.

The hardware image does not link or call the vendor radio initializer. A
100 ms dwell on every 2.4 GHz channel from 1 through 13 produced:

```text
OPEN_RADIO_PHY_HIL schema=7 writes=full-phy+mac-rx+passive-scan mac=open
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=20 raw_frames=23 dropped=0 ring_epochs=0
```

The records contained real SSID, BSSID, RSSI, channel, RSN, HT, and HE data.
At each channel transition the HIL confirms that `RX_ENABLE` is clear before
rebuilding the list, retunes through `PhyChipChannelTransition`, and only then
republishes DMA ownership. The scanner and management-frame parser are live
`open-esp-radio-esp32s31-wifi-mac` modules; the copy under `migration/` is no
longer the application path.

The generated radio clock/power PAC was then qualified with
`open-esp-radio-rs` pinned to
`a5505af729a01bf714ea23d17a1c56293dac4282`. The 334464-byte OTA image
(`sha256:707ffbae3109711b9c0d077d4bd26aa5fc56dd7ed19eef294386c60943d19dc6`)
was written to `ota_1` at `0x310000` and read back byte-for-byte before boot.
The complete PHY, open MAC RX, and passive-scan workload still passed:

```text
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=14 observed_frames=20 raw_frames=23 dropped=0 ring_epochs=0
```

After restoring the selector, the bootloader confirmed `ota_0` at `0x10000`.
Its resident open-radio HIL image independently completed another live scan
with 17 records from 28 observed frames and no drops. The test therefore
covers the real generated PAC register constants and field masks, not only
host-side equivalence tests.

The next register-ownership stage pins `open-esp-radio-rs` to
`b522b9d2acbf807c435ab6c804d826f72d0600a3`. Its generated PAC adds the
recovered PMU analog-I2C, PHY-PBus, PHY-I2C host/master and 45-word command
RAM descriptions. `complete_rf` now borrows `RadioRegisters` from the
powered radio and passes it through every cold I2C/PBus/MMIO binding, so the
new HAL methods no longer manufacture or receive raw addresses. Each method
records the complete ROM/blob body used for its operation order; unknown
pair/subfield semantics remain explicitly named `UNKNOWN`.

This ownership change was also exercised from a cold SPI-flash boot, rather
than only by host tests. The 335968-byte image
(`sha256:753da1c9aa1b84a80fb812ea8db6a53e4c7887eeafd02bdbc0c0fc474ddb6c33`)
was written to `ota_1` at `0x310000`; reading the exact image length back
produced the same SHA-256 and a byte-identical comparison. The bootloader
selected `ota_1`, the complete PHY initialization reported 5344 RF and 17201
baseband operations, open-MAC receive observed a frame, and the full passive
scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5344 baseband_operations=17201
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=15 observed_frames=20 raw_frames=23 dropped=0 ring_epochs=0
```

After the test, `ota_0` was selected again and the bootloader confirmed that
the resident image was loaded from `0x10000`.

The follow-up ownership pass pins `open-esp-radio-rs` to
`dfa7b4ffb43bf6472b1ec4d11cab59574e71cfa7`. The shared PHY-I2C transaction
and PBus force-test bindings now require `RadioRegisters` themselves, which
propagates the capability through the reusable RFPLL, RXIQ/TXIQ, DCO, gain,
temperature, saturation, power, and power-detector target methods. Their
former `unsafe` wrappers and the raw-owner command leaves were deleted; the
source-only audit now fails if either API shape returns.

The firmware consumer passes the one borrow through every nested calibration
executor and was requalified from flash. Its 335072-byte OTA image
(`sha256:f671c9b8af7ed4f18f9c77343c32de662940afeebbf6493fd25b29abbf529350`)
was byte-identical after reading the same range back from `ota_1`. The owned
command-engine path completed 5461 RF and 17150 baseband operations, received
a frame, and scanned all channels:

```text
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5461 baseband_operations=17150
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=16 raw_frames=20 dropped=0 ring_epochs=0
```

The selector was restored after the run and the bootloader again reported
`boot: Loaded app from partition at offset 0x10000`.

The shared PHY table-memory ownership stage pins `open-esp-radio-rs` to
`ec49f30cad3f6b7266319eb9689d6fc9318fb548`. Its generated PAC now owns the
multifunction aperture at `0x20100844..0x20100868`: one ten-bit PBUS memory
command, three mode-dependent data words, the CFR commit pulse, and six packed
first/last boundary words for twelve PBUS groups. Complete S31 ROM/blob bodies
are recorded for every field and operation; overlapping gain-memory/CFR/PBUS
subfields remain mode-dependent rather than receiving guessed aliases.

The baseband consumer now passes its unique `&mut RadioRegisters` borrow into
the PBUS-memory binding. The transition carries only semantic group, entry,
data, and command values; the old raw addresses, pointer writes, and six raw
snapshot reads were removed.

This path passed another cold SPI-flash qualification. The 335280-byte image
(`sha256:b9d2fcc06ade4f9d3af87b193cacec152002dca4b2731098fcde2e61faa2fa89`)
was written to `ota_1` at `0x310000`, read back for the exact image length, and
compared byte-for-byte before boot. The full cold PHY reported 5748 RF and
17097 baseband operations, received a real frame on channel 1, and completed
the thirteen-channel scan:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5748 baseband_operations=17097
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=14 observed_frames=18 raw_frames=28 dropped=0 ring_epochs=0
```

After the run, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`.

Management TX, authentication, association, and encrypted traffic are the
next live-crate frontier. Their historical implementations remain in the
migration inventory, but have not yet passed without the vendor TX scheduler.

The complete CFR/gain-memory ownership stage pins `open-esp-radio-rs` to
`5d5c84eeb63e2c80bb651af4819ca8bafecc40f5`. The multifunction command word
is now split into the instruction-evidenced common eight-bit index,
mode-dependent gain/PBUS bits and the CFR commit bit. The PAC also owns the
high-byte base-index source configured by `phy_fe_reg_init` and sampled once
by the CFR and TX-gain publishers.

All PBUS-memory, CFR, baseband RX-table, RX-gain and channel TX-gain
publications now require the unique `&mut RadioRegisters` borrow. The former
TX-gain `extern "C"` leaf and its five raw pointers were removed. Rust
expresses the vendor seed/output halfword concatenation with ordinary indexing
over the owned `PhyWifiTxGainImage`; a host test covers the exact field
boundary.

This path passed a fresh cold SPI-flash qualification. The 336352-byte image
had SHA-256
`9199acd6447255137704466d6ccf7da4a1e4d7bd1220cd769e19194988f70917`.
It was written to `ota_1` at `0x310000`, read back for the exact length, and
compared byte-for-byte before boot. The full open PHY, real receive path and
thirteen-channel scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5065 baseband_operations=17032
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc07406a4 length=464 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=23 raw_frames=31 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The PHY AGC/11b ownership stage pins `open-esp-radio-rs` to
`4912f1059a38babda973d1e37924c982d2722cfc`. Four complete rev0 ROM bodies
now provide the primary evidence for a generated `PHY_AGC_ORACLE` PAC block:
`phy_bb_agc_reg_update`, `phy_disable_agc`, `phy_enable_agc`, and both
branches of `phy_rx_11b_opt`. Sixteen internal register identities and the
shared `MODEM_SYSCON.WIFI_BB_CFG` field retain `OPAQUE`/`UNKNOWN` names where
the electrical meaning is not public.

The baseband state machine, RX-table suffix and every channel transition pass
their unique `&mut RadioRegisters` borrow into the new `phy_agc` HAL. Its host
model covers the exact fifteen-write baseband update, the enable/disable
edges, both 11b branches and preservation of unrelated bits. A source-only
audit rejects the former raw AGC/11b addresses from the live PHY crate.

The exact 336352-byte HIL image had SHA-256
`4e1bcb5ce59b8f9fbd9ae9f730c6ede36fceaad75e65a13e3300a35fe35a40b3`.
It was written to `ota_1` at `0x310000`, read back for the exact length and
compared byte-for-byte before boot. The owned AGC path completed cold PHY
initialization, received a real frame and retuned through all thirteen
channels:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4804 baseband_operations=16933
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc06f06a4 length=444 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=16 observed_frames=27 raw_frames=35 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The PHY post-initialization ownership stage pins `open-esp-radio-rs` to
`92ea76cb5eb010539df4d0e40d07bddadf4f93fe`. Complete pinned
`libphy.a` bodies now source `phy_reg_update_new` and its `phy_set_ftm_en`
tail, while the complete rev0 ROM `phy_wifi_agc_sat_gain` leaf proves the two
saturation-gain stores. The generated `PHY_AGC_ORACLE` block adds the five
post-init register identities and shares the instruction-proven nine-bit
window field at `0x20107104`.

The live baseband binding and `phy_reg_init` pass their existing unique
`&mut RadioRegisters` borrow into safe `phy_agc` HAL methods. Those methods
preserve all seven post-init writes, the fresh read before each update at
`0x201078c8`, and the dynamic saturation value. The former raw-MMIO C ABI and
duplicate mask helpers are absent from the live PHY crate.

The vendor-oracle, owned-power and full open-radio images built successfully
at 120224, 73280 and 336352 bytes respectively. The full image SHA-256 was
`19bdf460eb1109a1eddca8aa90b6648b0634df951f6604b39cd71fd6ba131ec5`.
It was written to `ota_1` at `0x310000`; reading exactly 336352 bytes back
produced the same digest and a byte-identical comparison before the selector
was changed. Cold PHY initialization, real RX and the thirteen-channel scan
then passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5333 baseband_operations=17173
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc07406a4 length=464 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=17 observed_frames=29 raw_frames=32 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The typed AGC initialization, RF RX saturation and gain-limit stage pins
`open-esp-radio-rs` to
`a49bc6ba6f51d97590894f208c58fb8b23523f3a`. Complete rev0 ROM
`phy_agc_reg_init` and `phy_rfrx_sat_rst` bodies, together with complete
`libphy.a[phy_rx_gain.o]::phy_set_rx_gain_table`, now source the generated
PAC fields used by these operations. Fields whose electrical purpose is not
public remain explicitly named `UNKNOWN`.

The safe HAL retains all ten AGC-init updates, either complete three-write
RF-saturation branch and both final RX-gain-limit writes. The live PHY path
passes its unique `&mut RadioRegisters` through `PhyRxGainInitMmioBinding`;
raw access to `0x201008bc`, `0x2010705c`, `0x20107068`, `0x20107094`,
`0x20107128` and `0x2010713c` is rejected by the source audit.

The vendor-oracle, owned-power and full open-radio images built at 120224,
73280 and 336368 bytes. The full image SHA-256 was
`2551be885415b44769a3a9857daa4d0057257ce43780d27827ca40133d72619f`.
It was written to `ota_1` at `0x310000`; reading exactly 336368 bytes back
produced the same digest and a byte-identical comparison. The cold open PHY,
real RX and full passive scan then passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4622 baseband_operations=17401
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc07406a4 length=464 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=16 observed_frames=31 raw_frames=42 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
and monitor logs were deleted.

The shared AGC/PBus/RX-compensation/antenna ownership stage pins
`open-esp-radio-rs` to
`4acdedb7b1f16dc93b0bf24a596e97437c5e7edf`. The recovered SVD v0.6 now
represents shared `0x2010702c` with one physical PAC identity. Complete rev0
ROM `phy_pbus_force_mode` and `phy_ant_init`, plus complete pinned
`libphy.a[phy_reg.o]::phy_set_rx_comp_new`, source the independent fields at
`0x20100884`, `0x2010088c`, `0x2010702c`, `0x20107030`, `0x201070a0`,
`0x2010711c`, and `0x20107120`. Unknown electrical meanings remain explicitly
named `UNKNOWN`.

The safe HAL retains the two RX-compensation writes, the delayed PBus
high-byte/set/clear sequence, and all three antenna updates. TX-DC,
TX-DC/PWDET, PWDET, TX calibration environment, RXIQ initialization, RX-gain
DC, TXIQ, and RX-saturation HIL bindings now pass the same unique
`&mut RadioRegisters`; the driver source audit rejects raw access to this
localized cluster.

The vendor-oracle, owned-power and full open-radio images built at 120224,
73280 and 336368 bytes. The full image SHA-256 was
`32c1a2ab35d2f6197f815ac15ea3e4cb2e335dea7aaab9b96235c63445c36300`.
It was written to `ota_1` at `0x310000`; reading exactly 336368 bytes back
produced the same digest and a byte-identical comparison. Cold open-PHY
initialization, real RX, and the full passive scan then passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4641 baseband_operations=17622
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc07406a4 length=464 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=14 observed_frames=22 raw_frames=28 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The typed channel-cleanup stage pins `open-esp-radio-rs` to
`1d3bc4c3f8646dfcbf747f3cf4954c9d9b0b3aa5`. Complete pinned
`libphy.a[phy_reg.o]::phy_dc_mem_clr` now sources the SVD v0.7 bit-20 pulse at
`0x2010703c`. Complete rev0 ROM `phy_bbpll_cal` sources the two encodings in
bits 3:2 of the already shared `PHY_I2C_MASTER.MASTER_CONTROL` at
`0x2010f818`. Both safe HAL methods require `&mut RadioRegisters`; the raw C
leaves, duplicate mask helpers, and unused raw master-register wrapper are
gone.

The vendor-oracle, owned-power and full open-radio images remained 120224,
73280 and 336368 bytes. The full image SHA-256 was
`2a902003728d2a359c0999064271a4cf6575d9dc5c9006605166fe65611f59eb`.
It was written to `ota_1` at `0x310000`; reading exactly 336368 bytes back
produced the same digest and a byte-identical comparison. Cold open-PHY
initialization, a real RX frame, and the full passive scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5289 baseband_operations=16968
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc02306a4 length=140 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=15 observed_frames=21 raw_frames=24 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The frequency/channel ownership stage pins `open-esp-radio-rs` to
`0d9cea966e95134239b2d757c605c05ac2a492b0`. SVD v0.8 now gives the
frequency-memory controller, channel switch, NRX, bandwidth, TX-offset and
TX-cap command paths one physical PAC identity each. The safe HAL retains the
complete rev0 ROM ordering for module reset, hardware/software frequency
control, packed I2C-number address programming, the channel-switch pulse and
ready observation. The Dcode HIL binding now also receives the same unique
`&mut RadioRegisters`; it no longer reaches the target through an unowned
internal MMIO handle. Bits whose electrical purpose could not be proven from
the pinned ROM and `libphy.a` bodies remain explicitly named `UNKNOWN`.

The vendor-oracle, owned-power and full open-radio images built at 120240,
73280 and 336448 bytes. The full image SHA-256 was
`d39aabb9e0c026dc05cf3d3a41f087ab8902d1d097bd322083ecbd82480bb876`.
It was written to `ota_1` at `0x310000`; reading exactly 336448 bytes back
produced the same digest and a byte-identical comparison. Cold open-PHY
initialization, repeated typed channel changes, a real RX frame and the
thirteen-channel passive scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5081 baseband_operations=17181
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=2 descriptor=0 word0=0xc04d06a4 length=308 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=9 observed_frames=15 raw_frames=17 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The baseband/PWDET ownership stage pins `open-esp-radio-rs` to
`e2ebc8b32b3e1e944ada1c89973af8f2eac4b8ef`. SVD v0.9 and the generated PAC
now own the 36-register `PHY_BASEBAND_CONFIG_ORACLE` aperture and the
independently addressed `PHY_POWER_DETECTOR_AUX_ORACLE` register. Complete
rev0 ROM and pinned `libphy.a` bodies source the register addresses, masks,
values and access order for baseband initialization, watchdog/PA/noise
configuration, TX-power tracking, PWDET and TX-DC calibration. Unresolved
electrical meanings remain explicitly named `UNKNOWN`.

The new safe `phy_baseband` and `phy_power_detector` HAL modules preserve
every separate fresh-read update, including the final baseband OR and the
individual PWDET clears. Cold initialization and all reusable BB/PWDET/TX-DC
bindings receive the same unique `&mut RadioRegisters` borrow. Even PWDET
ready/result sampling now requires that capability; the no-argument global
MMIO readers and the duplicate raw implementation in `radio_hal.rs` are
gone.

The full open-radio image built at 336288 bytes with SHA-256
`cc5c815a2f9e7775e737568b41feb97bcbd2363982c60d53e820fdd38db13281`.
It was written to `ota_1` at `0x310000`; reading exactly 336288 bytes back
produced the same digest and a byte-identical comparison. A second cold run
captured the complete acceptance sequence for the typed baseband/PWDET path:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4919 baseband_operations=17173
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc07406a4 length=464 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=17 observed_frames=31 raw_frames=37 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was selected again. The bootloader had already
confirmed `boot: Loaded app from partition at offset 0x10000` after the first
run; the second selector restoration used the same generated OTA-data image.
The temporary readback and oracle extraction files were deleted.

The owned IQ-estimator stage pins `open-esp-radio-rs` to
`ce4c7b287c4837b0897f6153e296401d3cd9e86a`. SVD v1.0 and the generated PAC
now own the eleven-register estimator block, including configuration,
control, signed DC/IQ and signal-power results, ready status, and the activity
word shared with RX-saturation sampling. Complete rev0 ROM bodies and
complete pinned `libphy.a::phy_check_rx_sat` provide the register and
access-order evidence.

All estimator and saturation samples now require the unique
`&mut RadioRegisters` borrow. Separate safe HAL methods preserve the two
different complete-ROM signal read orders used by `phy_rxiq_get_mis` and
`phy_get_rx_sig_pwr`; this also corrects the previous raw wrapper's
non-oracle read order while preserving the same semantic snapshot fields.

The full open-radio image built at 336032 bytes with SHA-256
`11dfd2da195ce78462703d3d8a800dc3ba23779b0a7a872dd9422956f1fcc5aa`.
It was written to `ota_1` at `0x310000`; reading exactly 336032 bytes back
produced the same digest and a byte-identical comparison. Cold PHY
initialization, a real RX descriptor and the thirteen-channel passive scan
passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4841 baseband_operations=16921
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc06f06a4 length=444 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=20 raw_frames=22 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was restored and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The owned temperature-sensor stage pins `open-esp-radio-rs` to
`149d88d85f7fb32ecaa5d745a333d0a57c216b7d`. SVD v1.1 and the generated PAC
now own the shared temperature-code/power word, sensor-control word and the
independently addressed system-control word. Complete pinned
`libphy.a::phy_tsens_read_init` and complete rev0 ROM temperature bodies
provide the address, field and fresh-read-order evidence.

The PHY temperature action no longer exposes an MMIO address or mask.
Initialization and the one-shot code sample both require the unique
`&mut RadioRegisters` borrow; the sample completion carries only the
already-extracted `u8` code. Both the full open-radio HIL and the
vendor-comparison HIL compile against this boundary.

The vendor-comparison, owned-power and full open-radio images built at
120416, 73280 and 339216 bytes respectively. The full image SHA-256 was
`9d877fcb1453233bbc7b45f10faa335492f1a7a531cdbb1aad4e458ded69f2cb`.
It was written to `ota_1` at `0x310000`; reading exactly 339216 bytes back
produced the same digest and a byte-identical comparison. Cold PHY
initialization, a real RX descriptor and the complete passive scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=4591 baseband_operations=17096
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=3 descriptor=0 word0=0xc06906a4 length=420 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=13 observed_frames=23 raw_frames=28 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was restored and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The owned RX-DCO control stage pins `open-esp-radio-rs` to
`7e3cedd80c615bb3247f8f5b13dbe692170a739f`. SVD v1.2 and the generated PAC
now own bits 23:22 of the shared control word at `0x2010_0434`. Complete
pinned `libphy.a::phy_xtal_duty_cal` and complete rev0 ROM
`phy_pbus_rx_dco_cal` independently prove the save, clear and restore
sequence while leaving the field's electrical name explicitly `UNKNOWN`.

The safe HAL preserves the exact two fresh reads used to capture and clear
the field and the final fresh restore read. RX-DCO, RX-DC calibration,
RX-gain initialization, crystal-duty and cold-PHY paths share that single
`&mut RadioRegisters` owner. The three target bindings that now contain only
PAC-backed RX-DCO and PBus operations no longer require `unsafe`; their old
raw volatile helpers and duplicate PBus result table were removed.

The vendor-comparison, owned-power and full open-radio images built at
120416, 73280 and 339440 bytes respectively. The full image SHA-256 was
`bd8108106b1890944e0f2573249235a49c1bd8be34bd192d1dfa15a6844940bb`.
It was written to `ota_1` at `0x310000`; reading exactly 339440 bytes back
produced the same digest and a byte-identical comparison. The full cold
calibration graph, a real RX descriptor and the thirteen-channel passive
scan passed:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5311 baseband_operations=16981
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc05f06a4 length=380 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=18 observed_frames=28 raw_frames=33 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was restored and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The cold-PHY prelude ownership stage pins `open-esp-radio-rs` to
`bd535c4c1f624504aef7d93ab1664fdb11dbbf07`. SVD v1.3 adds the read-only
SDM-deadline identity and moves the live force-TX/RX, two PHY-I2C master
reset, fixed 40 MHz tick, and deadline consumers behind the generated PAC
and safe HAL. The reset action now carries only host/sample identity and
returns `busy: bool`; physical addresses, masks, and raw status words no
longer cross into calibration policy. The deadline observation target no
longer requires an `unsafe` call.

Complete rev0 ROM `phy_force_txrx_off`, `phy_i2c_master_reset`, and
`phy_wait_i2c_sdm_stable`, plus complete pinned
`libphy.a[phy_init.o]::phy_get_xtal_freq`, are the exact operation sources.
The Rust transition still owns both force/release delays, the bounded reset
sampling policy, and the wrapping 9,999-cycle deadline; no safe HAL leaf
spins or advances hidden state.

The vendor-comparison, owned-power and full open-radio images built at
120416, 73280 and 341584 bytes respectively. The full image SHA-256 was
`122c8554e6bc1b412922a545f60071dd285be0956252c4fa98659a0c9a694104`.
It was written to `ota_1` at `0x310000`; reading exactly 341584 bytes back
produced the same digest and a byte-identical comparison. Revision v0.0
completed the full cold calibration graph, received a real frame, and scanned
all thirteen channels:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5638 baseband_operations=17201
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc06106a4 length=388 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=14 observed_frames=19 raw_frames=21 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was restored and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The shared PHY status/clock ownership stage pins `open-esp-radio-rs` to
`d08079d36b24a8634030b7ff6ab1a4dffb5238fd`. SVD v1.4 closes the remaining
raw frontier on physical word `0x20100890`: the RX clock pair and the two
independent RXIQ-root status consumers now share non-overlapping generated PAC
fields. Complete pinned `libphy.a[phy_rx_gain.o]::phy_rxiq_cal_init`, size
`0x198`, proves two root-entry writes and all eight correction prefix/suffix
writes.

This pass corrected a real ordering mismatch rather than only changing API
shape. The former raw RXIQ prefix collapsed four fresh-read blob writes into
two RMW operations. The safe HAL now retains all four observable intermediate
states, and RX-gain publication, TXIQ loopback, and RXIQ initialization pass
the unique `&mut RadioRegisters` capability without an `unsafe` call.

The vendor-comparison, owned-power and full open-radio images built at
120416, 73280 and 341600 bytes respectively. The full image SHA-256 was
`6d8e1192fb10946e2c7d9be5a3e846a8097d3172dd283043639b238502e90da6`.
It was written to `ota_1` at `0x310000`; reading exactly 341600 bytes back
produced the same digest and a byte-identical comparison. Revision v0.0
completed the corrected cold-calibration graph, received a real frame, and
scanned all thirteen channels:

```text
boot: Loaded app from partition at offset 0x310000
OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration=true mmio=12 delays=5 reset_samples=2 rf_operations=5044 baseband_operations=17228
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=1 descriptor=0 word0=0xc04d06a4 length=308 control=0x8805b000 next=0x00003434 last=0x01003428
OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels=13 records=17 observed_frames=31 raw_frames=36 dropped=0 ring_epochs=0
```

After qualification, `ota_0` was restored and the bootloader confirmed
`boot: Loaded app from partition at offset 0x10000`. The temporary readback
image was deleted.

The native `svd2rust` ownership stage was then tested from flash after
PHY-I²C and PBus target access moved out of the compatibility MMIO facade.
The first image reached RF operation 177 and reported an unexpected PBus
binding for signed RX-DCO value `0xff05`. Disassembly of
`_oracles/esp32s31_rev0_rom.elf` showed that complete
`phy_pbus_force_test` at `0x2f82_4228` retains the low eleven argument bits
after composing selector/value/path; complete `phy_pbus_rx_dco_cal` at
`0x2f82_8f44` passes the signed halfword unchanged. The PAC and SVD now record
and reproduce that exact overlapping command encoding instead of rejecting
the value as wider than the separately visible nine-bit physical value field.

The corrected no-password image was 542704 bytes with SHA-256
`b9f75ea74d89a5828b4506eb892137a0896fcdb3bd96c19b3540173a020bb723`.
It cold-booted from `ota_1`, completed the full PHY and MAC-RX setup, submitted
the channel-six probe request, received real frames and parsed both nearby
networks. The target AP measured `-13 dBm` in that run:

```text
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=6
OPEN_RADIO_PHY_HIL stage=scan-record ... channel=6 rssi=-13 privacy=true rsn=true ht=true he=true
OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-config error=missing-OPEN_RADIO_STA_PASSWORD
```

A password-enabled follow-up image was 544368 bytes with SHA-256
`0f97885569423bbd819cd88833554414b244eb52c15ae7701396ab80a2373e3f`.
PMK derivation passed and the target AP measured `-11 dBm`, but the existing
open MAC-TX frontier remained: authentication TX completed with status `5`
and no authentication response arrived. Therefore PHY initialization, RX and
scan are qualified under the generated-PAC scheme; STA connection is not.

```text
OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pmk-derive
OPEN_RADIO_PHY_HIL stage=sta-auth-tx channel=6 status=5
OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response error=timeout
```

Both images were written only to `ota_1`. After each observation `ota_0` was
selected again, and the device was reset into the working slot.

## Current driver/application boundary audit

The 2026-07-31 audit covers every Rust source that imports `open-esp-radio`
and every direct `0x2010_....` access in the firmware workspace. The current
split is no longer the early frontier described above:

- scan, authentication, association, WPA2, HT/HE frame codecs, BlockAck
  protocol state, rate/vector formats, RX rings, TX descriptors, PAC register
  identities and finite PHY leaf executors live in `open-esp-radio-rs`;
- the copy-free Embassy TX pool and the exact cache-ESF A-MSDU copy/recycle
  edge now live in `open-esp-radio-embassy-net` and the `open-esp-radio`
  ESP32-S31 facade;
- `StaTxRatePolicy` now joins the recovered ordinary/A-MPDU schedule with
  association width and HE LTF/LDPC capabilities, plus typed HT MCS/GI and
  HE-SU MCS/GI certification overrides inside the MAC crate; this application
  no longer decodes the overlapping Dot11N/Dot11Ax rate bytes or updates
  ACK-SNR itself;
- the application retains only credentials, network configuration, task
  scheduling, HIL traffic policy, diagnostic snapshots and the composition of
  the still-incomplete driver services.

The remaining reusable code that must move is, in priority order:

1. `PreludePort` PHY composition. Its pure `complete_*` dispatchers currently
   compose the already-promoted finite I2C/PBus leaves in the application.
   They belong in `open-esp-radio-esp32s31-phy::target_executor`, with an
   optional observer supplied by HIL for ROM/MMIO comparisons. Raw snapshots
   and the revision-pinned ROM equivalence call must remain in this repository.
2. The connected TX service. `TxStorage`, single-MPDU retry, A-MPDU BlockAck
   retry, queue contention state and protected cache-TX batch publication are
   MAC driver behavior. The application should eventually submit an owned
   Ethernet lease plus policy and receive a typed completion report. Rate
   selection and completion ACK-SNR observation have already moved; the
   remaining boundary is aggregate construction, retry ownership and the
   executor-neutral completion edge.
3. The concrete RX hardware resource owner. `DmaBuffer`, `RxStorage`,
   interrupt handoff and timeout-abort composition should become an ESP32-S31
   integration type. The former `ConnectedTxHardware` is already
   `open_esp_radio::esp32s31::cooperative_tx::CooperativeTxHardware`; it
   borrows the application's unique PAC owner for finite transactions without
   retaining that borrow across an asynchronous completion wait. Linker
   section selection and the actual `StaticCell` allocations remain
   board/application composition.
4. The stop/idle/retune/restart transaction should move beside the MAC/PHY
   link owner. The channel number and scan dwell policy remain in the scanner.

The following code is intentionally not a driver candidate:

- AP credentials, DHCP/static IPv4 selection and Embassy socket tasks;
- benchmark duration, synthetic traffic, HE matrix selection and qualification
  thresholds;
- emergency USB logging, raw MMIO page dumps and ROM/blob comparison calls;
- host `xtask` packet injection/capture/report generation;
- bootstrap, OTA selection, PSRAM layout and linker-section auditing.

Feature-gated `wifi_scan` remains a vendor oracle, not an alternate open
driver. Its old read of the vendor global `s_phy_get_max_pwr` was removed:
open TX power is owned by `PhyTxTargetPowerProfile`, and adjacent bytes of a
vendor global are not a supported ABI.

### Cache-TX/A-MSDU hardware result

The standard `psram-code-psram-data` image keeps the interrupt code in internal
SRAM (10,900 bytes) while application code and ordinary data use PSRAM. On the
controlled HT40 SGI link, the referenced path built 21-MPDU A-MPDUs containing
42 Ethernet MSDUs and 63,588 Ethernet bytes. The first implementation consumed
one newly produced frame without its partner and sent one ordinary MPDU after
almost every aggregate. Dedicated counters proved the defect:

```text
tx_ampdu=3773 tx_ampdu_attempts=3956
tx_ampdu_individual_retry_mpdu=50 tx_ampdu_spill_frames=3766
tx_attempts=7829
```

Bounded pair coalescing removed that ownership/scheduling leak:

```text
tx_ampdu=2942 tx_ampdu_attempts=3249
tx_ampdu_individual_retry_mpdu=92 tx_ampdu_spill_frames=0
tx_attempts=3355
```

The measured two-MSDU A-MSDU profile remained at approximately
72--75 Mbit/s. Because removing almost one hardware transmission per aggregate
did not increase offered throughput, this run localizes that profile's
remaining limit above radio/DMA, in serialized producer scheduling and packet
construction. Increasing A-MPDU or DMA storage again is not justified by this
evidence.

The ordinary referenced-cache profile does not need the jumbo A-MSDU backing.
It now uses 64 fixed 1.6-KiB network allocations: one hardware-owned
32-MPDU batch and one producer-owned burst. Once a complete ready queue stopped
paying an unconditional Embassy yield, preparation fell to approximately
225 us per 48,448-byte aggregate and real `embassy-net` UDP uplink sustained
98.752--108.237 Mbit/s. Thus 100+ Mbit/s is no longer a synthetic raw-MAC-only
result.

The first post-transfer bidirectional run reported 26 `BUFFER_FULL` events.
The defect was in HIL instrumentation: `udp-rx-direct` and periodic `udp-tx`
metrics used the synchronous ROM `ets_printf` path while traffic was active.
Those metrics now use the bounded asynchronous USB logger; boot and failure
records retain the immediate path. The final strict Rust-hosted rerun offered
25.001 Mbit/s downlink and measured:

```text
direct RX median       25.006 Mbit/s
referenced TX floor    68.276 Mbit/s
RX median + TX floor   93.282 Mbit/s
BUFFER_FULL                 0
FIFO_OVERFLOW               0
pairwise MIC failures       0
parser/policy rejections    0
```

The same image retained the 10,900-byte internal-SRAM ISR frontier. The host
qualifier now accepts both the raw-MAC and referenced-network TX evidence and
waits for the asynchronous logger to publish the post-load DMA snapshot.
Because TX and RX report independent five-second windows, it uses the minimum
complete TX sample as a conservative overlap floor; the old median could
select faster pre/post-load samples and overstate the bidirectional sum.

The subsequent ownership transfer removed the HIL application's custom
`IrqSink`. `open-esp-radio::esp32s31::embassy_irq::EmbassyMacIrqRuntime` now
joins the executor-neutral MAC `IrqState` to the coalescing Embassy RX/TX
wakes, so raw interrupt-bit classification and RX-before-TX publication order
are driver-owned. The application retains only the protocol-specific RX
service callback. Host tests and the complete target image gate pass after
this transfer; the strict bidirectional hardware rerun remains the next
regression gate.

### Vendor-oracle retirement from this repository

The standalone `open-radio-vendor-oracle-hil` binary and its application
feature/`xtask` workload were retired after byte-level source comparison and a
successful target build in `open-esp-radio-rs`. Its authoritative location is
now `hil/vendor-oracle/esp32s31` in the driver repository. That excluded
workspace pins the identity of all 20 closed inputs in `oracles.lock`, while
`cargo hil doctor` proves that neither the normal HIL nor driver crates resolve
`esp-phy`, `esp-rtos` or `esp-wifi-sys`. Historical oracle results below remain
provenance; new oracle work must use `cargo hil oracle verify/build/flash` from
the driver repository.
