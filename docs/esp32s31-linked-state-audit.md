# ESP32-S31 linked state and interposition audit

- final ELF: `wifi-sta`
- ELF SHA-256: `e6697568ebae36a1bab6522ce57ee8a078ebd98118bd7c5ccef80d9a9b562670`
- ROM ELF: `esp32s31_rev0_rom.elf` / SHA-256 `a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`
- strict vendor roots: 1
- Rust boundaries retaining vendor fallback: 1
- stateful or not-yet-proven runtime roots: 0
- temporary evidenced MMIO-only roots: 0
- reference-only control-flow roots: `phy_change_channel`
- separately auditable static-binding roots: `net80211_data_ptr_init`, `wdev_data_init`
- separately auditable static-PM root: `pm_beacon_offset_funcs_init`
- separately auditable Rust caller-task cold init: `wifi_init_in_caller_task`, `wifi_deinit_in_caller_task`
- vendor functions reachable from those roots: 14
- strict runtime archive functions: 11 definitions / 3236 bytes
- cold-PHY archive functions: 80 definitions / 21596 bytes
- strict runtime direct ROM frontier: 3 functions / 302 bytes; unresolved externals: 0
- cold-PHY direct ROM frontier: 119 functions / 22816 bytes; unresolved externals: 2
- focused Wi-Fi full-cal radio graph: 65 archive definitions / 16622 bytes; 102 direct ROM functions / 11578 bytes; unresolved externals: 1
- live mutable blob globals reached by strict leaves: 4 symbols / 1412 bytes
- live mutable blob globals reached from `register_chipv7_phy`: 1 symbols / 508 bytes
- ROM-ABI mutable indirection cells reached by strict leaves: 3 cells / 12 cell bytes
- fixed cold-init bindings live in this ELF: 39 / 43
- validated Rust-owned ABI data aliases: 1 / 1
- live mutable blob globals outside the strict-root graph: 167 symbols / 19864 bytes
- Rust strict static sections: 52 sections / 287288 bytes
- retained code wrappers: 85

The archive relocation graph supplies `vendor function -> data symbol`; the final ELF supplies liveness, address, size and section. A wrapper boundary stops traversal into the replaced vendor body. “Outside strict roots” means linked but not proven runtime-reachable by this vendor-leaf graph; it is not automatically safe to delete because cold initialization and non-Wi-Fi owners can still use it.
The former strict audit proved the fixed-storage cold-init leaves together
with the runtime roots. Its primary baseline rejected growth beyond the
qualified heap-free image while allowing vendor roots, linked blob state, and
Rust static storage to shrink. The analyzer was removed after the
vendor-library analysis phase; this generated result and Git history preserve
the evidence.
The application `wifi-rust-static-cold-init-hil` final-ELF audit additionally proves the three fixed SRAM locks, the exact direct init/deinit call targets, the taskless PP tail calls, and the absence of control-flow cycles.

## Strict runtime archive function frontier

These are the exact archive definitions remaining below the strict runtime root after stopping at every Rust interposition boundary. Sizes are the original archive text sizes, not the replacement sizes.

| function | archive text bytes | archive owner |
|---|---:|---|
| `get_sublen_offset` | 326 | `libpp.a[wdev.o]` |
| `hal_get_dump_ctrl_frame_cfg` | 18 | `libpp.a[hal_debug.o]` |
| `hal_he_get_aid` | 12 | `libpp.a[hal_mac_ctl.o]` |
| `ic_interface_enabled` | 16 | `libpp.a[if_hwctrl.o]` |
| `is_ndpa_to_dut` | 126 | `libpp.a[wdev.o]` |
| `lmacRxDone` | 28 | `libpp.a[lmac.o]` |
| `ppEnqueueRxq` | 28 | `libpp.a[pp.o]` |
| `wDev_DiscardFrame` | 32 | `libpp.a[wdev.o]` |
| `wDev_IndicateFrame` | 824 | `libpp.a[wdev.o]` |
| `wDev_ProcessRxData_NAN_Interface_Hook` | 130 | `libpp.a[wdev.o]` |
| `wDev_ProcessRxSucData` | 1696 | `libpp.a[wdev.o]` |

### Strict runtime direct ROM/external frontier

These are direct calls leaving the pinned static archives. A ROM text size and address are reported only when the separately supplied ROM ELF defines the symbol.

| function | ROM text bytes | ROM address / status |
|---|---:|---|
| `memcmp` | 66 | `0x2f80d21e` |
| `memcpy` | 224 | `0x2f80d260` |
| `roundup2` | 12 | `0x2f819d02` |

## PHY cold-init archive function graph

This is the complete direct relocation graph rooted at `register_chipv7_phy` for definitions present in the pinned static archives. An archive function may call the external/ROM frontier below; calls internal to the ROM image are not expanded here.

| function | archive text bytes | archive owner |
|---|---:|---|
| `_etoa` | 1160 | `libprintf.a[printf.c.obj]` |
| `_ftoa` | 1000 | `libprintf.a[printf.c.obj]` |
| `_ntoa_format` | 326 | `libprintf.a[printf.c.obj]` |
| `_ntoa_long` | 130 | `libprintf.a[printf.c.obj]` |
| `_ntoa_long_long` | 248 | `libprintf.a[printf.c.obj]` |
| `_out_rev` | 178 | `libprintf.a[printf.c.obj]` |
| `_vsnprintf` | 1488 | `libprintf.a[printf.c.obj]` |
| `phy_11p_set` | 18 | `libphy.a[phy_feature.o]` |
| `phy_bb_init` | 362 | `libphy.a[phy_init.o]` |
| `phy_bb_txpwr_track` | 244 | `libphy.a[phy_reg.o]` |
| `phy_bias_reg_set` | 48 | `libphy.a[phy_i2c.o]` |
| `phy_bt_rx_mx_dgain` | 42 | `libphy.a[phy_rx_cal.o]` |
| `phy_bt_set_tx_gain_new` | 102 | `libphy.a[phy_tx_gain.o]` |
| `phy_bt_tx_gain_init` | 90 | `libphy.a[phy_tx_gain.o]` |
| `phy_bt_tx_pwctrl_init` | 430 | `libphy.a[phy_tx_cal.o]` |
| `phy_bt_txdc_cal_new` | 254 | `libphy.a[phy_tx_cal.o]` |
| `phy_chan14_mic_cfg_new` | 70 | `libphy.a[phy_basic.o]` |
| `phy_check_rx_sat` | 118 | `libphy.a[phy_rx_cal.o]` |
| `phy_chip_set_chan` | 270 | `libphy.a[phy_rfpll.o]` |
| `phy_chip_set_chan_misc_new` | 36 | `libphy.a[phy_rfpll.o]` |
| `phy_chip_set_chan_offset` | 124 | `libphy.a[phy_rfpll.o]` |
| `phy_dc_mem_clr` | 28 | `libphy.a[phy_reg.o]` |
| `phy_fe_reg_update` | 50 | `libphy.a[phy_reg.o]` |
| `phy_freq_get_i2c_data` | 520 | `libphy.a[phy_hw_freq.o]` |
| `phy_freq_i2c_data_write` | 50 | `libphy.a[phy_hw_freq.o]` |
| `phy_get_rf_cal_version` | 6 | `libphy.a[phy_api.o]` |
| `phy_get_rf_freq_init` | 472 | `libphy.a[phy_hw_freq.o]` |
| `phy_get_romfunc_addr` | 152 | `libphy.a[phy_init.o]` |
| `phy_get_rxbb_dc_new` | 46 | `libphy.a[phy_rx_gain.o]` |
| `phy_get_temp_init` | 76 | `libphy.a[phy_tsens.o]` |
| `phy_get_tsens_value` | 8 | `libphy.a[phy_tsens.o]` |
| `phy_get_xtal_duty` | 54 | `libphy.a[phy_rx_cal.o]` |
| `phy_get_xtal_freq` | 64 | `libphy.a[phy_init.o]` |
| `phy_i2c_enter_critical` | 2 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_exit_critical` | 2 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_init1` | 534 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_master_cmd_mem_init` | 1470 | `libphy.a[phy_i2c.o]` |
| `phy_open_i2c_xpd_new` | 172 | `libphy.a[phy_reg.o]` |
| `phy_pbus_rx_dco_cal_1step_new` | 1186 | `libphy.a[phy_rx_cal.o]` |
| `phy_printf` | 46 | `libprintf.a[printf.c.obj]` |
| `phy_rc_cal_init` | 54 | `libphy.a[phy_init.o]` |
| `phy_reg_update_new` | 112 | `libphy.a[phy_init.o]` |
| `phy_rf_cal_data_backup_new` | 22 | `libphy.a[phy_init.o]` |
| `phy_rf_cal_data_recovery_new` | 10 | `libphy.a[phy_init.o]` |
| `phy_rf_init` | 290 | `libphy.a[phy_init.o]` |
| `phy_rfcal_data_check_new` | 126 | `libphy.a[phy_init.o]` |
| `phy_rfcal_data_sub_new` | 100 | `libphy.a[phy_init.o]` |
| `phy_rfrx_gain_index_new` | 116 | `libphy.a[phy_rx_cal.o]` |
| `phy_rx_table_init` | 124 | `libphy.a[phy_rx_gain.o]` |
| `phy_rxdc_est_delta` | 218 | `libphy.a[phy_rx_cal.o]` |
| `phy_rxdc_fine_delta` | 272 | `libphy.a[phy_rx_cal.o]` |
| `phy_rxiq_cal_init` | 408 | `libphy.a[phy_rx_gain.o]` |
| `phy_set_chan_freq_hw_init` | 40 | `libphy.a[phy_hw_freq.o]` |
| `phy_set_ftm_en` | 20 | `libphy.a[phy_reg.o]` |
| `phy_set_lb_txiq_new` | 50 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_most_tpw_new` | 26 | `libphy.a[phy_feature.o]` |
| `phy_set_rx_gain_cal_dc_new` | 716 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_rx_gain_cal_iq_new` | 606 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_rx_gain_table` | 650 | `libphy.a[phy_rx_gain.o]` |
| `phy_set_tsens_power` | 28 | `libphy.a[phy_tsens.o]` |
| `phy_set_tx_cfr_mem` | 118 | `libphy.a[phy_tx_gain.o]` |
| `phy_set_tx_gain_mem_new` | 304 | `libphy.a[phy_tx_gain.o]` |
| `phy_start_tx_tone_step_new` | 194 | `libphy.a[phy_reg.o]` |
| `phy_stop_tx_tone_new` | 44 | `libphy.a[phy_reg.o]` |
| `phy_tsens_read_init` | 54 | `libphy.a[phy_tsens.o]` |
| `phy_tx_cap_init` | 230 | `libphy.a[phy_tx_cal.o]` |
| `phy_tx_pwctrl_init` | 154 | `libphy.a[phy_tx_cal.o]` |
| `phy_tx_pwctrl_init_cal_new` | 396 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_init` | 272 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_pwdet_init` | 520 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_pwdet_new` | 948 | `libphy.a[phy_tx_cal.o]` |
| `phy_txiq_cal_init` | 332 | `libphy.a[phy_tx_cal.o]` |
| `phy_wifi_set_tx_gain_new` | 114 | `libphy.a[phy_tx_gain.o]` |
| `phy_wr_rx_gain_mem_new` | 454 | `libphy.a[phy_rx_gain.o]` |
| `phy_xtal_duty_cal` | 914 | `libphy.a[phy_rx_cal.o]` |
| `phy_xtal_duty_cal_init` | 116 | `libphy.a[phy_rx_cal.o]` |
| `register_chipv7_phy` | 486 | `libphy.a[phy_init.o]` |
| `register_chipv7_phy_init_param` | 148 | `libphy.a[phy_init.o]` |
| `syslog` | 110 | `libprintf.a[printf.c.obj]` |
| `vsnprintf` | 24 | `libprintf.a[printf.c.obj]` |

### PHY cold-init direct ROM/external frontier

These symbols are called by the cold-PHY archive graph but have no definition in the pinned static archives. A supplied ROM ELF proves direct ROM text sizes and addresses. Calls internal to those ROM bodies are still outside this direct-frontier inventory.

| function | ROM text bytes | ROM address / status |
|---|---:|---|
| `__adddf3` | 2424 | `0x2f81dfca` |
| `__divdf3` | 1654 | `0x2f81e942` |
| `__divdi3` | 926 | `0x2f81ce6e` |
| `__esp_radio_printf` | - | unresolved external |
| `__fixdfsi` | 206 | `0x2f82018a` |
| `__fixunsdfsi` | 148 | `0x2f820258` |
| `__floatsidf` | 104 | `0x2f8202ec` |
| `__floatunsidf` | 78 | `0x2f820354` |
| `__gedf2` | 200 | `0x2f81f04e` |
| `__gtdf2` | 200 | `0x2f81f04e` |
| `__ledf2` | 200 | `0x2f81f116` |
| `__ltdf2` | 200 | `0x2f81f116` |
| `__muldf3` | 1468 | `0x2f81f1de` |
| `__nedf2` | 150 | `0x2f81efb8` |
| `__subdf3` | 2432 | `0x2f81f7a8` |
| `__udivdi3` | 862 | `0x2f81d574` |
| `__umoddi3` | 808 | `0x2f81d8d2` |
| `ets_delay_us` | 28 | `0x2f8036b8` |
| `memcpy` | 224 | `0x2f80d260` |
| `memset` | 168 | `0x2f8220c6` |
| `phy_abs_temp` | 10 | `0x2f825fa2` |
| `phy_adc_rate_set` | 74 | `0x2f82a6d2` |
| `phy_bb_agc_reg_update` | 166 | `0x2f82860e` |
| `phy_bb_cbw_chan_cfg` | 116 | `0x2f828238` |
| `phy_bbpll_cal` | 28 | `0x2f827dbc` |
| `phy_bt_bb_to_index` | 28 | `0x2f826b36` |
| `phy_bt_index_to_bb` | 28 | `0x2f826b1a` |
| `phy_byte_to_word` | 30 | `0x2f826034` |
| `phy_chan_to_freq` | 38 | `0x2f825788` |
| `phy_dcode_cal_init` | 128 | `0x2f82b8da` |
| `phy_dis_hw_set_freq` | 20 | `0x2f824fb2` |
| `phy_disable_agc` | 16 | `0x2f827460` |
| `phy_en_hw_set_freq` | 20 | `0x2f824f9e` |
| `phy_en_pwdet` | 38 | `0x2f8263da` |
| `phy_enable_agc` | 40 | `0x2f827470` |
| `phy_encode_i2c_master` | 10 | `0x2f82a81a` |
| `phy_fe_reg_init` | 246 | `0x2f827740` |
| `phy_filter_dcap_set` | 446 | `0x2f82a476` |
| `phy_force_txrx_off` | 102 | `0x2f827bb0` |
| `phy_freq_correct` | 248 | `0x2f827fae` |
| `phy_freq_i2c_write_set` | 326 | `0x2f824d34` |
| `phy_freq_module_resetn` | 28 | `0x2f824abe` |
| `phy_freq_reg_init` | 96 | `0x2f824c46` |
| `phy_gen_rx_gain_table` | 312 | `0x2f826814` |
| `phy_get_data_sat` | 16 | `0x2f826024` |
| `phy_get_iq_value` | 54 | `0x2f8295f2` |
| `phy_get_power_atten` | 278 | `0x2f82b0c8` |
| `phy_get_rfcal_rxiq_data` | 260 | `0x2f828dda` |
| `phy_get_romfuncs` | 10 | `0x2f824a82` |
| `phy_get_rx_sig_pwr` | 118 | `0x2f829ea2` |
| `phy_i2c_bbpll_set` | 84 | `0x2f82a67e` |
| `phy_i2c_clk_sel` | 104 | `0x2f829f1c` |
| `phy_i2c_master_fill` | 14 | `0x2f82a824` |
| `phy_i2c_master_mem_txcap` | 36 | `0x2f82a832` |
| `phy_i2c_master_reset` | 116 | `0x2f8260d0` |
| `phy_i2c_rc_cal_set` | 74 | `0x2f82a634` |
| `phy_i2c_readReg` | 4 | `0x2f82a30a` |
| `phy_i2c_readReg_Mask` | 44 | `0x2f82a37c` |
| `phy_i2c_sar2_init_code` | 50 | `0x2f82a444` |
| `phy_i2c_txrate_init` | 56 | `0x2f8286d0` |
| `phy_i2c_writeReg` | 4 | `0x2f82a378` |
| `phy_i2c_writeReg_Mask` | 88 | `0x2f82a3a8` |
| `phy_i2cmst_reg_init` | 34 | `0x2f8276c4` |
| `phy_index_to_txbbgain` | 32 | `0x2f826afa` |
| `phy_iq_corr_enable` | 36 | `0x2f827d8c` |
| `phy_iq_est_disable` | 44 | `0x2f828a88` |
| `phy_iq_est_enable` | 180 | `0x2f8289d4` |
| `phy_loopback_mode_en` | 44 | `0x2f825ff8` |
| `phy_mhz2ieee` | 60 | `0x2f82574c` |
| `phy_open_fe_bb_clk` | 56 | `0x2f823ec0` |
| `phy_param_addr` | 10 | `0x2f824a8c` |
| `phy_pbus_clear_reg` | 144 | `0x2f824572` |
| `phy_pbus_debugmode` | 6 | `0x2f8242a6` |
| `phy_pbus_force_test` | 66 | `0x2f824228` |
| `phy_pbus_rd` | 60 | `0x2f82426a` |
| `phy_pbus_rx_dco_cal` | 552 | `0x2f828f44` |
| `phy_pbus_set_dco` | 62 | `0x2f8243d0` |
| `phy_pbus_set_rxgain` | 92 | `0x2f8242b2` |
| `phy_pbus_workmode` | 6 | `0x2f8242ac` |
| `phy_pbus_xpd_rx_off` | 38 | `0x2f82430e` |
| `phy_pbus_xpd_rx_on` | 98 | `0x2f824334` |
| `phy_pbus_xpd_tx_off` | 58 | `0x2f824396` |
| `phy_pbus_xpd_tx_on` | 124 | `0x2f82440e` |
| `phy_pwdet_code_cal` | 76 | `0x2f82b432` |
| `phy_pwdet_ref_code` | 118 | `0x2f82b3bc` |
| `phy_pwdet_reg_init` | 92 | `0x2f82634a` |
| `phy_rc_cal` | 264 | `0x2f826242` |
| `phy_read_pll_cap` | 52 | `0x2f825a32` |
| `phy_reg_init` | 82 | `0x2f823ef8` |
| `phy_rfcal_pwrctrl` | 462 | `0x2f82b586` |
| `phy_rfcal_txcap` | 264 | `0x2f82b47e` |
| `phy_rfcal_txiq` | 306 | `0x2f82b1de` |
| `phy_rfpll_chgp_cal` | 244 | `0x2f825cd4` |
| `phy_rfpll_set_freq` | 162 | `0x2f8258ca` |
| `phy_rfrx_sat_rst` | 66 | `0x2f828944` |
| `phy_rxdc_est_min` | 152 | `0x2f82916c` |
| `phy_set_chan_reg` | 80 | `0x2f826080` |
| `phy_set_channel_dcode` | 54 | `0x2f82b95a` |
| `phy_set_channel_rfpll_freq` | 80 | `0x2f825c38` |
| `phy_set_loopback_gain` | 116 | `0x2f82448a` |
| `phy_set_mac_data` | 74 | `0x2f823fce` |
| `phy_set_pbus_mem` | 384 | `0x2f82479e` |
| `phy_set_rf_freq_offset` | 16 | `0x2f825c10` |
| `phy_set_rfpll_freq` | 118 | `0x2f825b9a` |
| `phy_set_rxclk_en` | 32 | `0x2f827cf6` |
| `phy_set_txcap_reg` | 68 | `0x2f82a400` |
| `phy_set_txclk_en` | 36 | `0x2f827cd2` |
| `phy_tsens_temp_read` | 50 | `0x2f825eec` |
| `phy_tsens_temp_read_local` | 94 | `0x2f825f1e` |
| `phy_tx_pwctrl_bg_init` | 30 | `0x2f8267f6` |
| `phy_txbbgain_to_index` | 50 | `0x2f826ac8` |
| `phy_txcal_work_mode` | 30 | `0x2f824554` |
| `phy_txdc_cal` | 476 | `0x2f82abbe` |
| `phy_txiq_set_reg` | 104 | `0x2f827c16` |
| `phy_wait_i2c_sdm_stable` | 74 | `0x2f823e76` |
| `phy_wifi_agc_sat_gain` | 12 | `0x2f827db0` |
| `phy_wifi_enable_set` | 24 | `0x2f828220` |
| `phy_wr_rf_freq_mem` | 82 | `0x2f824bf4` |
| `phy_write_gain_mem` | 42 | `0x2f8274f0` |
| `phy_write_pll_cap` | 64 | `0x2f8259f2` |
| `rtc_clk_xtal_freq_get` | - | unresolved external |

## Focused Wi-Fi full-calibration radio graph

This is the porting workset for the primary no-NVS Wi-Fi profile. Traversal stops before vendor logging/formatting and calibration-record check, backup, or recovery. Those omitted boundaries are deleted policy, not replacement targets. The table still includes BT/coexistence-named descendants reached unconditionally by the original parent; they remain candidates until register evidence or hardware qualification proves that a Wi-Fi-only parent may omit them.

Omitted boundaries: `phy_printf`, `syslog`, `phy_get_rf_cal_version`, `phy_rfcal_data_check_new`, `phy_rf_cal_data_backup_new`, `phy_rf_cal_data_recovery_new`.

| function | archive text bytes | archive owner |
|---|---:|---|
| `phy_11p_set` | 18 | `libphy.a[phy_feature.o]` |
| `phy_bb_init` | 362 | `libphy.a[phy_init.o]` |
| `phy_bb_txpwr_track` | 244 | `libphy.a[phy_reg.o]` |
| `phy_bias_reg_set` | 48 | `libphy.a[phy_i2c.o]` |
| `phy_bt_rx_mx_dgain` | 42 | `libphy.a[phy_rx_cal.o]` |
| `phy_bt_set_tx_gain_new` | 102 | `libphy.a[phy_tx_gain.o]` |
| `phy_bt_tx_gain_init` | 90 | `libphy.a[phy_tx_gain.o]` |
| `phy_bt_tx_pwctrl_init` | 430 | `libphy.a[phy_tx_cal.o]` |
| `phy_bt_txdc_cal_new` | 254 | `libphy.a[phy_tx_cal.o]` |
| `phy_chan14_mic_cfg_new` | 70 | `libphy.a[phy_basic.o]` |
| `phy_check_rx_sat` | 118 | `libphy.a[phy_rx_cal.o]` |
| `phy_chip_set_chan` | 270 | `libphy.a[phy_rfpll.o]` |
| `phy_chip_set_chan_misc_new` | 36 | `libphy.a[phy_rfpll.o]` |
| `phy_chip_set_chan_offset` | 124 | `libphy.a[phy_rfpll.o]` |
| `phy_dc_mem_clr` | 28 | `libphy.a[phy_reg.o]` |
| `phy_fe_reg_update` | 50 | `libphy.a[phy_reg.o]` |
| `phy_freq_get_i2c_data` | 520 | `libphy.a[phy_hw_freq.o]` |
| `phy_freq_i2c_data_write` | 50 | `libphy.a[phy_hw_freq.o]` |
| `phy_get_rf_freq_init` | 472 | `libphy.a[phy_hw_freq.o]` |
| `phy_get_romfunc_addr` | 152 | `libphy.a[phy_init.o]` |
| `phy_get_rxbb_dc_new` | 46 | `libphy.a[phy_rx_gain.o]` |
| `phy_get_temp_init` | 76 | `libphy.a[phy_tsens.o]` |
| `phy_get_tsens_value` | 8 | `libphy.a[phy_tsens.o]` |
| `phy_get_xtal_duty` | 54 | `libphy.a[phy_rx_cal.o]` |
| `phy_get_xtal_freq` | 64 | `libphy.a[phy_init.o]` |
| `phy_i2c_enter_critical` | 2 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_exit_critical` | 2 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_init1` | 534 | `libphy.a[phy_i2c.o]` |
| `phy_i2c_master_cmd_mem_init` | 1470 | `libphy.a[phy_i2c.o]` |
| `phy_open_i2c_xpd_new` | 172 | `libphy.a[phy_reg.o]` |
| `phy_pbus_rx_dco_cal_1step_new` | 1186 | `libphy.a[phy_rx_cal.o]` |
| `phy_rc_cal_init` | 54 | `libphy.a[phy_init.o]` |
| `phy_reg_update_new` | 112 | `libphy.a[phy_init.o]` |
| `phy_rf_init` | 290 | `libphy.a[phy_init.o]` |
| `phy_rfrx_gain_index_new` | 116 | `libphy.a[phy_rx_cal.o]` |
| `phy_rx_table_init` | 124 | `libphy.a[phy_rx_gain.o]` |
| `phy_rxdc_est_delta` | 218 | `libphy.a[phy_rx_cal.o]` |
| `phy_rxdc_fine_delta` | 272 | `libphy.a[phy_rx_cal.o]` |
| `phy_rxiq_cal_init` | 408 | `libphy.a[phy_rx_gain.o]` |
| `phy_set_chan_freq_hw_init` | 40 | `libphy.a[phy_hw_freq.o]` |
| `phy_set_ftm_en` | 20 | `libphy.a[phy_reg.o]` |
| `phy_set_lb_txiq_new` | 50 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_most_tpw_new` | 26 | `libphy.a[phy_feature.o]` |
| `phy_set_rx_gain_cal_dc_new` | 716 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_rx_gain_cal_iq_new` | 606 | `libphy.a[phy_rx_cal.o]` |
| `phy_set_rx_gain_table` | 650 | `libphy.a[phy_rx_gain.o]` |
| `phy_set_tsens_power` | 28 | `libphy.a[phy_tsens.o]` |
| `phy_set_tx_cfr_mem` | 118 | `libphy.a[phy_tx_gain.o]` |
| `phy_set_tx_gain_mem_new` | 304 | `libphy.a[phy_tx_gain.o]` |
| `phy_start_tx_tone_step_new` | 194 | `libphy.a[phy_reg.o]` |
| `phy_stop_tx_tone_new` | 44 | `libphy.a[phy_reg.o]` |
| `phy_tsens_read_init` | 54 | `libphy.a[phy_tsens.o]` |
| `phy_tx_cap_init` | 230 | `libphy.a[phy_tx_cal.o]` |
| `phy_tx_pwctrl_init` | 154 | `libphy.a[phy_tx_cal.o]` |
| `phy_tx_pwctrl_init_cal_new` | 396 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_init` | 272 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_pwdet_init` | 520 | `libphy.a[phy_tx_cal.o]` |
| `phy_txdc_cal_pwdet_new` | 948 | `libphy.a[phy_tx_cal.o]` |
| `phy_txiq_cal_init` | 332 | `libphy.a[phy_tx_cal.o]` |
| `phy_wifi_set_tx_gain_new` | 114 | `libphy.a[phy_tx_gain.o]` |
| `phy_wr_rx_gain_mem_new` | 454 | `libphy.a[phy_rx_gain.o]` |
| `phy_xtal_duty_cal` | 914 | `libphy.a[phy_rx_cal.o]` |
| `phy_xtal_duty_cal_init` | 116 | `libphy.a[phy_rx_cal.o]` |
| `register_chipv7_phy` | 486 | `libphy.a[phy_init.o]` |
| `register_chipv7_phy_init_param` | 148 | `libphy.a[phy_init.o]` |

### Focused Wi-Fi direct ROM/external frontier

| function | ROM text bytes | ROM address / status |
|---|---:|---|
| `__divdi3` | 926 | `0x2f81ce6e` |
| `ets_delay_us` | 28 | `0x2f8036b8` |
| `memcpy` | 224 | `0x2f80d260` |
| `memset` | 168 | `0x2f8220c6` |
| `phy_abs_temp` | 10 | `0x2f825fa2` |
| `phy_adc_rate_set` | 74 | `0x2f82a6d2` |
| `phy_bb_agc_reg_update` | 166 | `0x2f82860e` |
| `phy_bb_cbw_chan_cfg` | 116 | `0x2f828238` |
| `phy_bbpll_cal` | 28 | `0x2f827dbc` |
| `phy_bt_bb_to_index` | 28 | `0x2f826b36` |
| `phy_bt_index_to_bb` | 28 | `0x2f826b1a` |
| `phy_chan_to_freq` | 38 | `0x2f825788` |
| `phy_dcode_cal_init` | 128 | `0x2f82b8da` |
| `phy_dis_hw_set_freq` | 20 | `0x2f824fb2` |
| `phy_disable_agc` | 16 | `0x2f827460` |
| `phy_en_hw_set_freq` | 20 | `0x2f824f9e` |
| `phy_en_pwdet` | 38 | `0x2f8263da` |
| `phy_enable_agc` | 40 | `0x2f827470` |
| `phy_encode_i2c_master` | 10 | `0x2f82a81a` |
| `phy_fe_reg_init` | 246 | `0x2f827740` |
| `phy_filter_dcap_set` | 446 | `0x2f82a476` |
| `phy_force_txrx_off` | 102 | `0x2f827bb0` |
| `phy_freq_correct` | 248 | `0x2f827fae` |
| `phy_freq_i2c_write_set` | 326 | `0x2f824d34` |
| `phy_freq_module_resetn` | 28 | `0x2f824abe` |
| `phy_freq_reg_init` | 96 | `0x2f824c46` |
| `phy_gen_rx_gain_table` | 312 | `0x2f826814` |
| `phy_get_data_sat` | 16 | `0x2f826024` |
| `phy_get_iq_value` | 54 | `0x2f8295f2` |
| `phy_get_power_atten` | 278 | `0x2f82b0c8` |
| `phy_get_rfcal_rxiq_data` | 260 | `0x2f828dda` |
| `phy_get_romfuncs` | 10 | `0x2f824a82` |
| `phy_get_rx_sig_pwr` | 118 | `0x2f829ea2` |
| `phy_i2c_bbpll_set` | 84 | `0x2f82a67e` |
| `phy_i2c_clk_sel` | 104 | `0x2f829f1c` |
| `phy_i2c_master_fill` | 14 | `0x2f82a824` |
| `phy_i2c_master_mem_txcap` | 36 | `0x2f82a832` |
| `phy_i2c_master_reset` | 116 | `0x2f8260d0` |
| `phy_i2c_rc_cal_set` | 74 | `0x2f82a634` |
| `phy_i2c_readReg` | 4 | `0x2f82a30a` |
| `phy_i2c_readReg_Mask` | 44 | `0x2f82a37c` |
| `phy_i2c_sar2_init_code` | 50 | `0x2f82a444` |
| `phy_i2c_txrate_init` | 56 | `0x2f8286d0` |
| `phy_i2c_writeReg` | 4 | `0x2f82a378` |
| `phy_i2c_writeReg_Mask` | 88 | `0x2f82a3a8` |
| `phy_i2cmst_reg_init` | 34 | `0x2f8276c4` |
| `phy_index_to_txbbgain` | 32 | `0x2f826afa` |
| `phy_iq_corr_enable` | 36 | `0x2f827d8c` |
| `phy_iq_est_disable` | 44 | `0x2f828a88` |
| `phy_iq_est_enable` | 180 | `0x2f8289d4` |
| `phy_loopback_mode_en` | 44 | `0x2f825ff8` |
| `phy_mhz2ieee` | 60 | `0x2f82574c` |
| `phy_open_fe_bb_clk` | 56 | `0x2f823ec0` |
| `phy_param_addr` | 10 | `0x2f824a8c` |
| `phy_pbus_clear_reg` | 144 | `0x2f824572` |
| `phy_pbus_debugmode` | 6 | `0x2f8242a6` |
| `phy_pbus_force_test` | 66 | `0x2f824228` |
| `phy_pbus_rd` | 60 | `0x2f82426a` |
| `phy_pbus_rx_dco_cal` | 552 | `0x2f828f44` |
| `phy_pbus_set_dco` | 62 | `0x2f8243d0` |
| `phy_pbus_set_rxgain` | 92 | `0x2f8242b2` |
| `phy_pbus_workmode` | 6 | `0x2f8242ac` |
| `phy_pbus_xpd_rx_off` | 38 | `0x2f82430e` |
| `phy_pbus_xpd_rx_on` | 98 | `0x2f824334` |
| `phy_pbus_xpd_tx_off` | 58 | `0x2f824396` |
| `phy_pbus_xpd_tx_on` | 124 | `0x2f82440e` |
| `phy_pwdet_code_cal` | 76 | `0x2f82b432` |
| `phy_pwdet_ref_code` | 118 | `0x2f82b3bc` |
| `phy_pwdet_reg_init` | 92 | `0x2f82634a` |
| `phy_rc_cal` | 264 | `0x2f826242` |
| `phy_read_pll_cap` | 52 | `0x2f825a32` |
| `phy_reg_init` | 82 | `0x2f823ef8` |
| `phy_rfcal_pwrctrl` | 462 | `0x2f82b586` |
| `phy_rfcal_txcap` | 264 | `0x2f82b47e` |
| `phy_rfcal_txiq` | 306 | `0x2f82b1de` |
| `phy_rfpll_chgp_cal` | 244 | `0x2f825cd4` |
| `phy_rfpll_set_freq` | 162 | `0x2f8258ca` |
| `phy_rfrx_sat_rst` | 66 | `0x2f828944` |
| `phy_rxdc_est_min` | 152 | `0x2f82916c` |
| `phy_set_chan_reg` | 80 | `0x2f826080` |
| `phy_set_channel_dcode` | 54 | `0x2f82b95a` |
| `phy_set_channel_rfpll_freq` | 80 | `0x2f825c38` |
| `phy_set_loopback_gain` | 116 | `0x2f82448a` |
| `phy_set_pbus_mem` | 384 | `0x2f82479e` |
| `phy_set_rf_freq_offset` | 16 | `0x2f825c10` |
| `phy_set_rfpll_freq` | 118 | `0x2f825b9a` |
| `phy_set_rxclk_en` | 32 | `0x2f827cf6` |
| `phy_set_txcap_reg` | 68 | `0x2f82a400` |
| `phy_set_txclk_en` | 36 | `0x2f827cd2` |
| `phy_tsens_temp_read` | 50 | `0x2f825eec` |
| `phy_tsens_temp_read_local` | 94 | `0x2f825f1e` |
| `phy_tx_pwctrl_bg_init` | 30 | `0x2f8267f6` |
| `phy_txbbgain_to_index` | 50 | `0x2f826ac8` |
| `phy_txcal_work_mode` | 30 | `0x2f824554` |
| `phy_txdc_cal` | 476 | `0x2f82abbe` |
| `phy_txiq_set_reg` | 104 | `0x2f827c16` |
| `phy_wait_i2c_sdm_stable` | 74 | `0x2f823e76` |
| `phy_wifi_agc_sat_gain` | 12 | `0x2f827db0` |
| `phy_wifi_enable_set` | 24 | `0x2f828220` |
| `phy_wr_rf_freq_mem` | 82 | `0x2f824bf4` |
| `phy_write_gain_mem` | 42 | `0x2f8274f0` |
| `phy_write_pll_cap` | 64 | `0x2f8259f2` |
| `rtc_clk_xtal_freq_get` | - | unresolved external |

## Mutable blob state reached by strict vendor leaves

| symbol | size | placement | archive owner | strict referrers |
|---|---:|---|---|---|
| `TxRxCxt` | 1044 | `internal SRAM` / `.data` | `libpp.a[pp.o]` | `ppEnqueueRxq`, `wDev_IndicateFrame` |
| `wDevCtrl` | 72 | `internal SRAM` / `.data` | `libpp.a[wdev.o]` | `ic_interface_enabled` |
| `g_wifi_menuconfig` | 104 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ioctl.o]` | `wDev_ProcessRxSucData` |
| `g_lmac_cnt` | 192 | `internal SRAM` / `.bss` | `libpp.a[pp_debug.o]` | `wDev_ProcessRxSucData` |

## Mutable blob state reached by PHY cold initialization

This is the direct archive call graph rooted at `register_chipv7_phy`. It does not prove indirect ROM callbacks unreachable.

| symbol | size | placement | archive owner | cold PHY referrers |
|---|---:|---|---|---|
| `phy_param` | 508 | `internal SRAM` / `.data` | `libphy.a[phy_init.o]` | `phy_11p_set`, `phy_bb_init`, `phy_bt_set_tx_gain_new`, `phy_bt_tx_gain_init`, `phy_bt_tx_pwctrl_init`, +35 |

## Mutable ROM-ABI indirection cells reached by strict leaves

These absolute symbols name four-byte pointer/callback cells in the S31 ROM ABI RAM table. They are state even though `llvm-nm` reports linker kind `A`. A conventional `*_ptr -> *` backing is shown when the backing object is present in the ELF.

| cell | address | inferred backing | strict referrers |
|---|---:|---|---|
| `wifi_sta_rx_probe_req` | `0x2f07fe9c` | unresolved ROM ABI cell | `wDev_ProcessRxSucData` |
| `g_osi_funcs_p` | `0x2f07ff44` | Rust-installed strict OSI table pointer | `wDev_ProcessRxSucData` |
| `pTxRx` | `0x2f07ff58` | `TxRxCxt` | `ppEnqueueRxq`, `wDev_IndicateFrame` |

## Fixed cold-init state bindings

These are the exact direct stores recovered from the two separately audited cold-init leaves. The Rust interposition path publishes the same backing addresses without calling either vendor body.

| published pointer cell | address | fixed backing | bytes | placement |
|---|---:|---|---:|---|
| `g_wifi_nvs` | `0x2f00c4f4` | `s_wifi_nvs` | 1440 | `internal SRAM` / `.bss` |
| `g_scan` | `0x2f07ff8c` | `gScanStruct` | 284 | `internal SRAM` / `.bss` |
| `g_chm` | `0x2f07ff88` | `gChmCxt` | 592 | `internal SRAM` / `.bss` |
| `g_ic_ptr` | `0x2f07ff84` | `g_ic` | 788 | `internal SRAM` / `.bss` |
| `g_hmac_cnt_ptr` | `0x2f07ff80` | `g_hmac_cnt` | 64 | `internal SRAM` / `.bss` |
| `g_tx_cacheq_ptr` | `0x2f07ff7c` | `s_tx_cacheq` | 8 | `internal SRAM` / `.bss` |
| `g_mac_sleep_en_ptr` | `0x2f07fed4` | `g_mac_sleep_en` | 1 | `internal SRAM` / `.bss` |
| `g_esp_mesh_quick_funcs_ptr` | `0x2f07fedc` | `esp_mesh_quick_funcs` | 176 | `internal SRAM` / `.bss` |
| `g_mesh_init_ps_type_ptr` | `0x2f07fec8` | `g_mesh_init_ps_type` | 4 | `internal SRAM` / `.data.wifi` |
| `g_mesh_is_started_ptr` | `0x2f07fec4` | `g_mesh_is_started` | 1 | `internal SRAM` / `.data.wifi` |
| `g_mesh_is_root_ptr` | `0x2f07fed0` | `g_mesh_is_root` | 1 | `internal SRAM` / `.data.wifi` |
| `g_mesh_topology_ptr` | `0x2f07fecc` | `g_mesh_topology` | 4 | `internal SRAM` / `.bss` |
| `pTxRx` | `0x2f07ff58` | `TxRxCxt` | 1044 | `internal SRAM` / `.data` |
| `lmacConfMib_ptr` | `0x2f07ff54` | `lmacConfMib` | 48 | `internal SRAM` / `.data` |
| `wDevCtrl_ptr` | `0x2f07ff40` | `wDevCtrl` | 72 | `internal SRAM` / `.data` |
| `wDevMacSleep_ptr` | `0x2f07ff3c` | `wDevMacSleep` | 120 | `internal SRAM` / `.bss` |
| `g_lmac_cnt_ptr` | `0x2f07ff38` | `g_lmac_cnt` | 192 | `internal SRAM` / `.bss` |
| `pp_sig_cnt_ptr` | `0x2f07ff34` | `pp_sig_cnt` | 36 | `internal SRAM` / `.data.wifi` |
| `g_wifi_menuconfig_ptr` | `0x2f07fef0` | `g_wifi_menuconfig` | 104 | `internal SRAM` / `.bss` |
| `g_eb_list_desc_ptr` | `0x2f07ff30` | `g_eb_list_desc` | 220 | `internal SRAM` / `.data` |
| `s_fragment_ptr` | `0x2f07ff2c` | `s_fragment` | 16 | `internal SRAM` / `.bss` |
| `if_ctrl_ptr` | `0x2f07ff28` | `if_ctrl` | 40 | `internal SRAM` / `.bss` |
| `ap_no_lr_ptr` | `0x2f07ff08` | `ap_no_lr` | 1 | `internal SRAM` / `.bss` |
| `trc_ctl_ptr` | `0x2f07fef4` | `trc_ctl` | 28 | `internal SRAM` / `.data` |
| `g_pm_cfg_ptr` | `0x2f07fee4` | `g_pm_cfg` | 88 | `internal SRAM` / `.data` |
| `g_pm_ptr` | `0x2f07fee8` | `g_pm` | 1176 | `internal SRAM` / `.bss` |
| `g_txop_queue_status_ptr` | `0x2f07fed8` | `wifi_strict_txop_queue_status` | 3 | `internal SRAM` / `.critical.data.wifi_strict.txop_queue_status` |
| `g_pm_cnt_ptr` | `0x2f07feec` | `g_pm_cnt` | 72 | `internal SRAM` / `.bss` |
| `g_pp_timer_info_ptr` | `0x2f07fc7c` | `g_pp_timer_info` | 136 | `internal SRAM` / `.data` |
| `g_rts_threshold_bytes_ptr` | `0x2f07fc78` | `g_rts_threshold_bytes` | 120 | `internal SRAM` / `.bss` |
| `g_pm_twt_ptr` | `0x2f07fee0` | `g_pm_twt` | 24 | `internal SRAM` / `.data` |
| `g_he_max_apep_length_tab_ptr` | `0x2f07fc74` | `g_he_max_apep_length_tab` | 480 | `internal SRAM` / `.bss` |
| `g_wdev_dbg_rx_ptr` | `0x2f07fda4` | `g_wdev_dbg_rx` | 16 | `internal SRAM` / `.bss` |
| `s_pm_beacon_offset_ptr` | `0x2f07fc70` | `s_pm_beacon_offset` | 76 | `internal SRAM` / `.bss` |
| `s_pm_beacon_offset_config_ptr` | `0x2f07fc6c` | `s_pm_beacon_offset_config` | 6 | `internal SRAM` / `.bss` |
| `s_tbttstart_ptr` | `0x2f07fc68` | `s_tbttstart` | 8 | `internal SRAM` / `.bss` |
| `s_offchan_tx_progress_in_ptr` | `0x2f07fc64` | `offchan_tx_progress_in` | 1 | `internal SRAM` / `.bss` |
| `g_offchan_packet_lifetime_ptr` | `0x2f07fc60` | `g_offchan_packet_lifetime` | 4 | `internal SRAM` / `.bss` |
| `g_send_wake_null_timer_ptr` | `0x2f07fc5c` | `send_wake_null_timer` | 20 | `internal SRAM` / `.bss` |

## Rust-owned ABI data aliases

Pinned vendor objects may still load these public C data names. The final link proves that each name resolves directly to explicit Rust-owned storage of the required size in internal SRAM; no separate blob allocation remains.

| public ABI name | Rust-owned backing | address | bytes | placement |
|---|---|---:|---:|---|
| `g_phyFuns` | `wifi_strict_phy_rom_function_table_binding` | `0x2f0541ec` | 4 | `internal SRAM` / `.critical.data.wifi_strict.phy_rom_function_table_binding` |

## Rust-owned strict static storage

| section | address | bytes | placement |
|---|---:|---:|---|
| `.data.wifi` | `0x2f00d050` | 360 | `internal SRAM` |
| `.critical.data.wifi_strict.radio_executor` | `0x2f00d1b8` | 17 | `internal SRAM` |
| `.critical.bss.wifi_strict.trc_default_contexts` | `0x2f00e1fc` | 456 | `internal SRAM` |
| `.critical.bss.wifi_strict.tbtt_adaptive_data` | `0x2f00e3c4` | 4 | `internal SRAM` |
| `.critical.bss.wifi_strict.pmksa_cache` | `0x2f00e3c8` | 20 | `internal SRAM` |
| `.critical.bss.wifi_strict.misc_nvs_initialized` | `0x2f00e3dc` | 1 | `internal SRAM` |
| `.critical.bss.wifi_strict.misc_nvs` | `0x2f00e3e0` | 60 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_large_rx_slots` | `0x2f00e41c` | 59008 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_large_rx_claims` | `0x2f01ca9c` | 8 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_management_slots` | `0x2f01caa4` | 27904 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_aggregate_rx_owners` | `0x2f0237a4` | 8 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_aggregate_rx_headers` | `0x2f0237ac` | 288 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_rejections` | `0x2f0238cc` | 4 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_management_claims` | `0x2f0238d0` | 4 | `internal SRAM` |
| `.critical.bss.wifi_strict.lmac_tx_done_state` | `0x2f0238d4` | 1 | `internal SRAM` |
| `.critical.data.wifi_strict.init_global_lock` | `0x2f0238d8` | 12 | `internal SRAM` |
| `.critical.data.wifi_strict.init_mac_list_lock` | `0x2f0238e4` | 12 | `internal SRAM` |
| `.critical.bss.wifi_strict.init_interrupt_lock` | `0x2f0238f0` | 4 | `internal SRAM` |
| `.critical.bss.wifi_strict.data_rx_channel` | `0x2f0238f4` | 532 | `internal SRAM` |
| `.critical.bss.wifi_strict.data_rx_slots` | `0x2f023b08` | 4160 | `internal SRAM` |
| `.critical.bss.wifi_strict.data_tx_channel` | `0x2f024b48` | 276 | `internal SRAM` |
| `.critical.bss.wifi_strict.data_tx_slots` | `0x2f024c5c` | 51584 | `internal SRAM` |
| `.critical.data.wifi_strict.basic_secondary_schedule` | `0x2f0315dc` | 12 | `internal SRAM` |
| `.critical.bss.wifi_strict.tx_queue_process_hil` | `0x2f0315e8` | 156 | `internal SRAM` |
| `.critical.bss.wifi_strict.pm_beacon_offset_functions` | `0x2f031684` | 68 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_cold_wifi_648` | `0x2f0316d0` | 5248 | `internal SRAM` |
| `.critical.bss.wifi_strict.rate_contexts` | `0x2f032b50` | 2432 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_cold_internal_788` | `0x2f0334d0` | 1600 | `internal SRAM` |
| `.critical.bss.wifi_strict.wdev_rx_payloads` | `0x2f033b10` | 54784 | `internal SRAM` |
| `.critical.bss.wifi_strict.esf_cold_internal_1748` | `0x2f041110` | 56320 | `internal SRAM` |
| `.critical.bss.wifi_strict.cold_api_envelopes` | `0x2f04ed10` | 48 | `internal SRAM` |
| `.critical.bss.wifi_strict.rate_table_scratch` | `0x2f04ed40` | 212 | `internal SRAM` |
| `.critical.bss.wifi_strict.wifi_interface_phy` | `0x2f04ee20` | 1296 | `internal SRAM` |
| `.critical.bss.wifi_strict.wifi_nvs_cfg_items` | `0x2f04f330` | 4640 | `internal SRAM` |
| `.critical.bss.wifi_strict.wdev_function_table` | `0x2f050550` | 1568 | `internal SRAM` |
| `.critical.bss.wifi_strict.wifi_interface_state` | `0x2f050b70` | 624 | `internal SRAM` |
| `.critical.bss.wifi_strict.wifi_nvs_load_scratch` | `0x2f050de0` | 1024 | `internal SRAM` |
| `.critical.bss.wifi_strict.net80211_function_table` | `0x2f0511e0` | 336 | `internal SRAM` |
| `.critical.bss.wifi_strict.wdev_rx_descriptor_arena` | `0x2f051330` | 384 | `internal SRAM` |
| `.critical.bss.wifi_strict.supplicant_callbacks` | `0x2f0514b0` | 108 | `internal SRAM` |
| `.critical.bss.wifi_strict.pp_bars` | `0x2f05151c` | 160 | `internal SRAM` |
| `.critical.bss.wifi_strict.ap_assoc_rejection` | `0x2f0515bc` | 44 | `internal SRAM` |
| `.critical.bss.wifi_strict.rx_addba` | `0x2f0515e8` | 15 | `internal SRAM` |
| `.critical.bss.wifi_strict.ap_nodes` | `0x2f0515f8` | 10400 | `internal SRAM` |
| `.critical.data.wifi_strict.rate_schedules` | `0x2f053e98` | 852 | `internal SRAM` |
| `.critical.data.wifi_strict.phy_rom_function_table_binding` | `0x2f0541ec` | 4 | `internal SRAM` |
| `.critical.data.wifi_strict.txop_queue_status` | `0x2f0541f0` | 3 | `internal SRAM` |
| `.critical.bss.wifi_strict.data_tx_wakers` | `0x2f0541f4` | 24 | `internal SRAM` |
| `.critical.bss.wifi_strict.ap_join_diagnostics` | `0x2f05420c` | 8 | `internal SRAM` |
| `.bss.esp_wifi_async_net80211` | `0x2f054214` | 33 | `internal SRAM` |
| `.critical.text.wifi_strict.lmac_release_txop_queue` | `0x4009f6c4` | 72 | `flash-mapped` |
| `.critical.text.wifi_strict.lmac_request_txop_queue` | `0x4009f70c` | 90 | `flash-mapped` |

## Final-link interposition

| boundary | replacement | mode | `__real_*` target |
|---|---:|---|---|
| `calloc` | `0x400924f6` | GNU `--wrap` boundary | - |
| `chm_return_home_channel` | `0x4009246e` | GNU `--wrap` boundary | - |
| `chm_start_op` | `0x40092466` | GNU `--wrap` boundary | - |
| `cnx_add_to_blacklist` | `0x2f0026f2` | retained replacement only | - |
| `cnx_check_bssid_in_blacklist` | `0x2f0026ee` | retained replacement only | - |
| `cnx_clear_blacklist` | `0x2f0026f4` | retained replacement only | - |
| `cnx_node_alloc` | `0x40095644` | GNU `--wrap` boundary | - |
| `cnx_node_search` | `0x4009564c` | retained replacement only | - |
| `cnx_remove_from_blacklist` | `0x2f0026f2` | retained replacement only | - |
| `dbg_dump_rx_ppdu` | `0x400938f6` | retained replacement only | - |
| `dbg_dump_rx_sigb` | `0x400938f8` | retained replacement only | - |
| `dbg_read_tx_ppdu` | `0x400938f8` | retained replacement only | - |
| `esf_buf_alloc` | `0x2f0024c2` | direct public alias | `0x2f800d1c` (ROM export) |
| `esf_buf_recycle` | `0x2f0024ca` | direct public alias | `0x2f800d24` (ROM export) |
| `esp_test_rx_parse_mu` | `0x400938f6` | direct public alias | `0x2f801178` (ROM export) |
| `esp_test_rx_process_complete` | `0x40093900` | direct public alias | `0x2f801158` (ROM export) |
| `esp_test_tx_enab_statistics` | `0x400938fc` | direct public alias | `0x2f801144` (ROM export) |
| `esp_wifi_internal_reg_rxcb` | `0x40095650` | retained replacement only | - |
| `esp_wifi_register_mgmt_frame_internal` | `0x400956be` | retained replacement only | - |
| `esp_wifi_set_config` | `0x40095714` | retained replacement only | - |
| `esp_wifi_set_country` | `0x400957ba` | retained replacement only | - |
| `esp_wifi_set_inactive_time` | `0x400959c6` | retained replacement only | - |
| `esp_wifi_set_max_tx_power` | `0x40095b02` | retained replacement only | - |
| `esp_wifi_set_mode` | `0x40095b90` | retained replacement only | - |
| `esp_wifi_set_promiscuous` | `0x40095be0` | retained replacement only | - |
| `esp_wifi_set_protocols` | `0x40095c48` | retained replacement only | - |
| `esp_wifi_set_ps` | `0x40095ef6` | retained replacement only | - |
| `esp_wifi_stop` | `0x40095f5c` | retained replacement only | - |
| `ets_delay_us` | `0x40092bec` | direct public alias | `0x2f80003c` (ROM export) |
| `free` | `0x40092692` | GNU `--wrap` boundary | - |
| `hal_crypto_set_key_entry` | `0x4009303c` | retained replacement only | - |
| `hal_mac_get_txq_complete` | `0x2f002568` | direct public alias | `0x2f800d44` (ROM export) |
| `hal_mac_get_txq_state` | `0x40092c1e` | direct public alias | `0x2f800d3c` (ROM export) |
| `ic_get_next_tbtt` | `0x40092e2a` | retained replacement only | - |
| `ieee80211_classify` | `0x4009244e` | GNU `--wrap` boundary | - |
| `ieee80211_hostapd_beacon_txcb` | `0x40092c94` | GNU `--wrap` boundary | - |
| `ieee80211_mgmt_output` | `0x40092476` | GNU `--wrap` boundary | - |
| `ieee80211_search_node` | `0x40095f9a` | direct public alias | `0x2f800ca8` (ROM export) |
| `ieee80211_set_tx_pti` | `0x4009247e` | direct public alias | `0x2f800cb8` (ROM export) |
| `ieee80211_timer_process` | `0x4009245e` | GNU `--wrap` boundary | - |
| `ieee80211_tx_mgt_cb` | `0x40092c9c` | retained replacement only | - |
| `lmacTxDone` | `0x2f0026ba` | direct public alias | `0x2f800dec` (ROM export) |
| `malloc` | `0x40092486` | GNU `--wrap` boundary | - |
| `misc_nvs_deinit` | `0x40095fa6` | retained replacement only | - |
| `misc_nvs_init` | `0x40095fba` | retained replacement only | - |
| `net80211_data_ptr_init` | `0x40096000` | retained replacement only | - |
| `os_sleep` | `0x40092c00` | GNU `--wrap` boundary | - |
| `pm_extend_tbtt_adaptive_attach` | `0x400960c2` | retained replacement only | - |
| `pm_extend_tbtt_adaptive_deattach` | `0x4009611e` | retained replacement only | - |
| `pm_funcs_deinit` | `0x40096158` | retained replacement only | - |
| `pm_funcs_init` | `0x40096164` | retained replacement only | - |
| `pm_on_beacon_rx` | `0x4009390a` | direct public alias | `0x2f800e98` (ROM export) |
| `pm_on_coex_schm_status_config` | `0x2f002a18` | retained replacement only | - |
| `pm_on_data_rx` | `0x4009390c` | direct public alias | `0x2f800e9c` (ROM export) |
| `pm_on_data_tx` | `0x4009390e` | direct public alias | `0x2f800ea0` (ROM export) |
| `pm_set_beacon_duration` | `0x40093910` | retained replacement only | - |
| `pmksa_cache_deinit` | `0x4009619c` | retained replacement only | - |
| `pmksa_cache_init` | `0x400961ce` | retained replacement only | - |
| `ppTxPkt` | `0x2f002c2c` | retained replacement only | - |
| `pp_create_task` | `0x4009620e` | retained replacement only | - |
| `pp_delete_task` | `0x4009629c` | retained replacement only | - |
| `pp_post` | `0x40092244` | GNU `--wrap` boundary | - |
| `rcGetSched` | `0x2f000dac` | retained replacement only | - |
| `realloc` | `0x4009261c` | GNU `--wrap` boundary | - |
| `sleep` | `0x4008fbaa` | GNU `--wrap` boundary | - |
| `sta_rx_cb` | `0x400848c8` | GNU `--wrap` boundary | - |
| `trc_deinit` | `0x4009639a` | retained replacement only | - |
| `trc_init` | `0x400963e0` | retained replacement only | - |
| `usleep` | `0x4008fc06` | GNU `--wrap` boundary | - |
| `vTaskDelay` | `0x4008fc32` | GNU `--wrap` boundary | - |
| `wDev_AppendRxBlocks` | `0x2f002a22` | direct public alias | `0x2f8010c4` (ROM export) |
| `wDev_IndicateCtrlFrame` | `0x2f002a1a` | GNU `--wrap` boundary | - |
| `wDev_SnifferRxData` | `0x4009390e` | retained replacement only | - |
| `wDev_ftm_set_t1t4` | `0x40093912` | retained replacement only | - |
| `wDev_isNANPktInValidSlot` | `0x40093914` | GNU `--wrap` boundary | - |
| `wDev_record_ftm_data` | `0x400938f6` | retained replacement only | - |
| `wdev_csi_rx_process` | `0x4009390e` | retained replacement only | - |
| `wdev_data_init` | `0x40096540` | retained replacement only | - |
| `wifi_assert` | `0x40093902` | retained replacement only | - |
| `wifi_deinit_in_caller_task` | `0x40096734` | retained replacement only | - |
| `wifi_gpio_debug` | `0x400938fa` | retained replacement only | - |
| `wifi_init_in_caller_task` | `0x4009681c` | retained replacement only | - |
| `wifi_log` | `0x4009390e` | retained replacement only | - |
| `wpa_ap_rx_eapol` | `0x40092ffa` | retained replacement only | - |
| `wpa_sm_rx_eapol` | `0x40092fbc` | retained replacement only | - |
| `wDev_DiscardFrame` | `0x2f002a2a` | direct public alias | `0x2f8010c8` (ROM export) |
| `wDev_ProcessRxSucData` | `0x2f002a32` | direct public alias | `0x2f8010f4` (ROM export) |
| `ppRecycleRxPkt` | `0x2f002552` | direct public alias | `0x2f800f98` (ROM export) |
| `esp_wifi_internal_free_rx_buffer` | `0x2f002552` | direct public alias | - |
| `esp_test_set_rx_error_occurs` | `0x4008d576` | direct public alias | `0x2f801164` (ROM export) |
| `rcUpdateTxDone` | `0x40092e3e` | direct public alias | `0x2f80106c` (ROM export) |
| `rcUpdateAckSnr` | `0x4009391c` | direct public alias | `0x2f801064` (ROM export) |
| `rcTxUpdatePer` | `0x4009396c` | direct public alias | `0x2f801060` (ROM export) |
| `trc_update_ifx_phy_mode` | `0x40097cc8` | direct public alias | - |
| `rcAttach` | `0x400971b4` | direct public alias | - |
| `rcUpdatePhyMode` | `0x4009721a` | direct public alias | - |
| `rc_get_default_sched` | `0x400971fe` | direct public alias | - |
| `rc_get_G6M_sched` | `0x4009720c` | direct public alias | - |
| `lmacRequestTxopQueue` | `0x4009f70c` | direct public alias | - |
| `lmacReleaseTxopQueue` | `0x4009f6c4` | direct public alias | - |
| `hal_get_tsf_time` | `0x2f002c34` | direct public alias | `0x2f82b9f8` (ROM export) |
| `hal_mac_rx_get_last_dscr` | `0x2f002c74` | direct public alias | `0x2f8386a2` (ROM export) |
| `hal_mac_tx_set_cca` | `0x2f002cae` | direct public alias | - |
| `hal_mac_is_txq_valid` | `0x2f002c60` | direct public alias | - |
| `hal_mac_set_txq_invalid` | `0x2f002c94` | direct public alias | - |
| `hal_mac_txq_disable` | `0x2f002cc6` | direct public alias | - |
| `hal_mac_set_csi_cbw` | `0x2f002c92` | direct public alias | - |
| `ic_set_mac` | `0x40096ab8` | direct public alias | - |
| `ic_set_rx_policy` | `0x40096b08` | direct public alias | - |
| `ic_set_rx_policy_ubssid_check` | `0x40096b9a` | direct public alias | - |
| `ieee80211_getmgtframe` | `0x40096bda` | direct public alias | - |
| `ic_set_key` | `0x40096954` | direct public alias | - |
| `ic_del_key` | `0x40096912` | direct public alias | - |
| `wDev_Insert_KeyEntry` | `0x40096996` | direct public alias | - |
| `phy_set_rx_comp_new` | `0x2f002d32` | direct public alias | - |
| `phy_dc_mem_clr` | `0x2f002d16` | direct public alias | - |
| `phy_bbpll_cal` | `0x2f002cdc` | direct public alias | - |
| `phy_set_tx_gain_mem_new` | `0x2f002d56` | direct public alias | - |
| `ieee80211_post_hmac_tx` | `0x40096c56` | direct public alias | `0x2f800cc0` (ROM export) |
| `ieee80211_crypto_encap` | `0x40092456` | direct public alias | `0x2f800cac` (ROM export) |
| `ieee80211_align_eb` | `0x40096bd2` | direct public alias | `0x2f800c7c` (ROM export) |
| `ieee80211_set_tx_desc` | `0x40096c5e` | direct public alias | `0x2f800c98` (ROM export) |
| `ppTxProtoProc` | `0x2f00161e` | direct public alias | - |
| `ppProcTxSecFrame` | `0x2f000f48` | direct public alias | - |

## Linked mutable blob state outside the strict-root graph

| symbol | size | placement | archive owner | linked referrers |
|---|---:|---|---|---|
| `est_PHY_RESP_FTM_COMP_40_40D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_40_40D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_40_40U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_40_40U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_40_40D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_40_40D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_40_40U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_40_40U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_40D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_40D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_40U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_40U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_40D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_40D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_40U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_40U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_20D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_20D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_20U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_RESP_FTM_COMP_20_20U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_20D_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_20D_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_20U_MHZ_DIS` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `est_PHY_INIT_FTM_COMP_20_20U_MHZ` | 2 | `internal SRAM` / `.data` | `libwifi_support.a[ftm_calibration_data.o]` | `ftm_get_phy_comp` |
| `TmpSTAAPCloseAP` | 1 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_hostap.o]` | `ieee80211_hostapd_beacon_txcb`, `wifi_softap_start`, `wifi_softap_stop` |
| `ccmp` | 24 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_crypto_ccmp.o]` | `ccmp_encap`, `esp_wifi_set_ap_key_internal`, `hostap_input`, `hostap_recv_mgmt`, `ppInstallKey`, +1 |
| `g_mesh_self_organized` | 1 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_mesh_quick.o]` | `cnx_connect_to_bss`, `cnx_sta_leave` |
| `s_itwt_id` | 16 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_twt.o]` | - |
| `s_tmp_itwt_id` | 16 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_twt.o]` | - |
| `setup_timer_param` | 372 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_twt.o]` | - |
| `g_def_2g_channels` | 11 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_nan_datapath.o]` | - |
| `s_wfa_oui` | 3 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_nan_sd.o]` | `nan_construct_publish_sdf`, `nan_construct_sdea`, `nan_construct_subscribe_sdf`, `nan_send_solicited_publish`, `nan_update_static_sdfs` |
| `g_espnow_user_oui` | 3 | `internal SRAM` / `.data` | `libespnow.a[manatick.o]` | `ieee80211_add_ie_vendor_esp_head`, `ieee80211_add_ie_vendor_esp_manufacturer`, `ieee80211_recv_action_vendor_esp_now` |
| `g_phy_cap_rx_stbc` | 1 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_ht.o]` | `ieee80211_add_hecap`, `ieee80211_ht_attach` |
| `g_wifi_nvs` | 4 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_nvs.o]` | `_do_wifi_disconnect`, `chm_check_channel_is_valid`, `chm_init`, `cnx_auth_done`, `cnx_bss_alloc`, +127 |
| `tkip` | 24 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_crypto_tkip.o]` | `esp_wifi_set_ap_key_internal`, `ppInstallKey`, `tkip_encap` |
| `wep` | 24 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_crypto_wep.o]` | `esp_wifi_set_ap_key_internal`, `ppInstallKey`, `wep_encap` |
| `sms4` | 24 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_crypto_sms4.o]` | `ppInstallKey`, `sms4_encap` |
| `g_timer_info` | 376 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_timer.o]` | `ieee80211_register_ftm_timer`, `ieee80211_register_hostap_timer`, `ieee80211_timer_do_process` |
| `gcmp` | 24 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_crypto_gcmp.o]` | `esp_wifi_set_ap_key_internal`, `gcmp_encap`, `ppInstallKey` |
| `WIFI_MESH_EVENT` | 4 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_api.o]` | - |
| `g_wifi_event_mask` | 4 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_api.o]` | - |
| `esp_test_tx_addba_request` | 1 | `internal SRAM` / `.data` | `libnet80211.a[wl_cnx.o]` | `cnx_auth_done`, `cnx_node_join`, `ieee80211_encap_esfbuf` |
| `g_dynamic_cs` | 12 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_sta.o]` | - |
| `send_deauth` | 1 | `internal SRAM` / `.data` | `libnet80211.a[ieee80211_sta.o]` | - |
| `phy_param` | 508 | `internal SRAM` / `.data` | `libphy.a[phy_init.o]` | `phy_11p_set`, `phy_bb_init`, `phy_bt_get_tx_tab_new`, `phy_bt_set_tx_gain_new`, `phy_bt_tx_gain_init`, +42 |
| `g_pp_timer_info` | 136 | `internal SRAM` / `.data` | `libpp.a[pp_timer.o]` | `pp_timer_do_process` |
| `g_pm_cfg` | 88 | `internal SRAM` / `.data` | `libpp.a[pm.o]` | `pm_attach`, `pm_beacon_miss_exceeded_wakeup_disabled`, `pm_beacon_monitor_tbtt_allowed`, `pm_beacon_monitor_timeout_process`, `pm_beacon_offset_get_expect`, +29 |
| `g_eb_list_desc` | 220 | `internal SRAM` / `.data` | `libpp.a[esf_buf.o]` | - |
| `lmacConfMib` | 48 | `internal SRAM` / `.data` | `libpp.a[lmac.o]` | `ppProcessLifeTime`, `ppProcessTxQ`, `ppRxFragmentProc`, `ppTxFragmentProc` |
| `g_pm_twt` | 24 | `internal SRAM` / `.data` | `libpp.a[pm_twt.o]` | - |
| `trc_ctl` | 28 | `internal SRAM` / `.data` | `libpp.a[trc.o]` | - |
| `txop_max_list` | 8 | `internal SRAM` / `.data` | `libpp.a[trc.o]` | - |
| `BcnInterval` | 4 | `internal SRAM` / `.data` | `libpp.a[wdev.o]` | - |
| `he_data_bits_per_sym` | 160 | `internal SRAM` / `.data` | `libpp.a[hal_mac_ctl.o]` | `ic_set_he_rts_threshold_bytes_tab`, `rx11AXRate2AMPDULimit_update` |
| `he_preamble_ersu` | 16 | `internal SRAM` / `.data` | `libpp.a[hal_mac_ctl.o]` | `ic_set_he_rts_threshold_bytes_tab`, `rx11AXRate2AMPDULimit_update` |
| `he_preamble_su` | 16 | `internal SRAM` / `.data` | `libpp.a[hal_mac_ctl.o]` | `ic_set_he_rts_threshold_bytes_tab` |
| `he_time_per_sym` | 12 | `internal SRAM` / `.data` | `libpp.a[hal_mac_ctl.o]` | `ic_set_he_rts_threshold_bytes_tab`, `rx11AXRate2AMPDULimit_update` |
| `g_mesh_is_started` | 1 | `internal SRAM` / `.data.wifi` | `libnet80211.a[ieee80211_mesh_quick.o]` | `ic_set_trc`, `lmacAdjustTimestamp`, `lmacSetTxFrame`, `pm_allow_tx`, `pm_disconnected_sleep`, +20 |
| `g_mesh_init_ps_type` | 4 | `internal SRAM` / `.data.wifi` | `libnet80211.a[ieee80211_mesh_quick.o]` | `cnx_auth_done`, `cnx_beacon_timeout_process`, `cnx_update_bss_more`, `ieee80211_assoc_req_construct`, `ieee80211_assoc_resp_construct`, +18 |
| `g_mesh_is_root` | 1 | `internal SRAM` / `.data.wifi` | `libnet80211.a[ieee80211_mesh_quick.o]` | `cnx_beacon_timeout_process`, `cnx_connect_next_ap`, `cnx_connect_to_bss`, `cnx_sta_leave`, `ic_set_trc`, +22 |
| `pp_sig_cnt` | 36 | `internal SRAM` / `.data.wifi` | `libpp.a[pp.o]` | - |
| `eb_txdesc_space` | 288 | `internal SRAM` / `.data.wifi` | `libpp.a[esf_buf.o]` | - |
| `ptr_beacon_offset_funcs` | 4 | `internal SRAM` / `.data.wifi` | `libpp.a[pm_beacon_offset.o]` | `pm_beacon_add_loss_counter`, `pm_beacon_add_total_counter`, `pm_beacon_monitor_tbtt_allowed`, `pm_beacon_monitor_tbtt_start`, `pm_beacon_offset_funcs_init`, +9 |
| `ap_rxcb` | 4 | `internal SRAM` / `.bss.esp_wifi_async_net80211` | `libnet80211.a[ieee80211_hostap.o]` | `ieee80211_decap_amsdu` |
| `eloop_lifecycle_busy` | 1 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[eloop.c.obj]` | `eloop_lifecycle_lock`, `eloop_lifecycle_unlock` |
| `eloop_data_lock` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[eloop.c.obj]` | `eloop_cancel_timeout`, `eloop_init`, `eloop_register_timeout`, `eloop_run` |
| `g_wpa_config_changed` | 1 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_wpa_main.c.obj]` | `wpa_sta_connect` |
| `wpa_cb` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_wpa_main.c.obj]` | `esp_supplicant_init` |
| `wifi_funcs` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_wpa_main.c.obj]` | `ap_free_sta`, `ap_get_sta`, `ap_sta_add`, `eloop_arm_next_locked`, `eloop_cancel_timeout`, +9 |
| `g_wpa_pmk_caching_disabled` | 1 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_wpa_main.c.obj]` | `wpa_sta_disconnected_cb` |
| `s_sm_valid_bitmap` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[wpa_auth.c.obj]` | `hostap_eapol_resend_process`, `wpa_auth_for_each_sta`, `wpa_auth_sta_init`, `wpa_free_sta_sm` |
| `s_wps_sm_cb` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_wps.c.obj]` | `wps_get_wps_sm_cb` |
| `global_hapd` | 4 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_hostap.c.obj]` | `hostap_init`, `hostapd_cleanup`, `hostapd_get_hapd_data`, `wpa_ap_remove` |
| `g_ic` | 788 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211.o]` | `_do_wifi_disconnect`, `_do_wifi_start`, `_do_wifi_stop`, `add_mic_ie_bip`, `addba_response_txcb`, +349 |
| `wpa_crypto_funcs` | 52 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_crypto.o]` | `ieee80211_ccmp_decrypt`, `ieee80211_ccmp_encrypt`, `ieee80211_crypto_aes_128_cmac_decrypt`, `ieee80211_crypto_bip_encrypt`, `ieee80211_crypto_bip_encrypt_with_key`, +4 |
| `color_change_timer` | 20 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_he.o]` | - |
| `esp_test_rx_trs_count` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_he.o]` | - |
| `esp_wifi_opr_bss_color` | 3 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_he.o]` | - |
| `len_dh_ie` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_output.o]` | - |
| `s_tx_cacheq` | 8 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_output.o]` | - |
| `g_beacon_eb` | 8 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_hostap.o]` | `ieee80211_getbcnframe` |
| `g_beacon_idx` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_hostap.o]` | `ieee80211_getbcnframe` |
| `g_deauth_mac_list` | 12 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_hostap.o]` | - |
| `g_sa_query_mac_list` | 12 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_hostap.o]` | - |
| `esp_mesh_quick_funcs` | 176 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_mesh_quick.o]` | `cnx_update_bss_more`, `hostap_handle_timer_process`, `hostap_recv_mgmt`, `ieee80211_alloc_proberesp`, `ieee80211_assoc_req_construct`, +17 |
| `g_mesh_topology` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_mesh_quick.o]` | `ieee80211_encap_esfbuf` |
| `itwt_information_timer` | 160 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_twt.o]` | - |
| `s_itwt_flow_id_bitmap` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_twt.o]` | - |
| `s_itwt_resume_flow_id_bitmap` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_twt.o]` | - |
| `s_itwt_suspend_flow_id_bitmap` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_twt.o]` | - |
| `btwt_setup_timer` | 1248 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_btwt.o]` | - |
| `g_btwt_num` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_btwt.o]` | - |
| `s_btwt_id_bitmap` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_btwt.o]` | `he_twt_teardown_txcb`, `ieee80211_close_all_twt_sessions` |
| `s_avail_seq` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_datapath.o]` | - |
| `s_dp` | 1324 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_datapath.o]` | - |
| `gChmCxt` | 592 | `internal SRAM` / `.bss` | `libnet80211.a[wl_chm.o]` | `chm_acquire_lock`, `chm_cancel_op`, `chm_change_channel`, `chm_deinit`, `chm_end_op`, +14 |
| `action_q` | 8 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | - |
| `g_nan_secure_dp_funcs` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | `nan_construct_ndp_confirm`, `nan_construct_ndp_req`, `nan_construct_ndp_resp`, `nan_construct_ndp_security_key_install`, `nan_construct_publish_sdf`, +13 |
| `g_nan_started` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | `nan_action_timeout`, `nan_disc_bcn_timeout`, `nan_dwend_timeout`, `nan_dwstart_timeout`, `nan_faw_end_timeout`, +5 |
| `ndp_rxcb` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | - |
| `s_nan_cb` | 52 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | `nan_construct_publish_sdf`, `nan_construct_subscribe_sdf`, `nan_rx_sdf`, `nan_send_solicited_publish`, `nan_update_static_sdfs` |
| `s_ni` | 376 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nan_common.o]` | - |
| `g_offchan_ctx` | 28 | `internal SRAM` / `.bss` | `libnet80211.a[wl_offchan.o]` | `wifi_set_rx_policy` |
| `g_offchan_packet_lifetime` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[wl_offchan.o]` | `ppProcessLifeTime` |
| `offchan_tx_progress_in` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_offchan.o]` | `offchan_in_progress` |
| `g_hmac_cnt` | 64 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_debug.o]` | `ampdu_dispatch_upto`, `hostap_input`, `hostap_recv_mgmt`, `ieee80211_ampdu_reorder`, `ieee80211_output_do`, +5 |
| `app_scan_params` | 16 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_scan.o]` | - |
| `connect_scan_flag` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_scan.o]` | - |
| `gScanStruct` | 284 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_scan.o]` | `ieee80211_scan_attach`, `ieee80211_scan_deattach`, `ieee80211_update_channel`, `mgd_probe_send_timeout_process`, `scan_add_probe_ssid`, +20 |
| `scannum` | 2 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_scan.o]` | - |
| `esp_test_baparas_support_amsdu` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ht.o]` | `ht_recv_action_ba_addba_request`, `ieee80211_ampdu_request` |
| `s_wifi_nvs` | 1440 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_nvs.o]` | `wifi_nvs_cfg_init`, `wifi_nvs_cfg_item_init`, `wifi_nvs_compare_cfg_diff`, `wifi_nvs_deinit`, `wifi_nvs_load`, +1 |
| `g_mac_sleep_en` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ioctl.o]` | - |
| `itwt_probe_timer` | 20 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ioctl.o]` | `itwt_probe_rc_tx_cb`, `itwt_probe_timeout_fn_process`, `itwt_stop_process`, `sta_recv_mgmt` |
| `mac_list_lock` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ioctl.o]` | `clear_mac_queue`, `hostap_add_in_mac_list`, `hostap_del_mac_info_from_list`, `hostap_query_mac_in_list` |
| `s_wifi_task_hdl` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ioctl.o]` | - |
| `ftm_resp_ctx` | 12 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_ftm.o]` | - |
| `s_wifi_api_lock` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_api.o]` | - |
| `s_wifi_stop_in_progress` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_api.o]` | `esp_sta_reset_rmac_process`, `wifi_stop_old_mode` |
| `ap_no_lr` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | - |
| `g_authmode_incompatible` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `scan_parse_beacon`, `scan_profile_check` |
| `g_authmode_threshold_failure` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `ieee80211_parse_rsn`, `scan_profile_check` |
| `g_cnxMgr` | 5256 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `cnx_add_rc`, `cnx_auth_done`, `cnx_bss_alloc`, `cnx_cal_rc_util`, `cnx_choose_rc`, +19 |
| `g_cnx_probe_rc_list_cb` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | - |
| `g_in_blacklist_flag` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `scan_profile_check` |
| `g_in_blacklist_scanned_again` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `scan_parse_beacon`, `scan_profile_check` |
| `g_rssi_threshold_failure` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[wl_cnx.o]` | `scan_profile_check` |
| `in_rssi_adjust` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | - |
| `rssi_index` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | - |
| `rssi_saved` | 8 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | - |
| `s_eapol_txdone_cb` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | - |
| `send_wake_null_timer` | 20 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | - |
| `sta_csa_timer` | 20 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_sta.o]` | `cnx_sta_leave` |
| `g_wifi_improve_contention_ability` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[ieee80211_proto.o]` | `ieee80211_wme_updateparams` |
| `esp_test_rx_ctrl` | 72 | `internal SRAM` / `.bss` | `libnet80211.a[test.o]` | `esp_test_rx_parse_trig`, `sta_recv_ctl` |
| `g_rx_trig_idx` | 4 | `internal SRAM` / `.bss` | `libnet80211.a[test_rx_trig.o]` | - |
| `g_store_rx_trig` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[test_rx_trig.o]` | - |
| `g_store_rx_trig_print` | 1 | `internal SRAM` / `.bss` | `libnet80211.a[test_rx_trig.o]` | - |
| `test_rx_trig_bfrp` | 1200 | `internal SRAM` / `.bss` | `libnet80211.a[test_rx_trig.o]` | - |
| `g_pm` | 1176 | `internal SRAM` / `.bss` | `libpp.a[pm.o]` | `hal_get_time_to_sta_next_tbtt`, `is_off_channel`, `pm_active_timeout_process`, `pm_allow_tx`, `pm_attach`, +133 |
| `g_rts_threshold_bytes` | 120 | `internal SRAM` / `.bss` | `libpp.a[if_hwctrl.o]` | - |
| `if_ctrl` | 40 | `internal SRAM` / `.bss` | `libpp.a[if_hwctrl.o]` | - |
| `s_is_6m` | 1 | `internal SRAM` / `.bss` | `libpp.a[if_hwctrl.o]` | - |
| `s_fragment` | 16 | `internal SRAM` / `.bss` | `libpp.a[pp.o]` | - |
| `g_bss_color_collision_detection_enabled` | 2 | `internal SRAM` / `.bss` | `libpp.a[pp_he_ctrl.o]` | `wifi_process_bsscolor_collision` |
| `eb_space` | 240 | `internal SRAM` / `.bss` | `libpp.a[esf_buf.o]` | `esf_buf_setup` |
| `g_he_max_apep_length_tab` | 480 | `internal SRAM` / `.bss` | `libpp.a[trc.o]` | - |
| `s_fix_rate` | 12 | `internal SRAM` / `.bss` | `libpp.a[trc.o]` | - |
| `s_fix_rate_mask` | 4 | `internal SRAM` / `.bss` | `libpp.a[trc.o]` | - |
| `g_pm_cnt` | 72 | `internal SRAM` / `.bss` | `libpp.a[pp_debug.o]` | `pm_active_timeout_process`, `pm_beacon_timestamp_statistic`, `pm_dream`, `pm_on_beacon_rx`, `pm_process_tim`, +4 |
| `BcnSendTick` | 4 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `g_wdev_dbg_rx` | 16 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `g_wdev_is_nan_pkt_in_valid_slot_cb` | 4 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `g_wdev_record_t1t4_cb` | 4 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `g_wdev_record_t2t3_cb` | 4 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `g_wdev_set_t1t4_cb` | 4 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `wDevMacSleep` | 120 | `internal SRAM` / `.bss` | `libpp.a[wdev.o]` | - |
| `s_pm_beacon_offset` | 76 | `internal SRAM` / `.bss` | `libpp.a[pm_beacon_offset.o]` | - |
| `s_pm_beacon_offset_config` | 6 | `internal SRAM` / `.bss` | `libpp.a[pm_beacon_offset.o]` | - |
| `s_tbttstart` | 8 | `internal SRAM` / `.bss` | `libpp.a[hal_tsf.o]` | `hal_set_sta_tbtt`, `hal_tsf_get_tbttstart` |
| `strid.1` | 20 | `internal SRAM` / `.bss` | `libpp.a[hal_utilities.o]` | `rate2str` |
| `assoc_ie_buf` | 48 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[wpa.c.obj]` | `wpa_set_bss` |
| `gWpaSm` | 1160 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[wpa.c.obj]` | `eapol_txcb`, `ieee80211_handle_rx_frm`, `set_assoc_ie`, `wpa_config_reload`, `wpa_deattach`, +16 |
| `eloop` | 36 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[eloop.c.obj]` | `eloop_arm_next_locked`, `eloop_cancel_timeout`, `eloop_init`, `eloop_insert_timeout_locked.isra.0`, `eloop_is_running`, +3 |
| `s_sm_table` | 64 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[wpa_auth.c.obj]` | `hostap_eapol_resend_process`, `wpa_auth_for_each_sta`, `wpa_auth_sta_init`, `wpa_free_sta_sm` |
| `g_wpa_supp` | 144 | `internal SRAM` / `.bss` | `libwpa_supplicant.a[esp_common.c.obj]` | `esp_supplicant_common_deinit`, `esp_supplicant_common_init` |
