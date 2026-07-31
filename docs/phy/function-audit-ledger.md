# ESP32-S31 complete PHY function-audit ledger

This ledger tracks the strict audit defined in
[audit-method.md](audit-method.md). Inventory completeness and behavioural
audit completeness are separate numbers.

Audit baseline: 2026-07-30.

## Coverage

| Population | Inventoried | Strictly closed | Body audited, proof open | Unreviewed |
| --- | ---: | ---: | ---: | ---: |
| `libphy.a` external code functions | 161 | 112 | 49 | 0 |
| ROM external `phy_*` code functions | 305 | 92 | 8 | 205 |
| **Total** | **466** | **204** | **57** | **205** |

The earlier cold-Wi-Fi analysis is valuable evidence, but its profile-scoped
rows are not promoted into these strict counts until every instruction, branch
and register-relevant child is rechecked under the complete standard.

All 161 archive functions now have complete direct-body instruction and
relocation inventories. The remaining 49 archive rows are open only because a
register-relevant ROM child, target binding, or full Rust trace proof has not
yet been closed; none is still unreviewed at the archive-body level.

## Archive member progress

| Member | Functions | Closed | Body audited/open | Current strict state |
| --- | ---: | ---: | ---: | --- |
| `phy_api.o` | 36 | 1 | 35 | Direct bodies audited; constant RF-calibration version leaf closed, delegated ROM/register proofs open |
| `phy_basic.o` | 1 | 1 | 0 | Complete member body inventory; function is NOT-PORTED |
| `phy_debug.o` | 12 | 12 | 0 | Complete member body inventory; six no-effect and six NOT-PORTED |
| `phy_feature.o` | 8 | 8 | 0 | Complete member body inventory; no Rust-owned entry is MATCHED |
| `phy_hw_freq.o` | 7 | 7 | 0 | Complete member body inventory; one no-effect, two NOT-PORTED, four MISMATCH |
| `phy_i2c.o` | 11 | 9 | 2 | Direct bodies complete; `phy_i2c_init1` and bias target/child proofs open |
| `phy_init.o` | 19 | 16 | 3 | Complete direct-body inventory; seven no-effect, six NOT-PORTED, three MISMATCH, three child proofs open |
| `phy_reg.o` | 12 | 11 | 1 | Direct bodies complete; `phy_open_i2c_xpd_new` ROM/target proof open |
| `phy_rfpll.o` | 4 | 1 | 3 | Direct bodies audited; child proofs and known channel mismatches open |
| `phy_rx_cal.o` | 13 | 13 | 0 | Complete member body inventory; three no-effect, one NOT-PORTED and nine MISMATCH |
| `phy_rx_gain.o` | 6 | 6 | 0 | Complete member body inventory; one no-effect, one NOT-PORTED, four MISMATCH |
| `phy_track.o` | 9 | 9 | 0 | Complete member body inventory; all runtime/background roots are NOT-PORTED |
| `phy_tsens.o` | 5 | 0 | 5 | All direct bodies audited; ROM children/integration proofs open |
| `phy_tx_cal.o` | 10 | 10 | 0 | Complete member body inventory; one no-effect, two NOT-PORTED and seven MISMATCH |
| `phy_tx_gain.o` | 8 | 8 | 0 | Complete member body inventory; three no-effect, two NOT-PORTED, three MISMATCH |
| **Total** | **161** | **112** | **49** | |

The six archive members without external code functions remain in the artifact
inventory but add no function rows.

## ROM progress

| Function | Address | Size | Status | Rust comparison |
| --- | ---: | ---: | --- | --- |
| `phy_chan14_mic_cfg` | `0x2f826144` | `0x42` | BODY-AUDITED | NOT-PORTED; transitive TX-gain child proof remains open |
| `phy_chan14_mic_enable` | `0x2f826186` | `0x26` | BODY-AUDITED | NOT-PORTED; caller and `phy_chan14_mic_cfg` child proof remain open |
| `phy_set_chan_cal_interp` | `0x2f825fac` | `0x4c` | NO-REGISTER-EFFECT | Pure signed three-point channel interpolation |
| `phy_get_data_sat` | `0x2f826024` | `0x10` | NO-REGISTER-EFFECT | Pure signed clamp |
| `phy_txbbgain_to_index` | `0x2f826ac8` | `0x32` | NO-REGISTER-EFFECT | Pure four-value halfword-to-index mapping |
| `phy_get_tx_gain_value` | `0x2f826e3c` | `0x6c` | NO-REGISTER-EFFECT | Pure bounded table search and caller-buffer output |
| `phy_bt_get_tx_gain` | `0x2f826ea8` | `0x150` | NO-REGISTER-EFFECT | Pure 16-entry generator plus optional diagnostic print |
| `phy_wifi_get_tx_gain` | `0x2f826ff8` | `0x102` | NO-REGISTER-EFFECT | Pure 32-entry generator plus optional diagnostic print |
| `phy_write_gain_mem` | `0x2f8274f0` | `0x2a` | MATCHED | Exact three data stores and one command RMW |
| `phy_pbus_force_mode` | `0x2f824102` | `0x90` | MISMATCH | Rust zero-input tail is caller-composed; reached owners omit it or shorten its second delay |
| `phy_pbus_rd_addr` | `0x2f824192` | `0x5c` | NO-REGISTER-EFFECT | Pure complete selector/path address map |
| `phy_pbus_rd_shift` | `0x2f8241ee` | `0x3a` | NO-REGISTER-EFFECT | Pure complete selector/path shift map |
| `phy_pbus_force_test` | `0x2f824228` | `0x42` | MISMATCH | Rust invents a pre-publication busy read and rejection path |
| `phy_pbus_rd` | `0x2f82426a` | `0x3c` | MISMATCH | Selector zero reads `0x201008a0` in ROM but `0x201008a4` in Rust; fallback selectors are rejected |
| `phy_pbus_debugmode` | `0x2f8242a6` | `0x06` | MATCHED | Exact nonzero force-mode wrapper |
| `phy_pbus_workmode` | `0x2f8242ac` | `0x06` | MISMATCH | Exact zero force-mode wrapper inherits incomplete Rust owner composition |
| `phy_pbus_set_rxgain` | `0x2f8242b2` | `0x5c` | MISMATCH | Three tuple values match but force-test traces do not |
| `phy_pbus_xpd_rx_off` | `0x2f82430e` | `0x26` | MISMATCH | Three tuple values match but force-test traces do not |
| `phy_pbus_xpd_rx_on` | `0x2f824334` | `0x62` | MISMATCH | Seven tuple values match but force-test traces do not |
| `phy_pbus_xpd_tx_off` | `0x2f824396` | `0x3a` | MISMATCH | Five tuple values match but force-test traces do not |
| `phy_pbus_set_dco` | `0x2f8243d0` | `0x3e` | MISMATCH | Four halfword tuples match but force-test traces do not |
| `phy_pbus_xpd_tx_on` | `0x2f82440e` | `0x7c` | BODY-AUDITED | Direct body complete; fixed eight-byte object at `0x2f8472d0` is not materialized in the ELF |
| `phy_pbus_clear_reg` | `0x2f824572` | `0x90` | MISMATCH | Twelve tuples and work-mode timing match; force-test traces do not |
| `phy_force_txrx_off` | `0x2f827bb0` | `0x66` | MATCHED | Exact two-phase zero/nonzero RMW and delay traces |
| `phy_set_txclk_en` | `0x2f827cd2` | `0x24` | MATCHED | Exact bits 17:16 replacement |
| `phy_set_rxclk_en` | `0x2f827cf6` | `0x20` | MATCHED | Exact bits 15:14 replacement |
| `phy_abs_temp` | `0x2f825fa2` | `0x0a` | NO-REGISTER-EFFECT | Pure wrapping signed absolute value |
| `phy_linear_to_db` | `0x2f826542` | `0x7c` | NO-REGISTER-EFFECT | Pure logarithmic approximation with exact 16-byte table |
| `phy_iq_est_enable` | `0x2f8289d4` | `0xb4` | MISMATCH | Rust invents an activity-register read on the final ready observation |
| `phy_iq_est_disable` | `0x2f828a88` | `0x2c` | MATCHED | Exact clear/delay/clear trace |
| `phy_dc_iq_est` | `0x2f828ab4` | `0x84` | MISMATCH | Inherits enable mismatch and Rust narrows the full-word divisor input |
| `phy_txiq_set_reg` | `0x2f827c16` | `0x68` | MISMATCH | Rust masks but does not saturate complete signed input values |
| `phy_pbus_rx_dco_cal` | `0x2f828f44` | `0x228` | MISMATCH | Rust fixes threshold/diagnostic inputs and inherits PBus/estimator differences |
| `phy_rxdc_est_min` | `0x2f82916c` | `0x98` | MISMATCH | Selection loop matches but nested estimates add a final activity read |
| `phy_pbus_rx_dco_cal_1step` | `0x2f829204` | `0x3ee` | MISMATCH | Rust unsigned DCO state differs and lower PBus/estimator children mismatch |
| `phy_get_iq_value` | `0x2f8295f2` | `0x36` | NO-REGISTER-EFFECT | Pure exact signed six-bit/seven-bit packed decode |
| `phy_dac_scale_set` | `0x2f82873a` | `0x3c` | MATCHED | Exact zero/nonzero byte images and two fresh RMWs are composed by Rust tone owners |
| `phy_start_tx_tone_step` | `0x2f828776` | `0x102` | MISMATCH | Reached enabled first-path trace matches; full six-word domain and zero/zero branch do not |
| `phy_stop_tx_tone` | `0x2f828878` | `0x30` | MATCHED | Exact two arm clears, stop RMW and two DAC-scale RMWs |
| `phy_fe_reg_update` | `0x2f8288a8` | `0x36` | MISMATCH | Rust implements the three-RMW archive variant but omits the ROM DAC-scale tail |
| `phy_txgain_comp_pacfg_` | `0x2f8288de` | `0x66` | MISMATCH | Zero branch matches; ROM restore bytes differ from the archive/Rust bytes |
| `phy_rfrx_sat_rst` | `0x2f828944` | `0x42` | MATCHED | Exact full store and both zero/nonzero two-RMW branches |
| `phy_force_rx_gain_trig` | `0x2f828986` | `0x4e` | NOT-PORTED | Conditional high-byte write and delayed bit-23 pulse are absent |
| `phy_rxiq_set_reg` | `0x2f827c7e` | `0x54` | MATCHED | Exact saturation and one-RMW gain/phase branches |
| `phy_bb_wdg_test_en` | `0x2f827d16` | `0x26` | NOT-PORTED | Two unrestricted six-input packed stores are absent |
| `phy_noise_floor_auto_set` | `0x2f827d3c` | `0x36` | MATCHED | Exact four fresh-read set-bit RMWs |
| `phy_read_hw_noisefloor` | `0x2f827d72` | `0x1a` | MATCHED | Exact read/transform fused with the matched MAC-facing decode |
| `phy_iq_corr_enable` | `0x2f827d8c` | `0x24` | MATCHED | Exact two fresh-read set-field RMWs |
| `phy_wifi_agc_sat_gain` | `0x2f827db0` | `0x0c` | MATCHED | Exact two unrestricted full-word stores |
| `phy_bbpll_cal` | `0x2f827dbc` | `0x1c` | BODY-AUDITED | Boolean encodings exist; external platform-trait MMIO implementation remains unproved |
| `phy_bbpll_recal` | `0x2f827dd8` | `0x1c` | NOT-PORTED | Contiguous set/read/clear trace, including the discarded middle read, is absent |
| `phy_ant_init` | `0x2f827df4` | `0x44` | MATCHED | Exact three fresh-read masks and field images |
| `phy_disable_agc` | `0x2f827460` | `0x10` | MATCHED | Exact one-RMW disable trace |
| `phy_enable_agc` | `0x2f827470` | `0x28` | MATCHED | Exact disable clear and two-edge pulse |
| `phy_disable_cca` | `0x2f827498` | `0x32` | NOT-PORTED | Missing two-RMW forced CCA image |
| `phy_enable_cca` | `0x2f8274ca` | `0x26` | NOT-PORTED | Missing two-RMW CCA field clears |
| `phy_rx_filter_mode` | `0x2f82751a` | `0x20` | NOT-PORTED | Missing general four-bit RX-filter mode RMW |
| `phy_bb_bss_cbw40_dig` | `0x2f82753a` | `0x16` | BODY-AUDITED | Correct Boolean abstraction; external target RMW proof remains open |
| `phy_mac_tx_chan_offset` | `0x2f827550` | `0x38` | MISMATCH | Byte and reached parent domain match; standalone full-word domain is narrowed |
| `phy_i2cmst_reg_init` | `0x2f8276c4` | `0x22` | BODY-AUDITED | Two abstract platform operations exist; target RMW proof remains open |
| `phy_bt_gain_offset` | `0x2f8276e6` | `0x5a` | NOT-PORTED | Missing four-RMW BT gain-offset publication |
| `phy_mac_enable_bb` | `0x2f827836` | `0x2a` | BODY-AUDITED | Three abstract platform edges exist; target RMW proof remains open |
| `phy_bb_wdg_cfg` | `0x2f827860` | `0x2c` | MATCHED | Exact two fresh-read watchdog RMWs |
| `phy_fe_txrx_reset` | `0x2f82788c` | `0x24` | NOT-PORTED | Missing clear/set pulse of bits 26:25 |
| `phy_set_rx_comp_` | `0x2f8278b0` | `0x28` | MISMATCH | ROM uses `0xeb`; Rust follows archive replacement value `0xed` |
| `phy_wifi_fbw_sel` | `0x2f827e38` | `0x58` | MATCHED | Exact three-RMW zero/nonzero branches |
| `phy_bt_filter_reg` | `0x2f827e90` | `0x34` | MATCHED | Exact three fresh-read RMWs |
| `phy_rx_sense_set` | `0x2f827ec4` | `0x40` | NOT-PORTED | Missing four-RMW general RX-sense control |
| `phy_tx_state_set` | `0x2f827f04` | `0x4c` | NOT-PORTED | Missing four-register TX-state publication |
| `phy_close_pa` | `0x2f827f50` | `0x5e` | NOT-PORTED | Missing both ordered three-RMW PA branches |
| `phy_set_pbus_reg` | `0x2f8280a6` | `0x32` | NOT-PORTED | Six-word save exists, inverse restore does not |
| `phy_wifi_rifs_mode_en` | `0x2f8280d8` | `0x14` | NOT-PORTED | Missing bit-zero replacement |
| `phy_nrx_freq_set` | `0x2f8280ec` | `0x32` | MISMATCH | Two reads/write match; signed division and complete divisor domain do not |
| `phy_fe_adc_on` | `0x2f82811e` | `0x5e` | NOT-PORTED | Missing one-write zero branch and delayed nonzero sequence |
| `phy_force_pwr_index` | `0x2f82817c` | `0x3a` | NOT-PORTED | Missing two-RMW force/index publication |
| `phy_fft_scale_force` | `0x2f8281b6` | `0x3e` | NOT-PORTED | Missing three-RMW scale/force sequence |
| `phy_force_rx_gain` | `0x2f8281f4` | `0x2c` | NOT-PORTED | Missing high-byte gain and bit-23 force RMWs |
| `phy_wifi_enable_set` | `0x2f828220` | `0x18` | BODY-AUDITED | Correct Boolean platform operation; physical target RMW proof remains open |
| `phy_bb_cbw_chan_cfg` | `0x2f828238` | `0x74` | MISMATCH | Byte domain matches; full-word high-path and first OR do not |
| `phy_vht_support` | `0x2f8282ac` | `0x1a` | NOT-PORTED | Missing bit-five replacement |
| `phy_csidump_force_lltf_cfg` | `0x2f8282c6` | `0x1c` | NOT-PORTED | Missing bit-fifteen replacement |
| `phy_hemu_ru26_good_res` | `0x2f8282e2` | `0x24` | NOT-PORTED | Missing ordered set-bit-24/clear-bit-25 RMWs |
| `phy_freq_band_reg_set` | `0x2f828306` | `0x1c` | NOT-PORTED | Missing inverse band bit and VHT tail |
| `phy_sifs_reg_init` | `0x2f828532` | `0x44` | NOT-PORTED | Missing fixed three-RMW initialization |
| `phy_bbtx_outfilter` | `0x2f828576` | `0x3e` | NOT-PORTED | Missing three input-bit replacements |
| `phy_bb_wdt_rst_enable` | `0x2f8285b4` | `0x1c` | MISMATCH | Cold init implements only the set branch |
| `phy_bb_wdt_int_enable` | `0x2f8285d0` | `0x20` | NOT-PORTED | Missing interrupt-enable replacement |
| `phy_bb_wdt_timeout_clear` | `0x2f8285f0` | `0x14` | NOT-PORTED | Missing timeout-clear set edge |
| `phy_bb_wdt_get_status` | `0x2f828604` | `0x0a` | NOT-PORTED | Missing standalone full-word status read |
| `phy_bb_dcmem_clr` | `0x2f8286b4` | `0x1c` | MATCHED | Exact fresh-read set/clear pulse |
| `phy_i2c_txrate_init` | `0x2f8286d0` | `0x38` | MATCHED | Exact two rate RMWs and installed archive gain-restore child |
| `phy_lltf_mask_en` | `0x2f828708` | `0x32` | NOT-PORTED | Missing two fresh input-bit replacements |
| `phy_temp_to_power` | `0x2f825f80` | `0x22` | NO-REGISTER-EFFECT | Pure signed delta/division mapping |
| `phy_byte_to_word` | `0x2f826034` | `0x1e` | NO-REGISTER-EFFECT | Pure four-byte little-endian load |
| `phy_get_rate_fcc_index` | `0x2f826d26` | `0x7e` | NO-REGISTER-EFFECT | Pointer/table reads and caller-buffer stores only |
| `phy_get_chan_target_power` | `0x2f826da4` | `0x98` | NO-REGISTER-EFFECT | Pure 18-byte target clamp and pure optional FCC child |
| `phy_index_to_txbbgain` | `0x2f826afa` | `0x20` | NO-REGISTER-EFFECT | Pure exact five-entry halfword lookup |
| `phy_bt_index_to_bb` | `0x2f826b1a` | `0x1c` | NO-REGISTER-EFFECT | Pure three-value mapping |
| `phy_bt_bb_to_index` | `0x2f826b36` | `0x1c` | NO-REGISTER-EFFECT | Pure inverse mapping with fallback |
| `phy_wifi_get_target_power` | `0x2f8270fa` | `0x22` | NO-REGISTER-EFFECT | Parameter-pointer wrapper around pure target clamp |

All other ROM functions are currently UNREVIEWED for strict-count purposes,
even when they contributed evidence to the earlier profile audit or vendor
defect analysis.

## Closed archive functions

| Member/function | Size | Status | Register result |
| --- | ---: | --- | --- |
| `phy_api.o::phy_get_rf_cal_version` | `0x06` | NO-REGISTER-EFFECT | Returns constant 100; no memory, MMIO or child call |
| `phy_basic.o::phy_chan14_mic_cfg_new` | `0x46` | NOT-PORTED | Vendor RMW of `0x20107400` and subsequent TX-gain regeneration are absent from Rust |
| `phy_debug.o::get_bias_ref_code` | `0x04` | NO-REGISTER-EFFECT | Returns zero without memory, MMIO or child calls |
| `phy_debug.o::get_dc_value` | `0x0e` | NO-REGISTER-EFFECT | Splits one word into two caller-buffer halfwords |
| `phy_debug.o::get_phy_version_str` | `0x4c` | NO-REGISTER-EFFECT | Formats the constant calibration version and fixed build strings |
| `phy_debug.o::phy_cal_print` | `0x5fa` | NOT-PORTED | Missing noise-floor, VDD33, temperature and Wi-Fi gain diagnostic composition |
| `phy_debug.o::phy_debug_print_line` | `0x48` | NO-REGISTER-EFFECT | Only reads parameter halfwords and formats inputs |
| `phy_debug.o::phy_get_vdd33` | `0x88` | NOT-PORTED | Missing exact PBus/I²C/SAR setup, sample and cleanup trace |
| `phy_debug.o::phy_i2c_check` | `0x1f6` | NOT-PORTED | Missing ordered 168-read dump across ten logical PHY-I²C banks |
| `phy_debug.o::phy_pbus_print` | `0xf4` | NOT-PORTED | Missing exact eleven-read selector/path sequence |
| `phy_debug.o::phy_reg_check` | `0x3d2` | NOT-PORTED | Missing ordered 1933-load dump of 21 finite MMIO ranges |
| `phy_debug.o::phy_tx_gain_print` | `0x1ee` | NO-REGISTER-EFFECT | Both table callbacks and all formatting are software-only |
| `phy_debug.o::phy_version_print` | `0x4a` | NO-REGISTER-EFFECT | Reads software version state and formats it |
| `phy_debug.o::rfpll_cap_check` | `0x100` | NOT-PORTED | Missing destructive channel 1-through-14 RFPLL sweep; vendor leaves channel 14 active |
| `phy_feature.o::phy_set_most_tpw_new` | `0x1a` | NOT-PORTED | Required TX-gain regeneration owner is absent |
| `phy_feature.o::phy_get_adc_rand` | `0x170` | NOT-PORTED | ADC/PMU/PBus enable and disable traces are absent |
| `phy_feature.o::phy_internal_delay` | `0x04` | NO-REGISTER-EFFECT | Returns zero and performs no delay |
| `phy_feature.o::phy_ftm_comp` | `0x1e` | NO-REGISTER-EFFECT | Pure three-result parameter lookup |
| `phy_feature.o::phy_11p_set` | `0x12` | NOT-PORTED | Parameter setter and later 802.11p branch are absent |
| `phy_feature.o::phy_freq_mem_backup` | `0x02` | NO-REGISTER-EFFECT | Single `ret` |
| `phy_feature.o::phy_set_rate` | `0x40` | NOT-PORTED | Two runtime PHY-I2C masked writes are absent |
| `phy_feature.o::phy_get_rx_freq` | `0x5e` | NO-REGISTER-EFFECT | Pure packed signed-frequency transform |
| `phy_rfpll.o::phy_chip_set_chan_offset` | `0x7c` | NOT-PORTED | Runtime frequency-offset correction and RFPLL retune are absent |
| `phy_hw_freq.o::phy_freq_offset_set` | `0x02` | NO-REGISTER-EFFECT | Single `ret`; no memory or register access |
| `phy_hw_freq.o::phy_freq_get_i2c_data` | `0x208` | MISMATCH | Rust fixes the descriptor count and narrows raw `phy_param[0x1af]` to `bool` |
| `phy_hw_freq.o::phy_freq_i2c_data_write` | `0x32` | MISMATCH | Rust implements only input `1`; vendor input zero suppresses memory writes |
| `phy_hw_freq.o::phy_bt_txpwr_freq` | `0x84` | NOT-PORTED | Missing 85-entry BT power-delta memory publication |
| `phy_hw_freq.o::phy_get_rf_freq_cap` | `0x78` | NOT-PORTED | Missing RFPLL program/calibrate plus two-byte cap acquisition contract |
| `phy_hw_freq.o::phy_get_rf_freq_init` | `0x1d8` | MISMATCH | Rust fixes count 85 and offset zero; vendor accepts general count and signed offset |
| `phy_hw_freq.o::phy_set_chan_freq_hw_init` | `0x28` | MISMATCH | Default profile matches, but final descriptor inherits raw-byte-to-Boolean mismatch |
| `phy_i2c.o::phy_get_i2c_read_mask_new` | `0x24` | MISMATCH | Rust omits five nonzero vendor table inputs below block `0x61` |
| `phy_i2c.o::phy_get_i2c_hostid_new` | `0x44` | MISMATCH | Rust rejects arbitrary inputs that vendor maps to host zero plus a host-map RMW |
| `phy_i2c.o::phy_i2c_enter_critical` | `0x02` | NO-REGISTER-EFFECT | Weak single-`ret` definition |
| `phy_i2c.o::phy_i2c_exit_critical` | `0x02` | NO-REGISTER-EFFECT | Weak single-`ret` definition |
| `phy_i2c.o::phy_i2c_init2` | `0x2b8` | NOT-PORTED | Missing 22-pair/44-command parallel PHY-I2C initialization |
| `phy_i2c.o::phy_get_i2c_data` | `0x02` | NO-REGISTER-EFFECT | Single `ret` |
| `phy_i2c.o::phy_i2c_master_cmd_mem_init` | `0x5be` | MATCHED | Exact 45 encoded full-word command-RAM stores |
| `phy_i2c.o::phy_i2c_master_mem_cfg` | `0x20` | NO-REGISTER-EFFECT | Only writes a six-byte caller buffer |
| `phy_i2c.o::phy_i2c_master_command_mem_cfg` | `0x2c` | NO-REGISTER-EFFECT | Only writes caller buffers |
| `phy_init.o::phy_get_xtal_freq` | `0x40` | MISMATCH | Rust fixes 40 MHz; vendor publishes distinct parameter and MMIO images for returned 26/32 MHz values |
| `phy_init.o::phy_wakeup_init` | `0x188` | NOT-PORTED | Missing complete wakeup reset, parallel-I2C, register restore and channel restore lifecycle |
| `phy_init.o::phy_xpd_rf_new` | `0x62` | NOT-PORTED | Missing AGC disable, I2C power-down, two system RMWs and clock-close tail |
| `phy_init.o::phy_close_rf` | `0x96` | NOT-PORTED | Missing guarded temperature sample and registered-radio shutdown composition |
| `phy_init.o::phy_get_romfunc_addr` | `0x98` | NO-REGISTER-EFFECT | Only installs software callback and parameter pointers |
| `phy_init.o::phy_close_fe_bb_clk` | `0x20` | NOT-PORTED | Missing exact three-write FE/BB clock shutdown |
| `phy_init.o::phy_get_chip_version` | `0x3c` | NOT-PORTED | Missing two eFuse reads and derived parameter-byte publication |
| `phy_init.o::phy_i2c_read_check` | `0x60` | NOT-PORTED | Missing finite 100-read PHY-I2C diagnostic capture |
| `phy_init.o::phy_bb_init` | `0x16a` | MISMATCH | Rust omits mandatory BT gain initialization and inherits reached RX/TX calibration defects |
| `phy_init.o::register_chipv7_phy_init_param` | `0x94` | NO-REGISTER-EFFECT | Exact 71-byte init-profile mapping is present in Rust |
| `phy_init.o::phy_get_rom_ver` | `0x0c` | NO-REGISTER-EFFECT | Returns the low nibble of a software ROM-version word |
| `phy_init.o::phy_rfcal_data_sub_new` | `0x64` | NO-REGISTER-EFFECT | Exact 508-byte little-endian calibration payload backup/recovery transform |
| `phy_init.o::phy_rf_cal_data_recovery_new` | `0x0a` | NO-REGISTER-EFFECT | Pure recovery wrapper |
| `phy_init.o::phy_rf_cal_data_backup_new` | `0x16` | NO-REGISTER-EFFECT | Pure backup wrapper returning zero |
| `phy_init.o::phy_rfcal_data_check_new` | `0x7e` | NO-REGISTER-EFFECT | Exact header identity and 130-word complemented-checksum geometry |
| `phy_init.o::register_chipv7_phy` | `0x1e6` | MISMATCH | Rust does not compose record check/recovery/backup modes and inherits baseband mismatches |
| `phy_reg.o::phy_set_rx_comp_new` | `0x28` | MATCHED | Exact two-RMW compensation update |
| `phy_reg.o::phy_fe_reg_update` | `0x32` | MATCHED | Exact three fresh-read RMWs |
| `phy_reg.o::phy_set_ftm_en` | `0x14` | MISMATCH | Rust implements only the reached set-to-one path |
| `phy_reg.o::phy_start_tx_tone_step_new` | `0xc2` | MISMATCH | Rust fixes the entire second tone path to zero and narrows selectors |
| `phy_reg.o::phy_stop_tx_tone_new` | `0x2c` | NOT-PORTED | No exact three-RMW archive stop operation |
| `phy_reg.o::phy_txgain_comp_pacfg_new` | `0x54` | MATCHED | Both full-zero and four-RMW restore branches match |
| `phy_reg.o::phy_bb_txpwr_track` | `0xf4` | MISMATCH | Fourteen edges match Boolean profiles; arbitrary vendor low-bit input is narrowed |
| `phy_reg.o::phy_iccfr_en` | `0x2c` | NOT-PORTED | Missing enable/disable RMW |
| `phy_reg.o::phy_force_iccfr` | `0x80` | NOT-PORTED | Missing five-RMW force image and ICCFR tail |
| `phy_reg.o::phy_config_hccfr` | `0x38` | NOT-PORTED | Missing two HCCFR field updates |
| `phy_reg.o::phy_dc_mem_clr` | `0x1c` | MATCHED | Exact fresh-read set/clear pulse |
| `phy_rx_cal.o::phy_pbus_rx_dco_cal_1step_new` | `0x4a2` | MISMATCH | Rust treats caller DCO halfwords as unsigned and narrows estimator control; vendor correction arithmetic begins with signed halfword loads |
| `phy_rx_cal.o::phy_set_lb_txiq_new` | `0x32` | MISMATCH | Rust fails to saturate decoded gain `-32` and phase `-64` before their field writes |
| `phy_rx_cal.o::phy_set_rx_gain_cal_iq_new` | `0x25e` | MISMATCH | Rust fixes the cold zero/`0x80` profile and omits the nonzero first-input I2C save/clear/restore trace |
| `phy_rx_cal.o::phy_bt_rx_mx_dgain` | `0x2a` | NO-REGISTER-EFFECT | Pure eleven-value mixer-digital-gain lookup with out-of-range fallback 7 |
| `phy_rx_cal.o::phy_rxdc_fine_delta` | `0x110` | MISMATCH | Direct six-code graph matches but nested PBus/estimator children add reads |
| `phy_rx_cal.o::phy_rxdc_est_delta` | `0xda` | MISMATCH | Direct low/high graph matches but nested PBus/estimator children add reads |
| `phy_rx_cal.o::phy_set_rx_gain_cal_dc_new` | `0x2cc` | MISMATCH | Both bank algorithms exist, but reached Rust cleanup holds the second PBus pulse for 1 µs instead of 2 µs |
| `phy_rx_cal.o::phy_rfrx_gain_index_new` | `0x74` | NO-REGISTER-EFFECT | Pure search of the exact eight- or eleven-halfword calibration table |
| `phy_rx_cal.o::phy_xtal_duty_cal` | `0x392` | MISMATCH | Candidate search matches but reached RX-DCO/PBus children add successful-path reads |
| `phy_rx_cal.o::phy_xtal_duty_cal_init` | `0x74` | MISMATCH | Exact two-frequency wrapper inherits both child mismatches |
| `phy_rx_cal.o::phy_get_xtal_duty` | `0x36` | NO-REGISTER-EFFECT | Exact pure frequency boundary and wrapping-subtraction selector |
| `phy_rx_cal.o::phy_xtal_duty_set` | `0x3e` | NOT-PORTED | Missing runtime I2C bit-clear plus full duty-byte write for a supplied frequency |
| `phy_rx_cal.o::phy_check_rx_sat` | `0x76` | MISMATCH | Sampling matches, but Rust omits the conditional work-mode settle/pulse tail |
| `phy_rx_gain.o::phy_get_rxbb_dc_new` | `0x2e` | NO-REGISTER-EFFECT | Pure clamped two-halfword wrapping addition |
| `phy_rx_gain.o::phy_wr_rx_gain_mem_new` | `0x1c6` | MISMATCH | Fixed Rust banks narrow the vendor domain, and each reached PBus pulse is held for 1 µs instead of 2 µs |
| `phy_rx_gain.o::phy_rxiq_cal_init` | `0x198` | MISMATCH | Rust omits the nonzero first-input PBus branch and nonzero third-input skip-cleanup branch |
| `phy_rx_gain.o::phy_rx_table_init` | `0x7c` | MISMATCH | Vendor stores `0x4f4f` at parameter offset `0x120`; Rust updates only byte `0x120` and uses stale byte `0x121` |
| `phy_rx_gain.o::phy_set_rx_gain_table` | `0x28a` | MISMATCH | Rust uses guard word `phy_param+0xb4` instead of vendor `+0xa4`, omits a cached-path read, and inherits the pulse-delay mismatch |
| `phy_rx_gain.o::phy_rx_table_track` | `0xc0` | NOT-PORTED | Missing temperature-threshold table regeneration, BB enable and channel retune |
| `phy_tx_gain.o::phy_bt_chan_pwr_interp` | `0x50` | NO-REGISTER-EFFECT | Pure signed three-point BT channel interpolation; no Rust equivalent |
| `phy_tx_gain.o::phy_set_tx_gain_mem_new` | `0x130` | MISMATCH | Rust fixes a 32-entry Wi-Fi image and does not implement the full bank/count/table domain |
| `phy_tx_gain.o::phy_wifi_set_tx_gain_new` | `0x72` | MISMATCH | Reached Wi-Fi profile matches, but the standalone vendor domain is narrowed by the channel owner |
| `phy_tx_gain.o::phy_bt_get_tx_tab_new` | `0xa8` | NO-REGISTER-EFFECT | Pure BT table wrapper is absent from Rust |
| `phy_tx_gain.o::phy_bt_set_tx_gain_new` | `0x66` | NOT-PORTED | Missing 16-entry BT gain-memory publication |
| `phy_tx_gain.o::phy_bt_tx_gain_init` | `0x5a` | NOT-PORTED | Missing mandatory six-child BT calibration and publication root |
| `phy_tx_gain.o::phy_wifi_get_tx_tab_new` | `0xa0` | NO-REGISTER-EFFECT | Pure Wi-Fi table wrapper; Rust calculation matches and omits diagnostic printing |
| `phy_tx_gain.o::phy_set_tx_cfr_mem` | `0x76` | MISMATCH | Exact for count 32; Rust omits the other finite vendor counts |
| `phy_track.o::phy_txpwr_cal_track_new` | `0x142` | NOT-PORTED | Missing thresholded Wi-Fi/BT correction, BBPLL bracket and gain regeneration |
| `phy_track.o::phy_tx_i2c_track` | `0x14a` | NOT-PORTED | Missing four-band temperature state and paired TXRF masked writes |
| `phy_track.o::phy_bt_track_tx_power_new` | `0x0e` | NOT-PORTED | Missing BT wrapper and parent graph |
| `phy_track.o::phy_wifi_track_tx_power_new` | `0x0e` | NOT-PORTED | Missing runtime Wi-Fi wrapper and parent graph |
| `phy_track.o::phy_param_track` | `0x68` | NOT-PORTED | Missing gated temperature/RFPLL/Wi-Fi/BT tracking composition |
| `phy_track.o::phy_cal_param_track` | `0x25a` | NOT-PORTED | Missing three-section runtime recalibration and unconditional PA-compensation tail |
| `phy_track.o::phy_param_track_tot` | `0xae` | NOT-PORTED | Missing prior-sample background dispatch for Wi-Fi and BT |
| `phy_track.o::phy_bt_track_pll_cap` | `0x0c` | NOT-PORTED | Missing `(0,1)` background wrapper |
| `phy_track.o::phy_tx_pwctrl_background` | `0x0c` | NOT-PORTED | Missing `(1,0)` background wrapper |
| `phy_tx_cal.o::phy_bt_txdc_cal_new` | `0xfe` | NOT-PORTED | Missing guarded three-row BT TXDC calibration |
| `phy_tx_cal.o::phy_txiq_cal_init` | `0x14c` | MISMATCH | Reached TXIQ graph inherits the shared 1 µs versus vendor 2 µs PBus pulse |
| `phy_tx_cal.o::phy_txdc_cal_init` | `0x110` | MISMATCH | Cold four-argument profile matches; general PBus/force inputs are absent |
| `phy_tx_cal.o::phy_txdc_cal_pwdet_new` | `0x3b4` | MISMATCH | Search/delay/sample graph matches, but every PBus command inherits the additional Rust pre-publication busy read |
| `phy_tx_cal.o::phy_txdc_cal_pwdet_init` | `0x208` | MISMATCH | BT and skip-cleanup inputs are absent; reached cleanup pulse is 1 µs instead of 2 µs |
| `phy_tx_cal.o::phy_tx_cap_init` | `0xe6` | MISMATCH | Three-channel algorithm matches, but reached shared cleanup pulse is too short |
| `phy_tx_cal.o::phy_tx_pwctrl_init_cal_new` | `0x18c` | MISMATCH | Rust implements Wi-Fi mode only; vendor also implements the distinct BT mode |
| `phy_tx_cal.o::phy_tx_pwctrl_init` | `0x9a` | MISMATCH | Wi-Fi body matches except for the reached shared PBus pulse timing |
| `phy_tx_cal.o::phy_tx_atten_comp` | `0x16` | NO-REGISTER-EFFECT | Two wrapping caller-buffer byte additions |
| `phy_tx_cal.o::phy_bt_tx_pwctrl_init` | `0x1ae` | NOT-PORTED | Missing guarded BBTOP/PBus/BT power-control calibration and restore |

Detailed proof:
[libphy `phy_basic.o`](audit/libphy-phy_basic.md) and
[libphy `phy_debug.o`](audit/libphy-phy_debug.md),
[libphy `phy_feature.o`](audit/libphy-phy_feature.md), and
[libphy `phy_hw_freq.o`](audit/libphy-phy_hw_freq.md), and
[libphy `phy_i2c.o`](audit/libphy-phy_i2c.md), and
[libphy `phy_init.o`](audit/libphy-phy_init.md), and
[libphy `phy_reg.o`](audit/libphy-phy_reg.md), and
[libphy `phy_rx_cal.o`](audit/libphy-phy_rx_cal.md), and
[libphy `phy_rx_gain.o`](audit/libphy-phy_rx_gain.md), and
[libphy `phy_tx_gain.o`](audit/libphy-phy_tx_gain.md), and
[libphy `phy_track.o`](audit/libphy-phy_track.md), and
[libphy `phy_tx_cal.o`](audit/libphy-phy_tx_cal.md).

Closed ROM TX-gain leaves:
[revision-zero ROM TX-gain leaves](audit/rom-tx-gain-leaves.md).

ROM PBus core:
[revision-zero ROM PBus core](audit/rom-pbus-core.md).

ROM DC/IQ and RX-DCO cluster:
[revision-zero ROM DC/IQ and RX-DCO](audit/rom-dc-iq-rx-dco.md).

ROM tone, front-end and AGC leaves:
[revision-zero ROM tone, front-end and AGC leaves](audit/rom-tone-fe-agc-leaves.md).

ROM RXIQ, noise-floor and BBPLL controls:
[revision-zero ROM RXIQ, noise-floor and BBPLL controls](audit/rom-rxiq-bbpll-control-leaves.md).

ROM AGC, CCA and channel controls:
[revision-zero ROM AGC, CCA and channel controls](audit/rom-agc-cca-channel-leaves.md).

ROM FBW, state and force controls:
[revision-zero ROM FBW, state and force controls](audit/rom-fbw-force-control-leaves.md).

ROM CBW, feature and watchdog controls:
[revision-zero ROM CBW, feature and watchdog controls](audit/rom-cbw-feature-watchdog-leaves.md).

ROM pure power/mapping helpers:
[revision-zero ROM pure power/mapping helpers](audit/rom-pure-power-mapping-leaves.md).

Body-audited archive members:
[libphy `phy_api.o`](audit/libphy-phy_api.md),
[libphy `phy_tsens.o`](audit/libphy-phy_tsens.md), and
[libphy `phy_rfpll.o`](audit/libphy-phy_rfpll.md). The two open
`phy_i2c.o` functions are documented on its member page above.
`phy_open_i2c_xpd_new` is documented on the `phy_reg.o` page above.
