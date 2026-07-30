# ESP32-S31 PHY PAC and HAL layers

This page records the lower two Rust layers used by the source-only PHY. They
do not correspond one-for-one to vendor functions: the PAC owns identities,
while the HAL owns finite transactions.

## Recovered SVD and PAC

The recovered SVD contains 54 radio peripherals in total. Ten are the current
PHY set, containing 123 register declarations and 228 explicitly represented
field declarations. Dimensioned arrays, such as the 45-entry I2C command RAM,
are counted once in these declaration totals:

| SVD peripheral | Register declarations | Field declarations | Principal evidence/use |
| --- | ---: | ---: | --- |
| `PHY_MEMORY` | 5 | 12 | PBus, CFR and gain-memory publication |
| `PHY_FREQUENCY_CHANNEL_ORACLE` | 11 | 38 | frequency table, channel switch, CBW and NRX |
| `PHY_PBUS` | 8 | 22 | PBus command, mode, busy and result windows |
| `PHY_I2C_COMMAND_RAM` | 1 array | 3 | 45 three-byte command-memory entries |
| `PHY_AGC_ORACLE` | 32 | 37 | AGC init/update, antenna, RX gain and saturation |
| `PHY_BASEBAND_CONFIG_ORACLE` | 49 | 104 | baseband init, IQ, TX/RX calibration and PWDET |
| `PHY_IQ_ESTIMATOR_ORACLE` | 11 | 7 | estimator control, ready and accumulators |
| `PHY_RX_DCO_ORACLE` | 1 | 1 | saved RX-DCO control field |
| `PHY_COLD_DEADLINE_ORACLE` | 1 | 1 | wrapping counter used by the SDM deadline |
| `PHY_CLOCK_ORACLE` | 4 | 3 | calibration clock and related recovered identities |

The source-facing PAC modules are `agc.rs`, `baseband.rs`, `clock.rs`,
`frequency.rs`, `iq_estimator.rs`, `pbus.rs`, `phy.rs`, `phy_i2c.rs`,
`table_memory.rs` and the shrinking compatibility facade `power.rs`.

### PAC assessment

The PAC is **matched for the identities used by the current cold Wi-Fi graph**:
access widths, masks and array bounds are sourced from the SVD, and the unit
tests fix important command images and recovered geometries.

It is not a complete ESP32-S31 PHY register specification. Names containing
`ORACLE`, `UNKNOWN` or `OPAQUE` deliberately preserve incomplete semantic
knowledge. A correct address and mask do not prove that every electrical
meaning of a field is known.

No PAC-level behavioural defect was found in the audited current graph.
Remaining risk is coverage: vendor functions outside the cold Wi-Fi graph may
touch registers or fields that are not yet represented.

## HAL inventory

There are 15 non-`lib.rs` HAL modules:

| HAL module | Vendor/ROM behaviour represented | Status |
| --- | --- | --- |
| `analog_i2c.rs` | `phy_open_i2c_xpd_new`, frontend/baseband power | Matched finite prefix/suffix |
| `pbus.rs` | force-TX/RX phases | Matched for four recovered encodings |
| `phy_agc.rs` | AGC init/update/enable, antenna, RX 11b | Matched for reached branches |
| `phy_baseband.rs` | baseband init/watchdog, tone and IQ controls | Matched for reached leaves |
| `phy_frequency.rs` | frequency reset/control, NRX, CBW, BT filter | Matched for reached leaves |
| `phy_i2c.rs` | master start/finish/reset and BBPLL controls | Scheduling-equivalent; see PHY-I2C finding |
| `phy_iq_estimator.rs` | estimator publish, ready and accumulator samples | Scheduling-equivalent |
| `phy_memory.rs` | PBus/CFR/gain-memory writes | Matched finite transactions |
| `phy_power_detector.rs` | detector configuration and SAR controls | Matched for reached leaves |
| `phy_prelude.rs` | fixed 40 MHz XTAL setup and deadline sample | Profile-matched |
| `phy_rx_dco.rs` | capture/clear/restore RX-DCO field | Matched |
| `phy_temperature.rs` | sensor setup and code sample | Matched for valid sensor state |
| `power.rs` | modem/PHY clock and reset prerequisites | Platform prerequisite, not a `libphy.a` one-to-one port |
| `power_detector_platform.rs` | target-specific PWDET platform operations | Matched for recovered encodings |
| `wifi_bb.rs` | Wi-Fi/baseband enable and AGC update platform controls | Matched for reached encodings |

`Radio<P, state>` in `lib.rs` adds unique ownership and type-state. This changes
the software API but not the intended register result. The platform power-up
sequence is a prerequisite outside `register_chipv7_phy`, so it must not be
counted as a vendor PHY child.

### HAL findings

The successful, uncontended hardware path is consistent with the recovered
vendor leaves. The following differences are owned by the PHY layer and are
not hidden here:

- PHY-I2C start rejects an already-busy host before publishing a command,
  whereas the ROM read leaf publishes without that check; the ROM write leaf
  does wait for idle but has no deadline.
- readiness is sampled once per caller/executor edge rather than in a HAL spin
  loop;
- the prelude currently selects a fixed 40 MHz XTAL profile.

These are documented in the parity findings because their observable result
depends on the caller profile or abnormal hardware state.
