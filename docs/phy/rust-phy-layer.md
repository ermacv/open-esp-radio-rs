# ESP32-S31 Rust PHY functional inventory

The Rust PHY crate has 26 `phy_*.rs` modules. `executor.rs` drives the top-level
transition, and `radio_hal.rs` is a temporary collection of finite MMIO leaves.
The table maps functional owners to vendor roots; action, completion, binding
and helper methods are not counted as independent vendor functions.

| Rust module | Principal vendor roots or graph | Current status |
| --- | --- | --- |
| `phy_register.rs` | `register_chipv7_phy` | Partial: full-calibration/40 MHz/default profile only |
| `phy_cold.rs` | owned `phy_param`, `phy_rf_init` composition and calibration-record transforms | Profile-matched; record transforms are not composed into the top-level lifecycle |
| `phy_bb.rs` | `phy_bb_init`, `phy_set_tx_cfr_mem`, RX table generation/publication | Partial: mandatory six-stage vendor `phy_bt_tx_gain_init` child is skipped |
| `phy_channel.rs` | `phy_chip_set_chan`, `phy_chip_set_chan_misc_new`, Wi-Fi TX-gain calculation/publication | Wi-Fi TX-gain arithmetic and 32-entry image match; enclosing channel/profile domain is narrower |
| `phy_i2c.rs` | target I2C callbacks, `phy_open_i2c_xpd_new`, `phy_i2c_init1`, command RAM, RC/SAR/filter/RF init graph | Profile-matched on an uncontended ready host; `phy_i2c_init2`, five low-ID masks and the target `PhyI2cMasterControl` implementation are absent |
| `phy_pbus.rs` | `phy_pbus_clear_reg`, force-test/work-mode graph | Mismatch: every force command adds a pre-publication busy read; work-mode tails are not uniformly composed |
| `phy_pbus_memory.rs` | `phy_set_pbus_mem`, `phy_write_pbus_mem`, `phy_save_pbus_reg` | Matched, 60 entries and six saved words |
| `phy_frequency.rs` | `phy_get_rf_freq_init`, 85 frequency records and I2C copies | Profile-matched for count 85, offset 0, write input 1 and `phy_param[0x1af]` in `0..=1`; general vendor inputs differ |
| `phy_rfpll.rs` | RFPLL frequency, SDM, lock and capacitor-search graph | Successful path matched; finite error replaces vendor nontermination |
| `phy_dc_iq.rs` | `phy_iq_est_enable`, `phy_iq_est_disable`, `phy_dc_iq_est`, `phy_linear_to_db` | Mismatch: final ready sample adds an activity read; DC/IQ control/divisor domain is narrowed |
| `phy_dcode.rs` | `phy_dcode_cal_init` | Matched for valid six-bit reads |
| `phy_pwdet.rs` | `phy_pwdet_code_cal`, ROM tone start/stop and SAR/PBus children | Reached first-path tone profile is represented; complete dual-path/zero-enable ROM tone function is not |
| `phy_rx_dco.rs` | `phy_pbus_rx_dco_cal`, `phy_rxdc_est_min` | Direct bounded algorithms match the fixed profile, but lower PBus/estimator traces and general inputs differ |
| `phy_rx_gain_cal.rs` | `phy_pbus_rx_dco_cal_1step_new`, RX gain DC calibration | Cold nine-bit inputs represented; general signed-halfword domain differs and composed cleanup pulse is 1 µs instead of 2 µs |
| `phy_rx_gain.rs` | `phy_gen_rx_gain_table`, `phy_wr_rx_gain_mem_new`, RX table parent inputs | Mismatch: second PBus pulse delay is 1 µs instead of 2 µs; complete vendor count/bank domain is narrowed |
| `phy_rx_saturation.rs` | `phy_check_rx_sat` | Eleven-command setup, delay and 100 samples match; conditional PBus work-mode pulse is omitted |
| `phy_rxiq.rs` | `phy_rxiq_cal_init` and reachable RX-IQ ROM graph | Cold profile represented; RXIQ coefficient saturation matches, but root argument branches and inherited estimator transactions differ |
| `phy_signal_power.rs` | `phy_get_rx_sig_pwr` | Scheduling-equivalent |
| `phy_temperature.rs` | `phy_tsens_temp_read` and conversion helpers | Profile-matched; invalid/reset DAC handling deliberately differs |
| `phy_tx_cal.rs` | shared TX-cap, attenuation, tone and SAR primitives | Mismatch: shared work-mode pulse is 1 µs instead of vendor 2 µs |
| `phy_tx_power.rs` | `phy_tx_pwctrl_init`, `phy_tx_pwctrl_init_cal_new`, `phy_rfcal_pwrctrl` | Mismatch: Wi-Fi calculation is represented, but cleanup timing differs and BT mode is absent |
| `phy_txdc.rs` | `phy_txdc_cal_init`, ROM `phy_txdc_cal` | Scheduling-equivalent |
| `phy_txdc_pwdet.rs` | `phy_txdc_cal_pwdet_init`, `phy_txdc_cal_pwdet_new` | Search matches; parent cleanup pulse is 1 µs instead of 2 µs and other input branches are absent |
| `phy_txiq.rs` | `phy_txiq_cal_init`, `phy_rfcal_txiq` graph | Reached graph inherits the shared cleanup-timing mismatch |
| `phy_xtal_duty.rs` | `phy_xtal_duty_cal_init`, `phy_xtal_duty_cal` | Direct bodies match for register behaviour; strict ROM child proofs remain open |
| `phy_param.rs` | init-data transform, calibration record/checksum, RC transform | Pure transforms matched; not all lifecycle modes are wired |

## Top-level Rust entry points

The source-only public entry is `run_phy_register`, which drives
`PhyRegisterTransition`. The default constructor fixes the production init
profile and channel 11. `with_default_profile_on_channel` is an open-driver
extension and is not a direct vendor `register_chipv7_phy` input.

The Rust parent owns the 508-byte parameter image and passes typed snapshots to
children. This intentionally removes the vendor `phy_param` pointer cell and
`g_phyFuns` callback table. Ownership is an API difference, but the audit
compares the resulting parameter bytes and radio transactions rather than ABI
plumbing.

The strict `phy_rx_gain.o` audit found two parameter-owner defects that are not
API-only differences:

- `prepare_rx_table_init` implements the vendor halfword store of `0x4f4f` at
  offset `0x120` as a single-byte store, leaving `parameter[0x121]` stale;
- RX-gain table guard capture and commit use offset `0xb4`, while the vendor
  reads and updates the word at offset `0xa4`.

Both defects can change subsequent AGC register values or guarded hardware
paths even when all typed transitions complete successfully.

The strict `phy_rx_cal.o` audit found two more reached cleanup defects:

- `PhyRxGainDcTransition` uses one microsecond for the second conditional
  PBus work-mode pulse, while the vendor uses two;
- `PhyRxSaturationMmioBinding` discards the baseband-enabled condition returned
  by `configure_work_mode` and therefore omits the complete conditional
  settle/pulse tail.

It also closes two standalone-domain differences. The RX-DC one-step vendor
search sign-extends its caller halfwords before correction, while Rust treats
them as unsigned; and the RX-IQ gain owner implements only the zero first
input and selector `0x80`, omitting the vendor's temporary I2C branch and
general selector input.

The strict ROM PBus audit found a lower-layer address defect and a transaction
ordering difference. For selector zero, ROM `phy_pbus_rd` reads
`0x201008a0`, while the recovered SVD/PAC and Rust HAL read `0x201008a4`.
Every Rust PBus force command also samples `BUSY` before publication; ROM
publishes first and begins sampling only afterwards. The bounded async wait is
an intentional replacement for vendor nontermination, but the additional
successful-path register read is not transaction-equivalent.

## Covered cold Wi-Fi sequence

For the default full-calibration profile, the Rust graph contains:

1. the `register_chipv7_phy` radio prelude;
2. the reached `phy_rf_init` graph;
3. all Wi-Fi calibration children of `phy_bb_init`;
4. TX CFR, PBus memory, RX-IQ, RX table, register and AGC work;
5. channel-11 setup;
6. the parent temperature/BBPLL/final-I2C and release tail.

The sequence is not fully vendor-equivalent because `phy_bb_init` also invokes
`phy_bt_tx_gain_init`, which the Rust parent deliberately skips today.

## Vendor capabilities outside the current Rust PHY graph

The largest unported groups are:

- wake/sleep/close lifecycle: `phy_wakeup_init`, `phy_close_rf`,
  `phy_close_fe_bb_clk`, `phy_xpd_rf_new`;
- runtime/background tracking: `phy_param_track*`, `phy_cal_param_track`,
  `phy_txpwr_cal_track_new`, Wi-Fi/BT tracking and background control;
- BT/BLE calibration and gain publication, including `phy_bt_tx_gain_init`,
  BT TX power, the 85-entry `phy_bt_txpwr_freq` memory publication,
  TXDC/TXIQ and channel interpolation;
- channel 14 MIC handling and 802.11p configuration;
- general dual-path tone generation, archive tone-stop, ICCFR and HCCFR
  controls;
- the conditional ROM `phy_force_rx_gain_trig` high-byte update and delayed
  AGC pulse;
- debug/check/print functions;
- several public API wrappers and feature controls such as watchdog, CCA,
  sensitivity, low-rate and tone/debug entry points.

Some underlying ROM leaves are already represented because the Wi-Fi cold
graph shares them. That does not make the corresponding vendor parent or
lifecycle capability ported.

The complete `phy_debug.o` audit closes that member without finding a hidden
Rust equivalent. In particular, the absent diagnostic surface includes a
1933-load dump of 21 direct-MMIO ranges, a 168-read dump of ten PHY-I2C banks,
an eleven-read PBus selector sequence, the PBus/I2C/SAR VDD33 measurement, and
a channel 1-through-14 RFPLL sweep that leaves channel 14 active. The pure
version, IQ-unpack and TX-gain formatting helpers have no register effect.

The complete `phy_track.o` audit also confirms that hardware background-power
enablement is not a replacement for the vendor software tracking layer. All
nine parents are absent: the vendor applies four temperature-dependent TXRF
I²C profiles, brackets Wi-Fi/BT gain regeneration with BBPLL calibration, and
can rerun RX/dcode and TXDC/PWDET calibration before restoring the channel.
The exact gates and ordering are recorded in
[the tracking audit](audit/libphy-phy_track.md).

The strict `phy_tx_gain.o` audit confirms that the Wi-Fi table constants,
packing, `phy_write_gain_mem` command and cold 32-entry CFR trace match.
Those are profile results: the generic archive publishers accept other finite
counts and a BT bank/table contract, while Rust fixes both publishers to the
32-entry Wi-Fi cold image.

The strict adjacent ROM tone audit distinguishes the ROM leaves from their
archive replacements. The reached nonzero first-path power-detector tone,
unconditional ROM tone stop and both `phy_rfrx_sat_rst` branches match. The
complete ROM tone-start function still accepts a second path and a zero/zero
restore branch that Rust cannot reproduce exactly. The ROM
`phy_fe_reg_update` additionally restores both DAC-scale bytes, while the
same-named archive function and its correct Rust implementation do not.
Likewise, ROM `phy_txgain_comp_pacfg_` restores bytes
`[fd,f8,fd,fb]`, whereas the installed archive function and Rust restore
`[00,fa,ff,00]`.

The RXIQ/noise-floor leaf audit confirms that the Rust RXIQ owner does clamp
gain and phase exactly before their register RMWs; the packed-extrema defect
is limited to the separate TXIQ path. Automatic noise-floor setup,
IQ-correction enable, AGC saturation-gain publication and antenna
initialization are exact. The six-input watchdog test function and contiguous
ROM BBPLL-recalibration operation are not ported, while the basic BBPLL mode
leaf remains open on the external platform-trait implementation.

The AGC/CCA/channel leaf audit closes AGC enable/disable and watchdog setup as
exact. It also identifies missing CCA controls, RX-filter mode, BT gain
offset and the FE TX/RX reset pulse. The reached byte-truncated
`phy_mac_tx_chan_offset` profile matches, but the Rust `u8` helper does not
implement the standalone ROM full-word domain. Digital BSS, MAC baseband and
PHY-I2C master-register operations retain open target-backend proofs.

The FBW/force-control audit closes Wi-Fi FBW and BT-filter setup as exact,
but finds that `configure_nrx_frequency` is only profile-equivalent: the
register sampling order is correct, while signed full-word RV32 division was
replaced by asserted nonzero `u16` unsigned division. The adjacent RX-sense,
TX-state, PA, RIFS, FE-ADC and force-control capabilities are absent. Rust
also implements only the six-word PBus save direction, not ROM
`phy_set_pbus_reg` restoration.

The CBW/feature/watchdog audit finds another general-domain narrowing:
`configure_channel_cbw(u8)` matches every byte image but not the standalone
ROM word domain. DC-memory clear and I²C TX-rate initialization match,
including the archive gain-restore callback. Wi-Fi enable remains dependent
on the external platform backend, and VHT/CSI/HE PHY controls, SIFS, TX
outfilter plus most standalone watchdog controls are absent.

The pure power/mapping audit confirms that the ROM target-power wrappers,
FCC limiter, temperature correction and Wi-Fi/BT baseband-gain conversions
contain no hidden MMIO. Rust `PhyTxTargetPowerProfile` owns the reached
18-byte Wi-Fi target inputs; its invalid-rate safety policy belongs to the
parent input-domain comparison rather than to a missing register leaf.

### Exact cold-init BT boundary

The missing `phy_bt_tx_gain_init` parent reaches existing Rust-owned classes of
operation—RFPLL frequency setup, TX-cap selection, generic TXDC calibration,
TXDC/PWDET calibration and gain-memory publication. What is absent is the
BT-specific composition and state:

- 2437 MHz calibration profile and BT TX-cap source;
- three-point BT TXDC calibration with the vendor cache flag;
- BT power-control calibration with its separate cache flag;
- target-provided BT table generation;
- publication of 16 calibrated entries to BT gain-memory bank 1.

Consequently this gap must be closed as an owned BT transition, not by deleting
the parent call as irrelevant to Wi-Fi.
