# PAC, MMIO and unsafe audit

Audit date: 2026-07-27.

## PAC generation

`svd/esp32s31-radio.svd` is the editable register source.
`tools/pac-gen` pins the `svd2rust 0.37.1` Rust library and generates the
`open-esp-radio-svd-esp32s31` crate with the architecture-neutral target.
Run it through the portable Cargo alias `cargo pac-gen`; `cargo pac-gen
--check` verifies reproducibility. It does not execute a shell or require a
separately installed `svd2rust` binary. The Rust runner pipes the generated
source directly through the toolchain's `rustfmt` component so normal
workspace formatting checks remain valid. The compatibility PAC re-exports
this crate as `open_esp_radio_pac_esp32s31::svd`.

Before invoking `svd2rust`, the Rust runner parses every peripheral, cluster
and register span and requires it to fit wholly inside one evidenced
ESP32-S31 1-MiB MMIO decode window. A peripheral outside those windows, or a
register crossing a window boundary, makes both generation and `--check`
fail. This prevents an incorrect base address from becoming safe-looking
generated Rust.

The SVD has no CPU interrupt table, so selecting `none` avoids coupling this
radio PAC to a particular RISC-V runtime.

## Hardware verification

The ownership/native-access transition passed the isolated power-only HIL on
2026-07-27:

- ESP32-S31 revision v0.0, 40 MHz crystal;
- RAM-only ROM download with `espflash --ram --no-stub`;
- no PHY state-machine, MAC, DMA or TX execution;
- terminal result:
  `OPEN_RADIO_POWER_HIL result=PASS stage=powered`;
- USB hard reset immediately followed capture; flash was not modified.

This result exercises the `Radio::power_up` implementation described below.
Target and host tests now execute the same semantic `PowerClockControl`
sequence; only the integration implementation touches the official PAC.

The same revision subsequently cold-booted the flash-backed full PHY/MAC HIL
after PHY-I²C and PBus were moved to the generated PAC. The first run exposed
a real compatibility error at RF operation 177: the safe PBus wrapper rejected
the signed RX-DCO halfword `0xff05` as wider than the physical nine-bit value
field. Complete ROM `phy_pbus_force_test` at `0x2f82_4228` shifts the halfword,
combines it with selector/path, and only then applies `0x0001_fffc`;
`phy_pbus_rx_dco_cal` at `0x2f82_8f44` supplies that signed halfword without
narrowing it. The instruction-exact encoder and these source addresses are now
recorded both in the PAC and SVD.

After that correction, revision v0.0 passed the full cold PHY graph, MAC RX,
channel-six probe-request submission, real frame reception and scan parsing:

```text
OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame channel=6
OPEN_RADIO_PHY_HIL stage=scan-record ... channel=6 rssi=-11 ... rsn=true
```

The password-enabled follow-up also passed WPA2 PMK derivation, then reached
the already separate MAC-TX frontier: authentication TX status `5` and an
authentication-response timeout. This does not qualify STA connection, but it
places the remaining failure after PAC-backed PHY initialization and working
RX/scan. The test image was written only to `ota_1`; `ota_0` selection was
restored and the device reset after both observations.

The older `power` module remains a temporary compatibility facade for
`Register32`/`Field32` consumers. New register identities must be added to the
SVD and consumed through `svd`; they must not extend that facade. HAL and MAC
can migrate one peripheral at a time before the facade and its raw-address
accessor are removed.

`RadioRegisters` now owns the complete generated `svd2rust::Peripherals`
value. The value is private: HAL, MAC and PHY cannot steal or retain an
individual generated peripheral. Safe PAC methods expose operations through a
mutable borrow of that owner. The first migrated target paths are:

- modem/PMU clock, reset and power prerequisites used by `Radio::power_up`;
- the fixed 40 MHz PHY tick and the recovered SDM deadline counter;
- RF and analog PHY-I²C power/reset edges;
- both PHY-I²C command hosts, command RAM, clock selection and BBPLL mode;
- PBus debug/work transitions, force-test commands, result windows and
  RX/TX clock pairs.

Their sequencing remains in the HAL, while register field access and the small
`svd2rust` unsafe blocks for bounded multi-bit writes stay in the PAC. The old
raw-address `PowerSequence` and its fake `RegisterIo` path have been removed;
host tests record and verify the same semantic capability calls as the target.

## Peripheral scope

The word "radio" is not one peripheral ownership block. The recovered SVD
currently touches three independently decoded chip-level windows:

- `0x2010_0000..0x201f_ffff`: modem/radio core, including PHY and Wi-Fi MAC;
- `0x2070_0000..0x207f_ffff`: LP-system and PMU dependencies;
- `0x2080_0000..0x208f_ffff`: LP analog/peripheral dependencies such as
  temperature sensing.

The latter two are dependencies of the radio driver, not parts of the Wi-Fi
MAC/PHY register fabric. Their source is the pinned ESP32-S31 `reg_base.h`;
the `0x201x_xxxx` identities additionally come from modem structures and
complete ROM/blob MMIO instructions. The PAC exposes this distinction through
`Esp32s31MmioWindow`, while one top-level `RadioRegisters` owner still
serializes cross-window initialization sequences.

The separately decoded HP-system window at
`0x2050_0000..0x205f_ffff` is still documented by `Esp32s31MmioWindow`, but
the custom SVD generator now rejects it. `HP_SYS_CLKRST` ownership and field
access are delegated entirely to the official `esp-hal` PAC.

Within those windows the recovered SVD currently covers:

- Wi-Fi MAC and its integrated RX DMA/BlockAck register windows;
- PHY baseband, AGC, PBus, PHY-I2C and PHY table memories;
- `MODEM_SYSCON`, `MODEM_LPCON` and `PMU` clock/reset/power prerequisites
  still awaiting the same split;
- PHY temperature sensor and its system clock/control register.

This is not the desired final ownership boundary. The ESP32-S31 PAC used by
the `esp32s31-async-platform` HAL branch already describes these complete
chip-level peripherals:

| Recovered identity | Official PAC identity | Base | Migration |
| --- | --- | --- | --- |
| `MODEM_SYSCON` | `esp32s31::MODEM_SYSCON` | `0x2010_9c00` | pending |
| `MODEM_LPCON` | `esp32s31::MODEM_LPCON` | `0x2010_f000` | pending |
| `PHY_I2C_MASTER` | `esp32s31::I2C_ANA_MST` | `0x2010_f800` | pending |
| `HP_SYS_CLKRST` | `esp32s31::HP_SYS_CLKRST` | `0x2058_7000` | removed |
| `PHY_POWER_DETECTOR_AUX_ORACLE` | `esp32s31::LP_AON_CLKRST` | `0x2070_1000` | pending |
| `PMU` | `esp32s31::PMU` | `0x2070_4000` | pending |
| `PHY_TEMPERATURE_SYSTEM_ORACLE` | `esp32s31::LP_PERICLKRST` | `0x2071_0000` | pending |
| `PHY_TEMPERATURE_SENSOR_ORACLE` | `esp32s31::LP_TSENS` | `0x2081_8000` | pending |

Those identities must move behind a platform capability borrowed from the
official `esp-hal`/PAC owner. They must then be removed from the recovered SVD;
otherwise two Rust singleton types claim the same physical MMIO. The custom
PAC remains necessary for the undocumented PHY/baseband aggregates, PHY
command RAM/deadline blocks and Wi-Fi MAC/RX-DMA registers that the official
PAC does not model. Until this split is complete, the private
`RadioRegisters` singleton is a migration serialization mechanism, not proof
of exclusive chip-wide ownership.

There is no raw access to the chip's general AXI-GDMA peripheral in the live
driver. Wi-Fi descriptors are owned SRAM objects shared with the Wi-Fi MAC
DMA engine; their volatile words are not peripheral MMIO.

## Remaining unsafe outside the PAC

Not all upper layers are safe yet:

- `open-esp-radio-phy-esp32s31::radio_hal` still contains the main raw-MMIO
  compatibility leaves;
- PHY `execute_target` methods are marked unsafe where they call those leaves
  or require the caller to uphold hardware sequencing;
- MAC descriptor, intrusive queue and A-MPDU code uses unsafe pointer access
  for DMA-owned SRAM;
- WPA2 frame owners use volatile writes only to erase secret material;
- HAL ownership has two explicit unsafe transitions: the initial singleton
  claim and adoption after external/vendor initialization. The initial claim
  binds the integration token to the generated PAC singleton.

The unused C-ABI-shaped MAC leaves were removed after repository-wide and
integration-repository searches found no consumer. Their source-owned
replacements live in the MAC crate behind a borrowed `Mmio` capability.
The FE/BB gate and calibration-clock leaves now use native generated PAC
access; their full-register `unsafe` writes are confined to the PAC and cite
the complete ROM/blob sources.

The power, PHY-I²C and PBus vertical slices are complete. PHY-I²C completion
observations now require `&mut RadioRegisters`; this exclusive borrow is
propagated through every nested cold-PHY binding rather than weakening the PAC
owner to a shared MMIO handle. The continuing removal target is `radio_hal`:
describe every remaining register in the SVD, expose an ownership-bound PAC
operation, pass the existing `RadioRegisters` borrow into each leaf, and
delete the unused C-ABI compatibility functions.
Descriptor-memory unsafe is a separate ownership problem and must not be
hidden inside the peripheral PAC.
