/*
 * ESP32-S31 ECO0 functions below are exported by esp-rom-sys as absolute
 * linker-script assignments. LLD applies --wrap to those assignments too,
 * which would replace the Rust __wrap_* definition with the ROM address.
 *
 * Load this fragment after the esp-rom-sys ROM fragments. Calls through the
 * public name enter Rust, while pre-strict delegation uses the pinned ROM
 * address through __real_*. No ROM or vendor archive bytes are modified.
 */
__real_ieee80211_set_tx_pti = 0x2f800cb8;
ieee80211_set_tx_pti = __wrap_ieee80211_set_tx_pti;

__real_ieee80211_search_node = 0x2f800ca8;
ieee80211_search_node = __wrap_ieee80211_search_node;

/* The ROM linker fragment exports ieee80211_set_tx_desc as an absolute
 * assignment and consequently captures GNU --wrap's generated __wrap symbol.
 * Route the public name through a unique Rust boundary and retain the pinned
 * ROM address only for calls made before strict ownership handoff. */
__real_ieee80211_set_tx_desc = 0x2f800c98;
EXTERN(wifi_strict_ieee80211_set_tx_desc);
ieee80211_set_tx_desc = wifi_strict_ieee80211_set_tx_desc;
ASSERT(ieee80211_set_tx_desc == wifi_strict_ieee80211_set_tx_desc,
       "ESP32-S31 ieee80211_set_tx_desc Rust boundary is inactive");

/* ESF alignment is another absolute ROM export. The strict Rust leaf keeps
 * the original address only for pre-handoff delegation. */
__real_ieee80211_align_eb = 0x2f800c7c;
EXTERN(wifi_strict_ieee80211_align_eb);
ieee80211_align_eb = wifi_strict_ieee80211_align_eb;
ASSERT(ieee80211_align_eb == wifi_strict_ieee80211_align_eb,
       "ESP32-S31 ieee80211_align_eb Rust boundary is inactive");

/* Like ieee80211_post_hmac_tx below, this ROM export captures the generated
 * __wrap symbol. Use a unique Rust boundary and keep the pinned ROM leaf only
 * for pre-strict cold-init delegation. */
__real_ieee80211_crypto_encap = 0x2f800cac;
EXTERN(wifi_strict_ieee80211_crypto_encap);
ieee80211_crypto_encap = wifi_strict_ieee80211_crypto_encap;
ASSERT(ieee80211_crypto_encap == wifi_strict_ieee80211_crypto_encap,
       "ESP32-S31 ieee80211_crypto_encap Rust boundary is inactive");

/* GNU --wrap cannot be used for this ROM export: it aliases the generated
 * __wrap symbol itself to 0x2f800cc0. Keep a separate pre-strict entry and
 * route the public name to the uniquely named Rust runtime boundary. */
__real_ieee80211_post_hmac_tx = 0x2f800cc0;
EXTERN(wifi_strict_ieee80211_post_hmac_tx);
ieee80211_post_hmac_tx = wifi_strict_ieee80211_post_hmac_tx;
ASSERT(ieee80211_post_hmac_tx == wifi_strict_ieee80211_post_hmac_tx,
       "ESP32-S31 ieee80211_post_hmac_tx Rust boundary is inactive");

__real_ets_delay_us = 0x2f80003c;
ets_delay_us = __wrap_ets_delay_us;

__real_esf_buf_alloc = 0x2f800d1c;
esf_buf_alloc = __wrap_esf_buf_alloc;

__real_esf_buf_recycle = 0x2f800d24;
esf_buf_recycle = __wrap_esf_buf_recycle;

/* This public PP leaf is also an absolute ROM export. The pinned libpp.a
 * reference body proves its complete fourteen-byte field transform, but GNU
 * --wrap would rename the ROM assignment itself. Route every public call to
 * a unique Rust owner and retain the ROM address only for cold delegation. */
__real_ppRecycleRxPkt = 0x2f800f98;
EXTERN(wifi_strict_pp_recycle_rx_pkt);
ppRecycleRxPkt = wifi_strict_pp_recycle_rx_pkt;
ASSERT(ppRecycleRxPkt == wifi_strict_pp_recycle_rx_pkt,
       "ESP32-S31 ppRecycleRxPkt Rust boundary is inactive");

/* libpp.a[if_hwctrl.o]::esp_wifi_internal_free_rx_buffer is only an
 * eight-byte tail-call to ppRecycleRxPkt. Publish the Rust owner directly so
 * the archive object is not extracted and no redundant vendor boundary
 * remains on the network-buffer Drop path. */
EXTERN(wifi_strict_esp_wifi_internal_free_rx_buffer);
esp_wifi_internal_free_rx_buffer =
    wifi_strict_esp_wifi_internal_free_rx_buffer;
ASSERT(esp_wifi_internal_free_rx_buffer ==
           wifi_strict_esp_wifi_internal_free_rx_buffer,
       "ESP32-S31 free RX buffer Rust boundary is inactive");
/* The RX callback is published through a mutable WDEV table rather than a
 * normal final-link relocation. Keep its unique symbol so the strict audit
 * can prove both the publication target and its internal-SRAM placement. */
EXTERN(wifi_strict_lmac_rx_done);

__real_wDev_AppendRxBlocks = 0x2f8010c4;
wDev_AppendRxBlocks = __wrap_wDev_AppendRxBlocks;

/* wDev_DiscardFrame is the adjacent absolute ROM export. GNU --wrap would
 * capture the generated __wrap symbol at 0x2f8010c8 and discard the Rust
 * body. Retain that address only for cold delegation and publish the unique
 * SRAM Rust ownership boundary after all ROM fragments. */
__real_wDev_DiscardFrame = 0x2f8010c8;
EXTERN(wifi_strict_wdev_discard_frame);
wDev_DiscardFrame = wifi_strict_wdev_discard_frame;
ASSERT(wDev_DiscardFrame == wifi_strict_wdev_discard_frame,
       "ESP32-S31 wDev_DiscardFrame Rust boundary is inactive");

/* The remaining successful-RX aggregate is an absolute ROM export too.
 * Retain its pinned body as the current protocol oracle, but force every
 * caller through the SRAM Rust metadata decoder so each subsequently ported
 * routing branch has an explicit, measurable boundary. */
__real_wDev_ProcessRxSucData = 0x2f8010f4;
EXTERN(wifi_strict_wdev_process_rx_success_data);
wDev_ProcessRxSucData = wifi_strict_wdev_process_rx_success_data;
ASSERT(wDev_ProcessRxSucData == wifi_strict_wdev_process_rx_success_data,
       "ESP32-S31 wDev_ProcessRxSucData Rust boundary is inactive");

__real_hal_mac_get_txq_state = 0x2f800d3c;
hal_mac_get_txq_state = __wrap_hal_mac_get_txq_state;

__real_hal_mac_get_txq_complete = 0x2f800d44;
hal_mac_get_txq_complete = __wrap_hal_mac_get_txq_complete;

__real_lmacTxDone = 0x2f800dec;
lmacTxDone = __wrap_lmacTxDone;

/* TXOP admission is a three-slot state machine, not a hardware leaf. Route
 * both archive callers and WDEV callback-table relocations to the Rust owner;
 * this also lets section GC discard libpp.a[lmac.o]'s private three-byte
 * g_txop_queue_status object. */
EXTERN(wifi_strict_lmac_request_txop_queue);
lmacRequestTxopQueue = wifi_strict_lmac_request_txop_queue;
ASSERT(lmacRequestTxopQueue == wifi_strict_lmac_request_txop_queue,
       "ESP32-S31 lmacRequestTxopQueue Rust boundary is inactive");

EXTERN(wifi_strict_lmac_release_txop_queue);
lmacReleaseTxopQueue = wifi_strict_lmac_release_txop_queue;
ASSERT(lmacReleaseTxopQueue == wifi_strict_lmac_release_txop_queue,
       "ESP32-S31 lmacReleaseTxopQueue Rust boundary is inactive");

/* The rev0 ROM TSF leaf is a complete three-register latch/read/unlatch
 * sequence. Keep its address as a differential oracle and route all runtime
 * callers to the equivalent SRAM Rust radio-HAL boundary. */
__real_hal_get_tsf_time = 0x2f82b9f8;
EXTERN(wifi_strict_hal_get_tsf_time);
hal_get_tsf_time = wifi_strict_hal_get_tsf_time;
ASSERT(hal_get_tsf_time == wifi_strict_hal_get_tsf_time,
       "ESP32-S31 hal_get_tsf_time Rust boundary is inactive");

/* The rev0 ROM RX-tail leaf only joins two disjoint MMIO address fields.
 * Retain the original address as an oracle and make the complete finite Rust
 * implementation the only runtime definition. */
__real_hal_mac_rx_get_last_dscr = 0x2f8386a2;
EXTERN(wifi_strict_hal_mac_rx_get_last_dscr);
hal_mac_rx_get_last_dscr = wifi_strict_hal_mac_rx_get_last_dscr;
ASSERT(hal_mac_rx_get_last_dscr == wifi_strict_hal_mac_rx_get_last_dscr,
       "ESP32-S31 hal_mac_rx_get_last_dscr Rust boundary is inactive");

/* Complete finite MAC-register leaves recovered from the pinned libpp.a.
 * None contains a call, loop, delay, allocation, or data-symbol reference. */
EXTERN(wifi_strict_hal_mac_tx_set_cca);
hal_mac_tx_set_cca = wifi_strict_hal_mac_tx_set_cca;
ASSERT(hal_mac_tx_set_cca == wifi_strict_hal_mac_tx_set_cca,
       "ESP32-S31 hal_mac_tx_set_cca Rust boundary is inactive");

EXTERN(wifi_strict_hal_mac_is_txq_valid);
hal_mac_is_txq_valid = wifi_strict_hal_mac_is_txq_valid;
ASSERT(hal_mac_is_txq_valid == wifi_strict_hal_mac_is_txq_valid,
       "ESP32-S31 hal_mac_is_txq_valid Rust boundary is inactive");

EXTERN(wifi_strict_hal_mac_set_txq_invalid);
hal_mac_set_txq_invalid = wifi_strict_hal_mac_set_txq_invalid;
ASSERT(hal_mac_set_txq_invalid == wifi_strict_hal_mac_set_txq_invalid,
       "ESP32-S31 hal_mac_set_txq_invalid Rust boundary is inactive");

EXTERN(wifi_strict_hal_mac_txq_disable);
hal_mac_txq_disable = wifi_strict_hal_mac_txq_disable;
ASSERT(hal_mac_txq_disable == wifi_strict_hal_mac_txq_disable,
       "ESP32-S31 hal_mac_txq_disable Rust boundary is inactive");

EXTERN(wifi_strict_hal_mac_set_csi_cbw);
hal_mac_set_csi_cbw = wifi_strict_hal_mac_set_csi_cbw;
ASSERT(hal_mac_set_csi_cbw == wifi_strict_hal_mac_set_csi_cbw,
       "ESP32-S31 hal_mac_set_csi_cbw Rust boundary is inactive");

/* Complete finite MAC address and RX-policy leaves recovered from
 * libpp.a[if_hwctrl.o] and libpp.a[hal_mac.o]. All retained state is MMIO;
 * no vendor object, call, loop, wait, delay, or allocation remains. */
EXTERN(wifi_strict_ic_set_mac);
ic_set_mac = wifi_strict_ic_set_mac;
ASSERT(ic_set_mac == wifi_strict_ic_set_mac,
       "ESP32-S31 ic_set_mac Rust boundary is inactive");

EXTERN(wifi_strict_ic_set_rx_policy);
ic_set_rx_policy = wifi_strict_ic_set_rx_policy;
ASSERT(ic_set_rx_policy == wifi_strict_ic_set_rx_policy,
       "ESP32-S31 ic_set_rx_policy Rust boundary is inactive");

EXTERN(wifi_strict_ic_set_rx_policy_ubssid_check);
ic_set_rx_policy_ubssid_check =
    wifi_strict_ic_set_rx_policy_ubssid_check;
ASSERT(ic_set_rx_policy_ubssid_check ==
           wifi_strict_ic_set_rx_policy_ubssid_check,
       "ESP32-S31 ic_set_rx_policy_ubssid_check Rust boundary is inactive");

/* Management-frame allocation is one bounded size-class selection over the
 * already Rust-owned ESF management pool. */
EXTERN(wifi_strict_ieee80211_getmgtframe);
ieee80211_getmgtframe = wifi_strict_ieee80211_getmgtframe;
ASSERT(ieee80211_getmgtframe == wifi_strict_ieee80211_getmgtframe,
       "ESP32-S31 ieee80211_getmgtframe Rust boundary is inactive");

/* WPA2/WPA3 hardware-key publication has one Rust-owned logical ledger and
 * finite key-table MMIO leaves. */
EXTERN(wifi_strict_ic_set_key);
ic_set_key = wifi_strict_ic_set_key;
ASSERT(ic_set_key == wifi_strict_ic_set_key,
       "ESP32-S31 ic_set_key Rust boundary is inactive");

EXTERN(wifi_strict_ic_del_key);
ic_del_key = wifi_strict_ic_del_key;
ASSERT(ic_del_key == wifi_strict_ic_del_key,
       "ESP32-S31 ic_del_key Rust boundary is inactive");

EXTERN(wifi_strict_wdev_insert_key_entry);
wDev_Insert_KeyEntry = wifi_strict_wdev_insert_key_entry;
ASSERT(wDev_Insert_KeyEntry == wifi_strict_wdev_insert_key_entry,
       "ESP32-S31 wDev_Insert_KeyEntry Rust boundary is inactive");

/* Complete finite PHY-register leaves recovered from libphy.a[phy_reg.o].
 * Keep the exact read/modify/write order, but make Rust the only linked
 * runtime owner; neither vendor body contains a call or hidden state access. */
EXTERN(wifi_strict_phy_set_rx_comp_new);
phy_set_rx_comp_new = wifi_strict_phy_set_rx_comp_new;
ASSERT(phy_set_rx_comp_new == wifi_strict_phy_set_rx_comp_new,
       "ESP32-S31 phy_set_rx_comp_new Rust boundary is inactive");

EXTERN(wifi_strict_phy_dc_mem_clr);
phy_dc_mem_clr = wifi_strict_phy_dc_mem_clr;
ASSERT(phy_dc_mem_clr == wifi_strict_phy_dc_mem_clr,
       "ESP32-S31 phy_dc_mem_clr Rust boundary is inactive");

/* The final TX-gain-table root is a bounded scalar transform followed by four
 * MMIO writes per entry. Rust owns the complete transform, including the two
 * former ROM helpers; no ROM/vendor call remains under this public symbol. */
EXTERN(wifi_strict_phy_set_tx_gain_mem_new);
phy_set_tx_gain_mem_new = wifi_strict_phy_set_tx_gain_mem_new;
ASSERT(phy_set_tx_gain_mem_new == wifi_strict_phy_set_tx_gain_mem_new,
       "ESP32-S31 phy_set_tx_gain_mem_new Rust boundary is inactive");

/* Complete finite phy_init.o parameter-transfer bodies. These still address
 * the cold vendor phy_param symbol, but all bulk mutations and calibration
 * serialization are now explicit Rust transforms. */
EXTERN(wifi_strict_register_chipv7_phy_init_param);
register_chipv7_phy_init_param =
    wifi_strict_register_chipv7_phy_init_param;
ASSERT(register_chipv7_phy_init_param ==
           wifi_strict_register_chipv7_phy_init_param,
       "ESP32-S31 PHY init parameter Rust boundary is inactive");

EXTERN(wifi_strict_phy_rfcal_data_sub_new);
phy_rfcal_data_sub_new = wifi_strict_phy_rfcal_data_sub_new;
ASSERT(phy_rfcal_data_sub_new == wifi_strict_phy_rfcal_data_sub_new,
       "ESP32-S31 PHY calibration transfer Rust boundary is inactive");

EXTERN(wifi_strict_phy_rf_cal_data_backup_new);
phy_rf_cal_data_backup_new = wifi_strict_phy_rf_cal_data_backup_new;
ASSERT(phy_rf_cal_data_backup_new ==
           wifi_strict_phy_rf_cal_data_backup_new,
       "ESP32-S31 PHY calibration backup Rust boundary is inactive");

EXTERN(wifi_strict_phy_rf_cal_data_recovery_new);
phy_rf_cal_data_recovery_new =
    wifi_strict_phy_rf_cal_data_recovery_new;
ASSERT(phy_rf_cal_data_recovery_new ==
           wifi_strict_phy_rf_cal_data_recovery_new,
       "ESP32-S31 PHY calibration recovery Rust boundary is inactive");

/* Complete bounded calibration-record transform. Rust refreshes the fixed
 * version/eFuse identity prefix and writes or validates the checksum without
 * calling the former ROM header and byte-to-word helpers. */
EXTERN(wifi_strict_phy_rfcal_data_check_new);
phy_rfcal_data_check_new = wifi_strict_phy_rfcal_data_check_new;
ASSERT(phy_rfcal_data_check_new ==
           wifi_strict_phy_rfcal_data_check_new,
       "ESP32-S31 PHY calibration record Rust boundary is inactive");

/* The S31 crystal is fixed at 40 MHz. Publish the exact phy_param code and
 * six-bit hardware divisor without the former clock-query call. */
EXTERN(wifi_strict_phy_get_xtal_freq);
phy_get_xtal_freq = wifi_strict_phy_get_xtal_freq;
ASSERT(phy_get_xtal_freq == wifi_strict_phy_get_xtal_freq,
       "ESP32-S31 PHY crystal Rust boundary is inactive");

/* Complete three-register FE/baseband clock-close transaction. */
EXTERN(wifi_strict_phy_close_fe_bb_clk);
phy_close_fe_bb_clk = wifi_strict_phy_close_fe_bb_clk;
ASSERT(phy_close_fe_bb_clk == wifi_strict_phy_close_fe_bb_clk,
       "ESP32-S31 PHY FE/baseband clock Rust boundary is inactive");

/* Complete four-register FE/baseband clock-open transaction. */
EXTERN(wifi_strict_phy_open_fe_bb_clk);
phy_open_fe_bb_clk = wifi_strict_phy_open_fe_bb_clk;
ASSERT(phy_open_fe_bb_clk == wifi_strict_phy_open_fe_bb_clk,
       "ESP32-S31 PHY FE/baseband clock-open Rust boundary is inactive");

/* Complete two-branch BBPLL calibration-control register transaction. */
EXTERN(wifi_strict_phy_bbpll_cal);
phy_bbpll_cal = wifi_strict_phy_bbpll_cal;
ASSERT(phy_bbpll_cal == wifi_strict_phy_bbpll_cal,
       "ESP32-S31 PHY BBPLL calibration Rust boundary is inactive");

/* Complete post-initialization register update, with both former MMIO-only
 * ROM/vendor leaves inlined. No other archive member calls those leaves. */
EXTERN(wifi_strict_phy_reg_update_new);
phy_reg_update_new = wifi_strict_phy_reg_update_new;
ASSERT(phy_reg_update_new == wifi_strict_phy_reg_update_new,
       "ESP32-S31 PHY post-init register Rust boundary is inactive");

/* Complete finite 45-word PHY-I2C command-RAM initialization. */
EXTERN(wifi_strict_phy_i2c_master_cmd_mem_init);
phy_i2c_master_cmd_mem_init = wifi_strict_phy_i2c_master_cmd_mem_init;
ASSERT(phy_i2c_master_cmd_mem_init ==
           wifi_strict_phy_i2c_master_cmd_mem_init,
       "ESP32-S31 PHY I2C command-memory Rust boundary is inactive");

/* Still-delegated calibration leaves expect the public g_phyFuns ABI to be a
 * word containing the rev0 callback-table address. Redirect that name to an
 * immutable Rust-owned SRAM word. The discarded phy_init.o .bss cell no
 * longer owns the binding. */
EXTERN(wifi_strict_phy_rom_function_table_binding);
g_phyFuns = wifi_strict_phy_rom_function_table_binding;
ASSERT(g_phyFuns == wifi_strict_phy_rom_function_table_binding,
       "ESP32-S31 PHY ROM function-table binding is not Rust-owned");

/* Publish the ROM PHY ABI table and parameter pointer directly from Rust.
 * The replacement performs no call into phy_get_romfuncs/phy_param_addr and
 * retains the two untouched rev0 ROM callbacks only after validation. */
EXTERN(wifi_strict_phy_get_romfunc_addr);
phy_get_romfunc_addr = wifi_strict_phy_get_romfunc_addr;
ASSERT(phy_get_romfunc_addr == wifi_strict_phy_get_romfunc_addr,
       "ESP32-S31 PHY ROM function table Rust boundary is inactive");

/* TX rate completion is an absolute ROM export even though the pinned archive
 * also contains its reference body. Keep the ROM entry only as an oracle and
 * route runtime calls to the unique finite Rust adapter. */
__real_rcUpdateTxDone = 0x2f80106c;
EXTERN(wifi_strict_rc_update_tx_done);
rcUpdateTxDone = wifi_strict_rc_update_tx_done;
ASSERT(rcUpdateTxDone == wifi_strict_rc_update_tx_done,
       "ESP32-S31 rcUpdateTxDone Rust boundary is inactive");

/* The ROM rcUpdateAckSnr leaf mutates two bytes in a caller-owned rate
 * record. It is therefore not an admissible pure/MMIO-only ROM dependency.
 * Route all callers through the safe Rust value transform and retain the ROM
 * address only as a differential oracle. */
__real_rcUpdateAckSnr = 0x2f801064;
EXTERN(wifi_strict_rc_update_ack_snr);
rcUpdateAckSnr = wifi_strict_rc_update_ack_snr;
ASSERT(rcUpdateAckSnr == wifi_strict_rc_update_ack_snr,
       "ESP32-S31 rcUpdateAckSnr Rust boundary is inactive");

/* TX PER accounting, schedule lowering, and its terminal HE/noise-floor MMIO
 * are Rust-owned. Keep the original ROM entry only as a differential oracle. */
__real_rcTxUpdatePer = 0x2f801060;
EXTERN(wifi_strict_rc_update_tx_per);
rcTxUpdatePer = wifi_strict_rc_update_tx_per;
ASSERT(rcTxUpdatePer == wifi_strict_rc_update_tx_per,
       "ESP32-S31 rcTxUpdatePer Rust boundary is inactive");

/* The default STA/AP/NAN schedule selector used archive-local table anchors.
 * Route it through typed Rust schedule references so no default context can
 * republish a vendor schedule pointer after trc_init. */
EXTERN(wifi_strict_trc_update_ifx_phy_mode);
trc_update_ifx_phy_mode = wifi_strict_trc_update_ifx_phy_mode;
ASSERT(trc_update_ifx_phy_mode == wifi_strict_trc_update_ifx_phy_mode,
       "ESP32-S31 TRC PHY-mode Rust boundary is inactive");

/* rcAttach only initialized shared table indices and four control words.
 * Indices are materialized in Rust literals; no vendor schedule arena may be
 * pulled back into SRAM by this cold initializer. */
EXTERN(wifi_strict_rc_attach);
rcAttach = wifi_strict_rc_attach;
ASSERT(rcAttach == wifi_strict_rc_attach,
       "ESP32-S31 rcAttach Rust boundary is inactive");

/* The vendor PHY-mode selector held archive-local references to every mutable
 * rate schedule. Route it through the typed Rust selector so the old 852-byte
 * bank can be garbage-collected. */
EXTERN(wifi_strict_rc_update_phy_mode);
rcUpdatePhyMode = wifi_strict_rc_update_phy_mode;
ASSERT(rcUpdatePhyMode == wifi_strict_rc_update_phy_mode,
       "ESP32-S31 rcUpdatePhyMode Rust boundary is inactive");

/* Two public ten-byte leaves still returned archive-local B[3] and G[7]
 * addresses after rcUpdatePhyMode moved. Bind them to the same Rust bank so
 * no duplicate schedule section remains live. */
EXTERN(wifi_strict_rc_get_default_schedule);
rc_get_default_sched = wifi_strict_rc_get_default_schedule;
ASSERT(rc_get_default_sched == wifi_strict_rc_get_default_schedule,
       "ESP32-S31 default rate schedule Rust boundary is inactive");

EXTERN(wifi_strict_rc_get_g6m_schedule);
rc_get_G6M_sched = wifi_strict_rc_get_g6m_schedule;
ASSERT(rc_get_G6M_sched == wifi_strict_rc_get_g6m_schedule,
       "ESP32-S31 G6M rate schedule Rust boundary is inactive");

__real_pm_on_beacon_rx = 0x2f800e98;
pm_on_beacon_rx = __wrap_pm_on_beacon_rx;

__real_pm_on_data_rx = 0x2f800e9c;
pm_on_data_rx = __wrap_pm_on_data_rx;

__real_pm_on_data_tx = 0x2f800ea0;
pm_on_data_tx = __wrap_pm_on_data_tx;

/* This leaf is replaced directly rather than through --wrap: the ROM export
 * otherwise defines __wrap_ppTxProtoProc itself before LTO can retain Rust. */
EXTERN(wifi_strict_pp_tx_proto_proc);
ppTxProtoProc = wifi_strict_pp_tx_proto_proc;

EXTERN(wifi_strict_pp_proc_tx_sec_frame);
ppProcTxSecFrame = wifi_strict_pp_proc_tx_sec_frame;

__real_esp_test_tx_enab_statistics = 0x2f801144;
esp_test_tx_enab_statistics = __wrap_esp_test_tx_enab_statistics;

__real_esp_test_set_rx_error_occurs = 0x2f801164;
EXTERN(wifi_strict_esp_test_set_rx_error_occurs);
esp_test_set_rx_error_occurs = wifi_strict_esp_test_set_rx_error_occurs;
ASSERT(esp_test_set_rx_error_occurs == wifi_strict_esp_test_set_rx_error_occurs,
       "ESP32-S31 RX error diagnostic Rust boundary is inactive");

__real_esp_test_rx_process_complete = 0x2f801158;
esp_test_rx_process_complete = __wrap_esp_test_rx_process_complete;

__real_esp_test_rx_parse_mu = 0x2f801178;
esp_test_rx_parse_mu = __wrap_esp_test_rx_parse_mu;
