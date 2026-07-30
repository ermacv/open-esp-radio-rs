# ESP32-S31 vendor PHY function inventory

This is the symbol-level inventory for the pinned oracle artifacts. It records
what exists in the vendor implementation; functional parity is assessed in
[behavior-parity.md](behavior-parity.md).

## `_oracles/libphy.a`

SHA-256:
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The archive has 21 members. Fifteen define 161 external code symbols. The six
members with no externally visible code symbol are `phy_tester_cali.o`,
`phy_pbus.o`, `phy_11ax.o`, `phy_rom.o`, `phy_analog_cal.o` and `phy_pwdet.o`.

| Member | External code functions |
| --- | ---: |
| `phy_api.o` | 36 |
| `phy_basic.o` | 1 |
| `phy_debug.o` | 12 |
| `phy_feature.o` | 8 |
| `phy_hw_freq.o` | 7 |
| `phy_i2c.o` | 11 |
| `phy_init.o` | 19 |
| `phy_reg.o` | 12 |
| `phy_rfpll.o` | 4 |
| `phy_rx_cal.o` | 13 |
| `phy_rx_gain.o` | 6 |
| `phy_track.o` | 9 |
| `phy_tsens.o` | 5 |
| `phy_tx_cal.o` | 10 |
| `phy_tx_gain.o` | 8 |
| **Total** | **161** |

### `phy_api.o` — 36

`RFChannelSel`, `ant_btrx_cfg`, `ant_bttx_cfg`, `ant_dft_cfg`, `ant_rx_cfg`,
`ant_tx_cfg`, `ant_wifirx_cfg`, `ant_wifitx_cfg`, `bb_wdt_get_status`,
`bb_wdt_int_enable`, `bb_wdt_rst_enable`, `bb_wdt_timeout_clear`,
`bt_track_pll_cap`, `esp_tx_state_out`, `noise_check_loop`,
`phy_bbpll_en_usb`, `phy_ble_set_chan_base`, `phy_bt_power_track`,
`phy_change_channel`, `phy_current_level_set`, `phy_get_rf_cal_version`,
`phy_get_rfdata_num`, `phy_init_param_set`, `phy_pwdet_always_en`,
`phy_pwdet_onetime_en`, `phy_rx_rifs_en`, `phy_set_chan_misc`,
`phy_track_temp_debug`, `phy_xpd_tsens`, `read_hw_noisefloor`,
`rx_gain_force`, `set_bb_wdg`, `set_cca`, `set_rx_sense`,
`tx_pwctrl_background`, `tx_state_set`.

### `phy_basic.o` — 1

`phy_chan14_mic_cfg_new`.

### `phy_debug.o` — 12

`get_bias_ref_code`, `get_dc_value`, `get_phy_version_str`, `phy_cal_print`,
`phy_debug_print_line`, `phy_get_vdd33`, `phy_i2c_check`, `phy_pbus_print`,
`phy_reg_check`, `phy_tx_gain_print`, `phy_version_print`,
`rfpll_cap_check`.

### `phy_feature.o` — 8

`phy_11p_set`, `phy_freq_mem_backup`, `phy_ftm_comp`, `phy_get_adc_rand`,
`phy_get_rx_freq`, `phy_internal_delay`, `phy_set_most_tpw_new`,
`phy_set_rate`.

### `phy_hw_freq.o` — 7

`phy_bt_txpwr_freq`, `phy_freq_get_i2c_data`, `phy_freq_i2c_data_write`,
`phy_freq_offset_set`, `phy_get_rf_freq_cap`, `phy_get_rf_freq_init`,
`phy_set_chan_freq_hw_init`.

### `phy_i2c.o` — 11

`phy_bias_reg_set`, `phy_get_i2c_data`, `phy_get_i2c_hostid_new`,
`phy_get_i2c_read_mask_new`, `phy_i2c_enter_critical`,
`phy_i2c_exit_critical`, `phy_i2c_init1`, `phy_i2c_init2`,
`phy_i2c_master_cmd_mem_init`, `phy_i2c_master_command_mem_cfg`,
`phy_i2c_master_mem_cfg`.

### `phy_init.o` — 19

`phy_bb_init`, `phy_close_fe_bb_clk`, `phy_close_rf`,
`phy_get_chip_version`, `phy_get_rom_ver`, `phy_get_romfunc_addr`,
`phy_get_xtal_freq`, `phy_i2c_read_check`, `phy_rc_cal_init`,
`phy_reg_update_new`, `phy_rf_cal_data_backup_new`,
`phy_rf_cal_data_recovery_new`, `phy_rf_init`, `phy_rfcal_data_check_new`,
`phy_rfcal_data_sub_new`, `phy_wakeup_init`, `phy_xpd_rf_new`,
`register_chipv7_phy`, `register_chipv7_phy_init_param`.

### `phy_reg.o` — 12

`phy_bb_txpwr_track`, `phy_config_hccfr`, `phy_dc_mem_clr`,
`phy_fe_reg_update`, `phy_force_iccfr`, `phy_iccfr_en`,
`phy_open_i2c_xpd_new`, `phy_set_ftm_en`, `phy_set_rx_comp_new`,
`phy_start_tx_tone_step_new`, `phy_stop_tx_tone_new`,
`phy_txgain_comp_pacfg_new`.

### `phy_rfpll.o` — 4

`phy_chip_set_chan`, `phy_chip_set_chan_misc_new`,
`phy_chip_set_chan_offset`, `phy_set_chanfreq`.

### `phy_rx_cal.o` — 13

`phy_bt_rx_mx_dgain`, `phy_check_rx_sat`, `phy_get_xtal_duty`,
`phy_pbus_rx_dco_cal_1step_new`, `phy_rfrx_gain_index_new`,
`phy_rxdc_est_delta`, `phy_rxdc_fine_delta`, `phy_set_lb_txiq_new`,
`phy_set_rx_gain_cal_dc_new`, `phy_set_rx_gain_cal_iq_new`,
`phy_xtal_duty_cal`, `phy_xtal_duty_cal_init`, `phy_xtal_duty_set`.

### `phy_rx_gain.o` — 6

`phy_get_rxbb_dc_new`, `phy_rx_table_init`, `phy_rx_table_track`,
`phy_rxiq_cal_init`, `phy_set_rx_gain_table`, `phy_wr_rx_gain_mem_new`.

### `phy_track.o` — 9

`phy_bt_track_pll_cap`, `phy_bt_track_tx_power_new`,
`phy_cal_param_track`, `phy_param_track`, `phy_param_track_tot`,
`phy_tx_i2c_track`, `phy_tx_pwctrl_background`,
`phy_txpwr_cal_track_new`, `phy_wifi_track_tx_power_new`.

### `phy_tsens.o` — 5

`phy_get_temp_init`, `phy_get_tsens_value`, `phy_set_tsens_power`,
`phy_set_tsens_range`, `phy_tsens_read_init`.

### `phy_tx_cal.o` — 10

`phy_bt_tx_pwctrl_init`, `phy_bt_txdc_cal_new`, `phy_tx_atten_comp`,
`phy_tx_cap_init`, `phy_tx_pwctrl_init`, `phy_tx_pwctrl_init_cal_new`,
`phy_txdc_cal_init`, `phy_txdc_cal_pwdet_init`,
`phy_txdc_cal_pwdet_new`, `phy_txiq_cal_init`.

### `phy_tx_gain.o` — 8

`phy_bt_chan_pwr_interp`, `phy_bt_get_tx_tab_new`,
`phy_bt_set_tx_gain_new`, `phy_bt_tx_gain_init`, `phy_set_tx_cfr_mem`,
`phy_set_tx_gain_mem_new`, `phy_wifi_get_tx_tab_new`,
`phy_wifi_set_tx_gain_new`.

`phy_bt_tx_gain_init` is a 90-byte mandatory child of cold `phy_bb_init`. Its
relocations establish the ordered calls to RFPLL frequency setup, TX-cap
selection, BT TXDC calibration, BT power-control calibration, TXDC/PWDET
calibration and BT gain publication. The detailed Rust coverage boundary is in
[behavior-parity.md](behavior-parity.md#phy-parity-001-unconditional-bt-gain-child-is-absent).

## `_oracles/esp32s31_rev0_rom.elf`

SHA-256:
`d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542`.

The canonical 513,396-byte ELF cited throughout the SVD has SHA-256
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`;
the `_oracles` copy is a 516,460-byte container. Their `.fixed.text`,
`.init.text`, `.text`, `.rodata` and `.rodata.interface` section images are
byte-identical. Function addresses, sizes and the 305-symbol inventory are
also identical. The local hash above describes the file actually inventoried,
while the canonical hash remains the provenance identity used by the SVD.

The ELF defines 305 external `phy_*` code symbols. Data objects such as
`phy_param_rom` and `phy_tsens_attribute` are not included in this function
count.

### A–C

`phy_abs_temp`, `phy_adc_rate_set`, `phy_agc_reg_init`, `phy_ant_btrx_cfg`,
`phy_ant_bttx_cfg`, `phy_ant_dft_cfg`, `phy_ant_init`,
`phy_ant_wifirx_cfg`, `phy_ant_wifitx_cfg`, `phy_bb_agc_reg_update`,
`phy_bb_bss_cbw40`, `phy_bb_bss_cbw40_dig`, `phy_bb_cbw_chan_cfg`,
`phy_bb_cfo_cfg`, `phy_bb_dcmem_clr`, `phy_bb_gain_index`,
`phy_bb_reg_init`, `phy_bb_wdg_cfg`, `phy_bb_wdg_test_en`,
`phy_bb_wdt_get_status`, `phy_bb_wdt_int_enable`, `phy_bb_wdt_rst_enable`,
`phy_bb_wdt_timeout_clear`, `phy_bbpll_cal`, `phy_bbpll_recal`,
`phy_bbtx_outfilter`, `phy_bt_bb_to_index`, `phy_bt_filter_reg`,
`phy_bt_gain_offset`, `phy_bt_get_tx_gain`, `phy_bt_get_tx_tab_`,
`phy_bt_index_to_bb`, `phy_bt_set_tx_gain`, `phy_bt_track_tx_power`,
`phy_bt_txdc_cal`, `phy_bt_txiq_cal`, `phy_btbb_wifi_bb_cfg2`,
`phy_byte_to_word`, `phy_chan14_mic_cfg`, `phy_chan14_mic_enable`,
`phy_chan_dump_cfg`, `phy_chan_filt_set`, `phy_chan_to_freq`,
`phy_chip_i2c_readReg`, `phy_chip_i2c_readReg_org`,
`phy_chip_i2c_writeReg`, `phy_chip_set_chan_ana`,
`phy_chip_set_chan_misc`, `phy_close_pa`, `phy_code_to_temp`,
`phy_csidump_force_lltf_cfg`.

### D–F

`phy_dac_rate_set`, `phy_dac_scale_set`, `phy_dc_iq_est`,
`phy_dcode_cal_init`, `phy_dig_gain_check`, `phy_dig_reg_backup`,
`phy_dis_hw_set_freq`, `phy_disable_agc`, `phy_disable_cca`,
`phy_disable_low_rate`, `phy_en_hw_set_freq`, `phy_en_pwdet`,
`phy_enable_agc`, `phy_enable_cca`, `phy_enable_low_rate`,
`phy_encode_i2c_master`, `phy_fe_adc_on`, `phy_fe_reg_init`,
`phy_fe_reg_update`, `phy_fe_txrx_reset`, `phy_fft_scale_force`,
`phy_filter_dcap_set`, `phy_force_pwr_index`, `phy_force_rx_gain`,
`phy_force_rx_gain_trig`, `phy_force_txrx_off`, `phy_freq_band_reg_set`,
`phy_freq_chan_en_sw`, `phy_freq_correct`, `phy_freq_i2c_mem_write`,
`phy_freq_i2c_num_addr`, `phy_freq_i2c_write_set`,
`phy_freq_module_resetn`, `phy_freq_num_get_data`, `phy_freq_reg_init`,
`phy_freq_set_reg`.

### G

`phy_gen_rx_gain_table`, `phy_get_cca`, `phy_get_cca_cnt`,
`phy_get_chan_target_power`, `phy_get_data_sat`, `phy_get_dco_comp`,
`phy_get_dreg1p6`, `phy_get_fm_sar_dout`, `phy_get_freq_mem_addr`,
`phy_get_freq_mem_param`, `phy_get_i2c_hostid_`,
`phy_get_i2c_mst0_mask`, `phy_get_i2c_read_mask_`, `phy_get_iq_value`,
`phy_get_mac_addr`, `phy_get_max_pwr`, `phy_get_most_tpw`,
`phy_get_noise_floor`, `phy_get_oc_dr1`, `phy_get_pll_vol`,
`phy_get_power_atten`, `phy_get_power_db`, `phy_get_pwr_index`,
`phy_get_rate_fcc_index`, `phy_get_rc_dout`, `phy_get_rfcal_rxiq_data`,
`phy_get_romfuncs`, `phy_get_rssi`, `phy_get_rx_sig_pwr`,
`phy_get_rxbb_dc`, `phy_get_sar2_vol`, `phy_get_sar_sig_ref`,
`phy_get_target_pwr`, `phy_get_tone_sar_dout_`,
`phy_get_tsens_value_`, `phy_get_tx_gain_value`, `phy_get_txiq_set`.

### H–M

`phy_hemu_ru26_good_res`, `phy_i2c_bbpll_set`, `phy_i2c_clk_sel`,
`phy_i2c_enter_critical_`, `phy_i2c_exit_critical_`,
`phy_i2c_master_fill`, `phy_i2c_master_mem_txcap`,
`phy_i2c_master_reset`, `phy_i2c_paral_read`, `phy_i2c_paral_set_mst0`,
`phy_i2c_paral_set_read`, `phy_i2c_paral_write`,
`phy_i2c_paral_write_mask`, `phy_i2c_paral_write_num`,
`phy_i2c_rc_cal_set`, `phy_i2c_readReg`, `phy_i2c_readReg_Mask`,
`phy_i2c_sar2_init_code`, `phy_i2c_txrate_init`, `phy_i2c_writeReg`,
`phy_i2c_writeReg_Mask`, `phy_i2cmst_reg_init`,
`phy_index_to_txbbgain`, `phy_iq_corr_enable`, `phy_iq_est_disable`,
`phy_iq_est_enable`, `phy_is_low_rate_enabled`, `phy_linear_to_db`,
`phy_lltf_mask_en`, `phy_loopback_mode_en`, `phy_mac_enable_bb`,
`phy_mac_tx_chan_offset`, `phy_meas_tone_pwr_db`, `phy_mhz2ieee`.

### N–R

`phy_noise_floor_auto_set`, `phy_nrx_freq_set`, `phy_open_fe_bb_clk`,
`phy_open_i2c_xpd`, `phy_param_addr`, `phy_pbus_clear_reg`,
`phy_pbus_debugmode`, `phy_pbus_force_mode`, `phy_pbus_force_test`,
`phy_pbus_rd`, `phy_pbus_rd_addr`, `phy_pbus_rd_shift`,
`phy_pbus_rx_dco_cal`, `phy_pbus_rx_dco_cal_1step`,
`phy_pbus_set_dco`, `phy_pbus_set_rxgain`, `phy_pbus_workmode`,
`phy_pbus_xpd_rx_off`, `phy_pbus_xpd_rx_on`, `phy_pbus_xpd_tx_off`,
`phy_pbus_xpd_tx_on`, `phy_pkdet_vol_start`, `phy_pll_cap_mem_update`,
`phy_pll_dac_mem_update`, `phy_pwdet_code_cal`, `phy_pwdet_ref_code`,
`phy_pwdet_reg_init`, `phy_pwdet_sar2_init`, `phy_pwdet_tone_start`,
`phy_pwdet_wait_idle`, `phy_rate_to_index`, `phy_rc_cal`,
`phy_read_hw_noisefloor`, `phy_read_pll_cap`, `phy_read_rf_freq_mem`,
`phy_read_sar2_code`, `phy_read_sar_dout`, `phy_reg_init`,
`phy_reset_ckgen`, `phy_restart_cal`, `phy_rf_cal_data_backup`,
`phy_rf_cal_data_recovery`, `phy_rfcal_data_check`, `phy_rfcal_data_sub`,
`phy_rfcal_pwrctrl`, `phy_rfcal_rxiq`, `phy_rfcal_txcap`,
`phy_rfcal_txiq`, `phy_rfpll_cap_correct`, `phy_rfpll_cap_init_cal`,
`phy_rfpll_cap_track`, `phy_rfpll_chgp_cal`, `phy_rfpll_set_freq`,
`phy_rfrx_gain_index`, `phy_rfrx_sat_rst`, `phy_rx11blr_cfg`,
`phy_rx_11b_opt`, `phy_rx_filter_mode`, `phy_rx_gain_force`,
`phy_rx_sense_set`, `phy_rxdc_est_min`, `phy_rxiq_cover_mg_mp`,
`phy_rxiq_get_mis`, `phy_rxiq_set_reg`.

### S

`phy_save_pbus_reg`, `phy_set_bb_wdg`, `phy_set_cal_rxdc`,
`phy_set_cca`, `phy_set_cca_cnt`, `phy_set_chan_cal_interp`,
`phy_set_chan_freq_sw_start`, `phy_set_chan_reg`,
`phy_set_channel_dcode`, `phy_set_channel_rfpll_freq`,
`phy_set_ext_dcode`, `phy_set_freq`, `phy_set_lb_txiq`,
`phy_set_loopback_gain`, `phy_set_mac_data`, `phy_set_most_tpw`,
`phy_set_pbus_mem`, `phy_set_pbus_reg`, `phy_set_rf_freq_offset`,
`phy_set_rfpll_freq`, `phy_set_rx_comp_`, `phy_set_rx_gain_cal_dc`,
`phy_set_rx_gain_cal_iq`, `phy_set_rx_sense`, `phy_set_rxclk_en`,
`phy_set_tsens_power_`, `phy_set_tsens_range_`, `phy_set_tx_gain_mem`,
`phy_set_txcap_reg`, `phy_set_txcap_reset`, `phy_set_txclk_en`,
`phy_sifs_reg_init`, `phy_spur_cal`, `phy_spur_coef_cfg`,
`phy_spur_reg_write_one_tone`, `phy_start_tx_tone_step`,
`phy_stop_tx_tone`.

### T–X

`phy_temp_to_power`, `phy_tsens_code_read`, `phy_tsens_dac_cal`,
`phy_tsens_dac_to_index`, `phy_tsens_temp_read`,
`phy_tsens_temp_read_local`, `phy_tx_paon_set`, `phy_tx_pwctrl_bg_init`,
`phy_tx_pwctrl_init_cal`, `phy_tx_state_out`, `phy_tx_state_set`,
`phy_txbbgain_to_index`, `phy_txcal_debuge_mode_`,
`phy_txcal_work_mode`, `phy_txdc_cal`, `phy_txdc_cal_pwdet`,
`phy_txgain_comp_pacfg_`, `phy_txiq_cover`, `phy_txiq_get_mis_pwr`,
`phy_txiq_set_reg`, `phy_txpwr_cal_track`, `phy_txpwr_correct`,
`phy_txpwr_track_slow`, `phy_txtone_linear_pwr`, `phy_vht_support`,
`phy_wait_freq_set_busy`, `phy_wait_i2c_sdm_stable`,
`phy_wait_rfpll_cal_end`, `phy_wifi_11g_rate_chg`,
`phy_wifi_agc_sat_gain`, `phy_wifi_enable_set`, `phy_wifi_fbw_sel`,
`phy_wifi_get_target_power`, `phy_wifi_get_tx_gain`,
`phy_wifi_get_tx_tab_`, `phy_wifi_rifs_mode_en`,
`phy_wifi_set_tx_gain`, `phy_wifi_track_tx_power`,
`phy_wr_rf_freq_mem`, `phy_wr_rx_gain_mem`, `phy_write_chan_freq`,
`phy_write_gain_mem`, `phy_write_pbus_mem`, `phy_write_pll_cap`,
`phy_write_rfpll_sdm`, `phy_xpd_rf`.

## Inventory boundary

The ROM inventory deliberately includes functions that are not reachable from
the current cold Wi-Fi graph. The presence of a same-purpose Rust helper does
not mark a ROM function as ported; only a complete parent-to-leaf mapping in
the Rust functional inventory does.

Addresses and code sizes remain machine-reproducible with:

```console
llvm-nm -A --defined-only --extern-only --print-size \
  _oracles/esp32s31_rev0_rom.elf
```
