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

The word "radio" is not one peripheral ownership block. The chip map exposes
three independently decoded 1-MiB windows used by the recovered initialization
sequence:

- `0x2010_0000..0x201f_ffff`: modem/radio core, including PHY and Wi-Fi MAC;
- `0x2070_0000..0x207f_ffff`: LP-system and PMU dependencies;
- `0x2080_0000..0x208f_ffff`: LP analog/peripheral dependencies such as
  temperature sensing.

The latter two are dependencies of the radio driver, not parts of the Wi-Fi
MAC/PHY register fabric. Their source is the pinned ESP32-S31 `reg_base.h`;
the `0x201x_xxxx` identities additionally come from modem structures and
complete ROM/blob MMIO instructions. All recovered `0x207x/0x208x`
dependencies are now delegated to official platform PAC singletons. The
custom SVD generator therefore accepts only the
`0x2010_0000..0x201f_ffff` modem/radio decode window; the legacy raw
compatibility owner remains narrower still at `0x2010_0000..0x2010_ffff`.

The separately decoded HP-system window at
`0x2050_0000..0x205f_ffff` is also rejected by the custom SVD generator.
`HP_SYS_CLKRST` ownership and field access are delegated entirely to the
official `esp-hal` PAC.

Within those windows the recovered SVD currently covers:

- Wi-Fi MAC and its integrated RX DMA/BlockAck register windows;
- PHY baseband, AGC, PBus, PHY-I2C command RAM and PHY table memories;
- `MODEM_SYSCON` clock/reset and PHY Wi-Fi/baseband control now use the same
  official-PAC ownership split;
- chip-level PHY analog-I2C, temperature and system clock/control peripherals
  are absent and delegated to the official PAC.

This is the current ownership boundary. The ESP32-S31 PAC used by the
`esp32s31-async-platform` HAL branch describes these complete chip-level
peripherals:

| Recovered identity | Official PAC identity | Base | Migration |
| --- | --- | --- | --- |
| `MODEM_SYSCON` | `esp32s31::MODEM_SYSCON` | `0x2010_9c00` | removed |
| `MODEM_LPCON` | `esp32s31::MODEM_LPCON` | `0x2010_f000` | removed |
| `PHY_I2C_MASTER` | `esp32s31::I2C_ANA_MST` | `0x2010_f800` | removed |
| `HP_SYS_CLKRST` | `esp32s31::HP_SYS_CLKRST` | `0x2058_7000` | removed |
| `PHY_POWER_DETECTOR_AUX_ORACLE` | `esp32s31::LP_AON_CLKRST` | `0x2070_1000` | removed |
| `PMU` | `esp32s31::PMU` | `0x2070_4000` | removed |
| `PHY_TEMPERATURE_SYSTEM_ORACLE` | `esp32s31::LP_PERICLKRST` | `0x2071_0000` | removed |
| `PHY_TEMPERATURE_SENSOR_ORACLE` | `esp32s31::LP_TSENS` | `0x2081_8000` | removed |

Those identities are now behind platform capabilities borrowed from the
official `esp-hal`/PAC owner and are absent from the recovered SVD, so two
Rust singleton types no longer claim the same physical MMIO. The custom PAC
remains necessary for the undocumented PHY/baseband aggregates, PHY command
RAM/deadline blocks and Wi-Fi MAC/RX-DMA registers that the official PAC does
not model. The private `RadioRegisters` singleton remains a serialization
mechanism for those custom radio blocks, not proof of exclusive chip-wide
ownership.

There is no raw access to the chip's general AXI-GDMA peripheral in the live
driver. Wi-Fi descriptors are owned SRAM objects shared with the Wi-Fi MAC
DMA engine; their volatile words are not peripheral MMIO.

## Remaining unsafe outside the PAC

Not all upper layers are safe yet:

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

The complete `open-esp-radio-phy-esp32s31` crate is now free of `unsafe`, raw
volatile access and pointer casts. Its target bindings express sequencing
through non-cloneable actions and require the unique `RadioRegisters` borrow;
hardware sequencing alone is not treated as a Rust memory-safety invariant.
The source-only audit rejects any return of `unsafe` to this upper PHY crate.

The power, PHY-I²C and PBus vertical slices are complete. PHY-I²C completion
observations now require `&mut RadioRegisters`; this exclusive borrow is
propagated through every nested cold-PHY binding rather than weakening the PAC
owner to a shared MMIO handle. Any remaining peripheral migration follows the
same rule: describe the register in the SVD, expose an ownership-bound PAC
operation and pass the existing `RadioRegisters` borrow into the safe PHY leaf.

The RX-gain DC calibration prefix/cleanup word at `0x2010_0424` is the first
baseband leaf moved directly to that final form without extending the
`Register32` compatibility facade. Its two legal field images and source
provenance live in the SVD; the generated writer is reachable only through the
unique `RadioRegisters` owner. The former derived raw address and upper-layer
`unsafe fn` are gone, and the source-only audit rejects both regressions.

SVD v2.6 moves the complete ADC-rate suffix, all seventeen ordered
front-end-initialization writes, and the three-write pinned front-end update
through the same native generated-PAC boundary. The front-end initializer is
split only around the already owned table-memory operation, preserving the
complete ROM order. Raw access to the six newly described unique words and
unsafe versions of all three upper wrappers are rejected by the audit.

SVD v2.7 closes the TX-DC measurement word. Trigger, one-shot ready sampling,
two independently sampled comparator bits and cleanup are generated-PAC
operations. PHY actions and completions now exchange only booleans, not the
physical address, masks or full register images; even the polling binding
requires the unique mutable register owner.

The same native PAC slice now owns TX-IQ/RX-IQ correction modes and signed
gain/phase coefficient publication. Signed coefficients retain the complete
ROM's six- or seven-bit truncation inside the PAC. Their bindings borrow
`RadioRegisters`; the unused physical-address constants and four upper
`unsafe` wrappers are removed.

SVD v2.8 closes the complete calibration-tone cluster at `0x040c`,
`0x0410..=0x0428` and `0x0c04`. Tone selectors, both packed path words,
TX-gain compensation, DAC scale, PWDET arm/stop and TX-IQ mismatch polarity
are now safe `RadioRegisters` operations. The SVD cites the exact rev0 ROM
leaves and `_oracles/libphy.a[phy_reg.o]` bodies; fields whose electrical
meaning is not independently established remain explicitly `UNKNOWN`.

The adjacent PWDET vertical slice has also left the `Register32` compatibility
facade. Initialization, three ordered enable-bit clears, background enable,
TX-DC field capture/restore, SAR mode/trigger, reference publication and
ready/result sampling are native generated-PAC operations. The HAL module is
now a thin coordinator for the official platform PAC mode selection; the
audit prevents compatibility register access from returning there.

The RX-DCO save/clear/restore word is native generated PAC as well. Capture
retains the complete source's two fresh reads, returns only the encoded
bits 23:22, and restore truncates its input back to that field. Its HAL module
is now a two-function semantic facade with no register compatibility types.

Descriptor-memory unsafe is a separate ownership problem and must not be
hidden inside the peripheral PAC.
