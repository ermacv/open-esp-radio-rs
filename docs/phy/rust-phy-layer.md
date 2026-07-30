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
| `phy_channel.rs` | `phy_chip_set_chan`, `phy_chip_set_chan_misc_new`, Wi-Fi TX-gain calculation/publication | Profile-matched for channel 1–13, CBW 0, frequency offset 0, 11p disabled and channel-14 MIC feature disabled |
| `phy_i2c.rs` | target I2C callbacks, `phy_open_i2c_xpd_new`, `phy_i2c_init1`, command RAM, RC/SAR/filter/RF init graph | Scheduling-equivalent on an uncontended ready host |
| `phy_pbus.rs` | `phy_pbus_clear_reg`, force-test/work-mode graph | Scheduling-equivalent on a ready PBus |
| `phy_pbus_memory.rs` | `phy_set_pbus_mem`, `phy_write_pbus_mem`, `phy_save_pbus_reg` | Matched, 60 entries and six saved words |
| `phy_frequency.rs` | `phy_get_rf_freq_init`, 85 frequency records and I2C copies | Profile-matched for count 85, offset 0, write input 1 and `phy_param[0x1af]` in `0..=1`; general vendor inputs differ |
| `phy_rfpll.rs` | RFPLL frequency, SDM, lock and capacitor-search graph | Successful path matched; finite error replaces vendor nontermination |
| `phy_dc_iq.rs` | `phy_iq_est_enable`, `phy_iq_est_disable`, `phy_dc_iq_est`, `phy_linear_to_db` | Scheduling-equivalent |
| `phy_dcode.rs` | `phy_dcode_cal_init` | Matched for valid six-bit reads |
| `phy_pwdet.rs` | `phy_pwdet_code_cal` and SAR/PBus children | Scheduling-equivalent |
| `phy_rx_dco.rs` | `phy_pbus_rx_dco_cal`, `phy_rxdc_est_min` | Matched reachable bounded graph |
| `phy_rx_gain_cal.rs` | `phy_pbus_rx_dco_cal_1step`, RX gain DC/IQ calibration primitives | Matched for cold caller argument profiles |
| `phy_rx_gain.rs` | `phy_gen_rx_gain_table`, `phy_wr_rx_gain_mem_new`, RX table parent inputs | Matched for Wi-Fi/shared cold banks |
| `phy_rx_saturation.rs` | `phy_check_rx_sat` | Matched 11-command, delay and 100-sample policy |
| `phy_rxiq.rs` | `phy_rxiq_cal_init` and reachable RX-IQ ROM graph | Matched for diagnostic mode zero |
| `phy_signal_power.rs` | `phy_get_rx_sig_pwr` | Scheduling-equivalent |
| `phy_temperature.rs` | `phy_tsens_temp_read` and conversion helpers | Profile-matched; invalid/reset DAC handling deliberately differs |
| `phy_tx_cal.rs` | shared TX-cap, attenuation, tone and SAR primitives | Matched for reached cold profiles |
| `phy_tx_power.rs` | `phy_tx_pwctrl_init`, `phy_tx_pwctrl_init_cal_new`, `phy_rfcal_pwrctrl` | Matched for Wi-Fi cold calibration |
| `phy_txdc.rs` | `phy_txdc_cal_init`, ROM `phy_txdc_cal` | Scheduling-equivalent |
| `phy_txdc_pwdet.rs` | `phy_txdc_cal_pwdet_init`, `phy_txdc_cal_pwdet_new` | Matched finite scans |
| `phy_txiq.rs` | `phy_txiq_cal_init`, `phy_rfcal_txiq` graph | Matched for cold caller profiles |
| `phy_xtal_duty.rs` | `phy_xtal_duty_cal_init`, `phy_xtal_duty_cal` | Matched for debug zero |
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
- debug/check/print functions;
- several public API wrappers and feature controls such as watchdog, CCA,
  sensitivity, low-rate and tone/debug entry points.

Some underlying ROM leaves are already represented because the Wi-Fi cold
graph shares them. That does not make the corresponding vendor parent or
lifecycle capability ported.

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
