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

The PAC is not yet matched for every identity used by the current cold Wi-Fi
graph. Access widths, most masks and array bounds are sourced from the SVD,
and unit tests fix important command images and recovered geometries, but the
strict ROM PBus audit found one physical-address error.

It is not a complete ESP32-S31 PHY register specification. Names containing
`ORACLE`, `UNKNOWN` or `OPAQUE` deliberately preserve incomplete semantic
knowledge. A correct address and mask do not prove that every electrical
meaning of a field is known.

ROM `phy_pbus_rd_addr(0, path)` returns `0x201008a0` for both path classes.
The recovered SVD describes selector zero as a window of `0x201008a4`, and
PAC `read_pbus_result` follows that claim. Any Rust `phy_pbus_rd(0, path)`
equivalent therefore reads the wrong physical word. Selectors 1 through 5
match the complete ROM address/shift tables.

The TX-IQ coefficient PAC methods reproduce the vendor field masks but not
the complete leaf semantics. ROM `phy_txiq_set_reg` saturates gain to
`[-31,31]` and phase to `[-63,63]` before its RMW. PAC methods merely mask an
`i8`, so decoded extrema `-32` and `-64` publish different field images.

The strict `phy_rx_gain.o` audit found an upper-layer timing error, not a
PAC/HAL encoding error: the HAL pulse leaf corresponds to the vendor set and
clear RMWs, but `PhyRxGainPublishTransition` requests a `1 µs` second delay
where ROM `phy_pbus_force_mode(0)` requests `2 µs`.

The complete TX-calibration audit found the same upper-layer timing error in
two additional owners. `PhyTxCalibrationEnvironmentTransition` and the
TXDC/PWDET cleanup also request `1 µs` for the second pulse. Their PAC/HAL
set/clear operations remain correct; the PHY transitions supply the wrong
delay. The dedicated `phy_txdc` transition independently uses the correct
`2 µs`.

The `phy_rx_cal.o` audit found two further upper-layer uses of the same
otherwise-correct HAL contract. `PhyRxGainDcTransition` supplies `1 µs`
instead of `2 µs` for the second pulse. `PhyRxSaturationMmioBinding` calls
`configure_work_mode` but discards its returned baseband-enabled condition,
so it never schedules the conditional settle delay, pulse, second delay or
clear. These are PHY-owner defects; the HAL correctly reports whether the
tail is required.

## HAL inventory

There are 15 non-`lib.rs` HAL modules:

| HAL module | Vendor/ROM behaviour represented | Status |
| --- | --- | --- |
| `analog_i2c.rs` | `phy_open_i2c_xpd_new`, frontend/baseband power | Finite prefix/suffix represented; target platform trait implementation and ROM wait proof remain open |
| `pbus.rs` | PBus force commands, reads, modes and force-TX/RX phases | Force-TX/RX phases match; selector-zero read address and force-command start trace differ |
| `phy_agc.rs` | AGC init/update/enable, antenna, RX 11b | Matched for reached branches |
| `phy_baseband.rs` | baseband init/watchdog, tone and IQ controls | Matched for reached leaves |
| `phy_frequency.rs` | frequency reset/control, NRX, CBW, BT filter | Matched for reached leaves |
| `phy_i2c.rs` | master start/finish/reset and BBPLL controls | Scheduling-equivalent; see PHY-I2C finding |
| `phy_iq_estimator.rs` | estimator publish, ready and accumulator samples | Mismatch: completed readiness sample adds an activity-register read |
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
- PBus force-test start samples `BUSY` before publication, whereas ROM first
  publishes and only then samples it. This adds a register read even when the
  bus is ready and rejects a busy-at-entry state that ROM overwrites.
- the IQ-estimator readiness leaf always reads both ready and activity words.
  ROM reads activity only after a not-ready sample, so Rust adds one activity
  read on the final ready observation.
- readiness is sampled once per caller/executor edge rather than in a HAL spin
  loop;
- the prelude currently selects a fixed 40 MHz XTAL profile.

The baseband PAC exactly composes ROM `phy_dac_scale_set`,
`phy_stop_tx_tone` and the reached enabled first-path portion of
`phy_start_tx_tone_step`. It does not expose the complete dual-path or
zero/zero start trace. Its `update_front_end` and gain-compensation restore
methods intentionally follow the installed archive functions, which differ
from the same-named ROM versions: the ROM front-end leaf has a two-RMW DAC
tail and the ROM gain restore uses `[fd,f8,fd,fb]` rather than
`[00,fa,ff,00]`.

The AGC PAC matches both branches of ROM `phy_rfrx_sat_rst`, including the
full store to `0x20107068` and two distinct reads of `0x2010705c`. It has no
leaf for ROM `phy_force_rx_gain_trig`; generic register identities at
`0x20100884` and `0x2010702c` do not by themselves implement its conditional
high-byte write and delayed bit-23 pulse.

The adjacent leaf audit also closes AGC antenna initialization, saturation
gain publication, automatic noise-floor control and IQ-correction enable as
exact register traces. RXIQ gain/phase publication is correctly saturated by
the owning PHY transition before the PAC RMW; this differs from the defective
TXIQ extrema path.

BBPLL calibration remains a lower-layer integration proof gap. The HAL
preserves the zero/nonzero encodings but delegates the actual RMW of
`0x2010f818[3:2]` to `PhyI2cMasterControl`, whose target implementation is
outside this repository. ROM `phy_bbpll_recal` additionally requires a
discarded fresh read between its mode-two write and the mode-one child's
fresh read; no Rust leaf composes that exact trace.

The same external-backend limitation applies to three more ROM leaves:
digital BSS bandwidth, PHY-I2C master register initialization and MAC
baseband enable. Their HAL calls preserve the recovered logical values and
operation count, but `PhyWifiBbControl`/`PhyI2cMasterControl` implementations
are required before physical fresh-RMW parity can be claimed.

Direct PAC leaves exactly match ROM AGC enable/disable and baseband watchdog
configuration. CCA enable/disable, general RX-filter mode and the FE TX/RX
reset pulse have no PHY-register implementation. The RX compensation PAC
uses `0xed`, correctly matching the installed archive `_new` function but
not the older ROM leaf's `0xeb`.

Wi-Fi FBW selection and BT-filter setup are also exact three-RMW PAC leaves.
The NRX helper preserves the ROM's two separate reads and final full-word
write, but not its arithmetic domain: ROM uses signed RV32 division by a
full word, including architectural division-by-zero behaviour; Rust asserts
on zero, accepts `u16`, and divides unsigned.

The remaining adjacent force/state leaves are absent: RX sense, TX state,
PA close/open, PBus-register restore, RIFS mode, FE ADC sequencing, forced
power index, FFT scale/force and forced RX gain. Generic identities in the
recovered PAC do not count as implementing their ordered RMW traces.

Channel-CBW configuration retains four distinct PAC RMWs and matches byte
inputs, but the ROM standalone full-word high path can OR normalized bits
outside the nominal first nibble. Rust narrows to `u8` and a four-bit field.
Wi-Fi enable has the correct Boolean platform operation but remains open on
the external physical backend.

DC-memory clear and I²C TX-rate initialization are exact. The latter resolves
its callback-table slot to the already matched archive gain-compensation
replacement. Baseband-watchdog initialization covers only the set branch of
ROM `phy_bb_wdt_rst_enable`; its clear branch, interrupt control,
timeout-clear and status-read leaves are not exposed.

These are documented in the parity findings because their observable result
depends on the caller profile or abnormal hardware state.
