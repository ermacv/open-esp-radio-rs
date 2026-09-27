//! Reviewed coverage exclusions: vendor locations the comparisons never reach,
//! each with its reason.
use crate::coverage::{Decision, Place};

/// Reviewed exclusions of every scenario. Vendor runtime helpers inside the
/// PHY closures: their paths depend on operand magnitudes, alignment and
/// diagnostic formatting, not on driver behavior; production has its own Rust
/// arithmetic, copies and no vendor console output, and the comparisons
/// observe the helpers only through the PHY effects that use their results.
/// Vendor configurations production rejects fail-closed have no production
/// path to compare.
pub const DECISIONS: &[Decision] = &[
    Decision {
        reason: "a packet PTI at least coexistence event one's PTI 5, which `mac_tx_set_pti` \
            then publishes instead: every data priority production sends is 3 or lower",
        places: &[Place::Range {
            function: "mac_tx_set_pti",
            start: 0x44,
            end: 0x4c,
        }],
    },
    Decision {
        reason: "rate codes production never transmits in the control-rate and PLCP1 maps: \
            code 4 and every code above HE MCS 9 (0x23); and two branches of \
            `mac_tx_get_rts_rate` no code reaches (the HT sub-ranges above 0x12 and 0x1c, \
            and a legacy code outside its three masks, which cover all eight)",
        places: &[
            Place::Range {
                function: "mac_tx_get_rts_rate",
                start: 0x3c,
                end: 0x3e,
            },
            Place::Range {
                function: "mac_tx_get_rts_rate",
                start: 0x56,
                end: 0x58,
            },
            Place::Range {
                function: "mac_tx_get_rts_rate",
                start: 0x70,
                end: 0x72,
            },
            Place::Range {
                function: "mac_tx_get_rts_rate",
                start: 0x82,
                end: 0x84,
            },
            Place::Range {
                function: "mac_tx_set_plcp1",
                start: 0x52,
                end: 0x5a,
            },
        ],
    },
    Decision {
        reason: "an HE PPDU at 40 MHz: production transmits HE at 20 MHz only (reviewed with \
            the Wi-Fi owner)",
        places: &[Place::Range {
            function: "mac_tx_set_plcp1",
            start: 0x8a,
            end: 0x8c,
        }],
    },
    Decision {
        reason: "descriptor classes production never publishes a PPDU for: an A-MPDU \
            container without its first-MPDU bit 19 (only later MPDUs carry that, and no PLCP0 \
            follows them), the bit-20 class of PLCP0 format three, and an HE PPDU without the \
            SU-acknowledgement bit 30 (HE no-ack and trigger-based are fail-closed); \
            production publishes the PLCP0 format from its typed program (reviewed with the \
            Wi-Fi owner)",
        places: &[
            Place::Range {
                function: "mac_tx_set_plcp0",
                start: 0x42,
                end: 0x5a,
            },
            Place::Range {
                function: "mac_tx_set_plcp0",
                start: 0x248,
                end: 0x24a,
            },
            Place::Range {
                function: "mac_tx_set_plcp0",
                start: 0x296,
                end: 0x2a0,
            },
        ],
    },
    Decision {
        reason: "diagnostic output of `mac_tx_set_plcp0` that publishes no register: the \
            protection-threshold log of a non-HE descriptor with threshold fields, and the \
            PPDU dump (`dbg_read_tx_ppdu`, which only reads and prints) of a descriptor and \
            frame header both flagged with bit 24; both return into the compared publication",
        places: &[
            Place::Range {
                function: "mac_tx_set_plcp0",
                start: 0x80,
                end: 0x10c,
            },
            Place::Range {
                function: "mac_tx_set_plcp0",
                start: 0x13a,
                end: 0x21a,
            },
            Place::Function("dbg_read_tx_ppdu"),
        ],
    },
    Decision {
        reason: "diagnostic logging of `hal_mac_tx_set_ppdu` and `mac_tx_set_hesig` that \
            publishes no register: an unexpected rate context, a misaligned frame buffer, an \
            entry class other than one and an aggregation flag without several descriptors; \
            both continue into the compared publication",
        places: &[
            Place::Range {
                function: "hal_mac_tx_set_ppdu",
                start: 0x20,
                end: 0x54,
            },
            Place::Range {
                function: "hal_mac_tx_set_ppdu",
                start: 0x5c,
                end: 0x80,
            },
            Place::Range {
                function: "mac_tx_set_hesig",
                start: 0x220,
                end: 0x27e,
            },
            Place::Range {
                function: "mac_tx_set_hesig",
                start: 0x2e0,
                end: 0x30a,
            },
        ],
    },
    Decision {
        reason: "`mac_tx_set_hesig` without a rate context: it logs and publishes no HE-SIG; \
            production cannot build an HE program without its rate",
        places: &[Place::Range {
            function: "mac_tx_set_hesig",
            start: 0x22,
            end: 0x80,
        }],
    },
    Decision {
        reason: "a non-HE rate code in `mac_tx_set_hesig`, which only HE descriptors reach; \
            every production HE rate maps to a code of at least 0x1a",
        places: &[Place::Range {
            function: "mac_tx_set_hesig",
            start: 0xa6,
            end: 0xaa,
        }],
    },
    Decision {
        reason: "TXOP holding: the descriptor's TXOP request raises the queue's PTI TXOP bit, \
            and `mac_tx_set_txop_q` programs a granted TXOP slot; production holds no TXOP \
            and clears that bit for every SU PPDU (reviewed with the Wi-Fi owner)",
        places: &[
            Place::Range {
                function: "hal_mac_tx_set_ppdu",
                start: 0xea,
                end: 0xfe,
            },
            Place::Function("mac_tx_set_txop_q"),
            Place::Function("hal_mac_fill_hwtxop"),
        ],
    },
    Decision {
        reason: "a software HE-Control image: the driver never supplies one, so the hardware \
            BSR path stays selected and `mac_tx_set_tb` is reached (reviewed with the Wi-Fi \
            owner)",
        places: &[
            Place::Range {
                function: "hal_mac_tx_set_ppdu",
                start: 0x1aa,
                end: 0x1ae,
            },
            Place::Range {
                function: "mac_tx_set_hesig",
                start: 0x312,
                end: 0x316,
            },
            Place::Range {
                function: "hal_he_set_htc",
                start: 0x1c,
                end: 0x20,
            },
        ],
    },
    Decision {
        reason: "HE trigger-based preparation: production keeps HE-TB fail-closed and \
            transmits HE SU only, so the MPDU-length links, their bitmap checks and the \
            TB-flow configuration are not used (reviewed with the Wi-Fi owner)",
        places: &[
            Place::Function("mac_tx_set_tb"),
            Place::Function("mac_tx_set_mplen"),
            Place::Function("hal_mac_tx_clr_mplen"),
            Place::Function("dbg_hal_check_set_mplen_bitmap"),
            Place::Function("dbg_hal_check_clr_mplen_bitmap"),
        ],
    },
    Decision {
        reason: "the BSS color of an interface other than the station: the access point \
            transmits no HE (reviewed with the Wi-Fi owner)",
        places: &[
            Place::Range {
                function: "hal_he_get_bss_color",
                start: 0x4,
                end: 0x6,
            },
            Place::Range {
                function: "hal_he_get_bss_color",
                start: 0x1a,
                end: 0x30,
            },
        ],
    },
    Decision {
        reason: "the OS-adapter critical section `wDev_AppendRxBlocks` enters and leaves \
            through `g_osi_funcs_p`, answered as quiet calls; production serializes the RX \
            ring through its single owner",
        places: &[
            Place::Range {
                function: "wDev_AppendRxBlocks",
                start: 0x7e,
                end: 0x80,
            },
            Place::Range {
                function: "wDev_AppendRxBlocks",
                start: 0xb4,
                end: 0xb6,
            },
        ],
    },
    Decision {
        reason: "an append doorbell that never clears: the vendor dumps its list after the \
            same 0x186a1 polls production bounds `complete_pending_reload` with, which then \
            reports the walker busy; a hardware fault no comparison case can bound cheaply",
        places: &[Place::Range {
            function: "wDev_AppendRxBlocks",
            start: 0xc2,
            end: 0xd2,
        }],
    },
    Decision {
        reason: "RX test statistics: the vendor counts base repairs and unrepairable \
            cursors through `esp_test_rx_statistics`, which the comparison leaves disabled \
            and production does not keep",
        places: &[
            Place::Range {
                function: "wDev_AppendRxBlocks",
                start: 0xfe,
                end: 0x10a,
            },
            Place::Range {
                function: "wDev_AppendRxBlocks",
                start: 0x15c,
                end: 0x16c,
            },
        ],
    },
    Decision {
        reason: "an exhausted walker whose LAST has no successor: the vendor counts it and \
            leaves the base alone, while production treats a LAST outside its accepted list \
            as corruption and requires a reset (fail-closed by design)",
        places: &[Place::Range {
            function: "wDev_AppendRxBlocks",
            start: 0xf2,
            end: 0xf4,
        }],
    },
    Decision {
        reason: "the MU-EDCA minimum exponent a success restores for a frame whose descriptor \
            selects MU EDCA: production neither negotiates nor applies the MU-EDCA parameter \
            set (`is_use_muedca` is zero for it) and always restores the ordinary minimum \
            (reviewed with the Wi-Fi owner)",
        places: &[
            Place::Range {
                function: "lmacProcessShortFrameSuccess",
                start: 0x1e,
                end: 0x24,
            },
            Place::Range {
                function: "lmacProcessLongFrameSuccess",
                start: 0x1e,
                end: 0x24,
            },
        ],
    },
    Decision {
        reason: "TXOP bursting: `lmacProcessTxSuccess` completes a held TXOP slot through \
            `lmacProcessTxopQComplete`; production holds no TXOP slot and runs every \
            transaction as its own exchange (reviewed with the Wi-Fi owner)",
        places: &[
            Place::Range {
                function: "lmacProcessTxSuccess",
                start: 0x48,
                end: 0x58,
            },
            Place::Function("lmacProcessTxopQComplete"),
        ],
    },
    Decision {
        reason: "a per-frame RTS length threshold in the transmit descriptor: production has \
            none, only the BSS length and HE TXOP-duration rules; after a success it only \
            chooses which queue short or long counter the vendor resets, and the compared \
            contention exponent is the same on both paths (reviewed with the Wi-Fi owner)",
        places: &[Place::Range {
            function: "lmacIsLongFrame",
            start: 0x16,
            end: 0x18,
        }],
    },
    Decision {
        reason: "`rcGetRate` modes the production ordinary retry owner does not have: the \
            descriptor rate bypass, a missing rate context and a context-fixed rate; \
            production always selects from its owned initial rate",
        places: &[
            // Descriptor bypass and null-context returns.
            Place::Range {
                function: "rcGetRate",
                start: 0x20,
                end: 0x26,
            },
            // The context-fixed rate and its descriptor adjustment.
            Place::Range {
                function: "rcGetRate",
                start: 0x26,
                end: 0x5c,
            },
            Place::Range {
                function: "rcGetRate",
                start: 0x68,
                end: 0x6e,
            },
        ],
    },
    Decision {
        reason: "an exhausted `rcGetRate` record: every admitted 802.11g record's attempt \
            counts reach its publication limit (checked on the captured arena), and \
            production ends the MPDU at that limit before another rate is selected",
        places: &[Place::Range {
            function: "rcGetRate",
            start: 0xf0,
            end: 0xf6,
        }],
    },
    Decision {
        reason: "the body of `hal_mac_txq_enable` after its publication edge: `GetAccess` \
            access bookkeeping, HE and MU-EDCA queue state and test statistics; the ordinary \
            transmit claim compares the prefix before that call, and production's transmit \
            engine owns its own queue bookkeeping",
        places: &[
            Place::Range {
                function: "hal_mac_txq_enable",
                start: 0x22,
                end: 0xbc,
            },
            Place::Function("esp_test_tx_enab_statistics"),
            Place::Function("is_use_muedca"),
            Place::Function("wifi_he_get_hetb_tid_bitmap"),
        ],
    },
    Decision {
        reason: "MAC selectors outside the claimed bounded features: `hal_mac_tsf_reset` \
            selector 1 (the mesh TSF `wDev_Mesh_Enable_Tsf` enables), 3 and other values, and \
            `hal_mac_clr_txq_state` selectors other than 2; production has no mesh role and \
            implements only the fresh AP epoch and the ordinary transmit-completion clear",
        places: &[
            // The selector dispatch and the other epochs after the fresh one.
            Place::Range {
                function: "hal_mac_tsf_reset",
                start: 0x2,
                end: 0xe,
            },
            Place::Range {
                function: "hal_mac_tsf_reset",
                start: 0x5e,
                end: 0xf8,
            },
            // Every arm before the selector-2 clear.
            Place::Range {
                function: "hal_mac_clr_txq_state",
                start: 0x0,
                end: 0x2e,
            },
        ],
    },
    Decision {
        reason: "receive-policy codes above 13 and interface indices above the vendor bounds \
            (3 for addresses and BSSID checks, 2 for receive filters), which fail or log: \
            production's typed `MacRoleReceivePolicy` and `MacInterface` cannot express them",
        places: &[
            Place::Range {
                function: "wifi_set_rx_policy",
                start: 0x16,
                end: 0x18,
            },
            Place::Range {
                function: "wifi_set_rx_policy",
                start: 0x1b8,
                end: 0x1bc,
            },
            Place::Range {
                function: "ic_set_rx_policy_ubssid_check",
                start: 0x2,
                end: 0x4,
            },
            Place::Range {
                function: "ic_set_rx_policy_ubssid_check",
                start: 0x1a,
                end: 0x1e,
            },
            Place::Range {
                function: "ic_set_mac",
                start: 0x4,
                end: 0x1e,
            },
            Place::Range {
                function: "hal_mac_rx_set_policy",
                start: 0x6,
                end: 0x8,
            },
            Place::Range {
                function: "hal_mac_rx_set_policy",
                start: 0xce,
                end: 0xd2,
            },
        ],
    },
    Decision {
        reason: "the other `wifi_set_rx_policy` jump-table arms serve vendor lifecycles \
            production does not have: the station scan start and end (3, 4), join (5) and \
            leave (7) policies of a scan through receive queue zero, while production scans \
            and authenticates through queue three's promiscuous admission (`hal_sniffer`), \
            enters the link with policy six through `apply_sta_link_receive_policy` and leaves \
            with policy two; and the remain-on-channel (13, ended by 1) and NAN (10 to 12) \
            policies, which have no production owner. The control-mode-two receive filter of \
            `hal_mac_rx_set_policy` serves only policies 10 and 13",
        places: &[
            Place::Range {
                function: "wifi_set_rx_policy",
                start: 0x30,
                end: 0x32,
            },
            Place::Range {
                function: "hal_mac_rx_set_policy",
                start: 0x98,
                end: 0x9c,
            },
            Place::Range {
                function: "hal_mac_rx_set_policy",
                start: 0xbc,
                end: 0xc4,
            },
        ],
    },
    Decision {
        reason: "vendor diagnostic formatting; production emits no vendor console output",
        places: &[
            Place::Function("wifi_log"),
            Place::Function("ets_printf"),
            Place::Function("ets_vprintf"),
            Place::Function("_cvt"),
            Place::Function("strlen"),
        ],
    },
    Decision {
        reason: "compiler and C runtime helpers; production uses Rust core arithmetic and copies",
        places: &[
            Place::Function("__divdi3"),
            Place::Function("__udivdi3"),
            Place::Function("__umoddi3"),
            Place::Function("__clzsi2"),
            Place::Function("memcpy"),
        ],
    },
    Decision {
        reason: "compiler prologue and epilogue millicode; its entry point depends on register pressure",
        places: &[
            Place::Function("__riscv_save_10"),
            Place::Function("__riscv_save_12"),
            Place::Function("__riscv_restore_10"),
            Place::Function("__riscv_restore_12"),
        ],
    },
    Decision {
        reason: "direct RFPLL programming for a frequency outside the 2400-2484 MHz channel \
            table: production rejects such a channel request fail-closed, and inside the \
            claimed closures only that branch of `phy_set_channel_rfpll_freq_new` (and of \
            its rev0 ROM predecessor) reaches the direct-programming chain",
        places: &[
            // The frequency-table bound check's out-of-table path.
            Place::Range {
                function: "phy_set_channel_rfpll_freq_new",
                start: 0x28,
                end: 0x3a,
            },
            Place::Function("phy_set_rf_freq_offset_new"),
            // The same bound check and chain of rev0 ROM, which keeps its own
            // capacitor search.
            Place::Range {
                function: "phy_set_channel_rfpll_freq",
                start: 0x24,
                end: 0x44,
            },
            Place::Function("phy_set_rf_freq_offset"),
            Place::Function("phy_set_rfpll_freq"),
            Place::Function("phy_rfpll_cap_init_cal"),
            Place::Function("phy_rfpll_set_freq"),
            Place::Function("phy_wait_rfpll_cal_end"),
            Place::Function("phy_write_rfpll_sdm"),
            Place::Function("phy_restart_cal"),
        ],
    },
    Decision {
        reason: "2484-MHz (channel 14) and 5-GHz MHz requests: production accepts only \
            channels 1 to 13 and rejects the others fail-closed",
        places: &[
            // The 2484-MHz equality and the above-2483-MHz directions ...
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0x6,
                end: 0x7,
            },
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0xe,
                end: 0xf,
            },
            // ... and their 5-GHz and channel-14 blocks.
            Place::Range {
                function: "phy_mhz2ieee",
                start: 0x26,
                end: 0x3a,
            },
        ],
    },
    Decision {
        reason: "RX-DC minimum search that admits no estimate: readiness activity without a \
            detected RX saturation, whose first radio search returns the ROM's \
            never-initialized output slot; characterized as a DIFF, not claimed \
            (user decision 2026-09-26)",
        places: &[
            // The rejected-estimate direction of the saturation guard ...
            Place::Range {
                function: "phy_rxdc_est_min",
                start: 0x52,
                end: 0x53,
            },
            // ... the retry while no estimate lowered the minimum, and the
            // exhausted search's power sentinel.
            Place::Range {
                function: "phy_rxdc_est_min",
                start: 0x76,
                end: 0x7e,
            },
        ],
    },
    Decision {
        reason: "diagnostic print of the RX-DC PBus results, selected by bit 3 of \
            `phy_param[0x10]`; production emits no vendor console output",
        places: &[Place::Range {
            function: "phy_pbus_rx_dco_cal_1step_new",
            start: 0x1c6,
            end: 0x26c,
        }],
    },
    Decision {
        reason: "diagnostic print of the generated RX gain table, selected by bit 8 of \
            `phy_param[0x10]`; production emits no vendor console output",
        places: &[
            Place::Range {
                function: "phy_gen_rx_gain_table",
                start: 0xc4,
                end: 0xf0,
            },
            Place::Range {
                function: "phy_gen_rx_gain_table",
                start: 0xf4,
                end: 0x106,
            },
        ],
    },
    Decision {
        reason: "RX gain table generation over the fixed tables `phy_set_rx_gain_table` \
            builds: the generation always stops at its limit within the table, and the \
            last indices it stores (75 and 71) never reach the 0x4f and 0x4c clamps, which \
            only foreign `phy_param` contents could; production derives the same indices \
            from its generated tables",
        places: &[
            Place::Range {
                function: "phy_gen_rx_gain_table",
                start: 0x72,
                end: 0x73,
            },
            Place::Range {
                function: "phy_gen_rx_gain_table",
                start: 0x86,
                end: 0x87,
            },
            Place::Range {
                function: "phy_gen_rx_gain_table",
                start: 0x12e,
                end: 0x136,
            },
            Place::Range {
                function: "phy_set_rx_gain_table",
                start: 0x184,
                end: 0x185,
            },
            Place::Range {
                function: "phy_set_rx_gain_table",
                start: 0x1c4,
                end: 0x1c5,
            },
            Place::Range {
                function: "phy_set_rx_gain_table",
                start: 0x22a,
                end: 0x232,
            },
            Place::Range {
                function: "phy_set_rx_gain_table",
                start: 0x27e,
                end: 0x286,
            },
        ],
    },
    Decision {
        reason: "diagnostic print of the TX-DC search, selected by bit 4 of \
            `phy_param[0x10]`; production emits no vendor console output",
        places: &[
            Place::Range {
                function: "phy_txdc_cal_pwdet_new",
                start: 0x174,
                end: 0x1a6,
            },
            Place::Range {
                function: "phy_txdc_cal_pwdet_new",
                start: 0x258,
                end: 0x280,
            },
            Place::Range {
                function: "phy_txdc_cal_pwdet_new",
                start: 0x310,
                end: 0x34a,
            },
        ],
    },
    Decision {
        reason: "TX-DC teardown skipped for a nonzero second argument, which every \
            `phy_txdc_cal_pwdet_init` caller in the claimed closures leaves zero",
        places: &[Place::Range {
            function: "phy_txdc_cal_pwdet_init",
            start: 0x166,
            end: 0x167,
        }],
    },
    Decision {
        reason: "repeated power-detector status polls before it reports ready: the ready \
            read is compared and the poll count is hardware timing",
        places: &[Place::Range {
            function: "phy_pwdet_tone_start",
            start: 0x4a,
            end: 0x4b,
        }],
    },
    Decision {
        reason: "diagnostic print of the calibration tracking parent, selected by its \
            first argument; production emits no vendor console output",
        places: &[
            Place::Range {
                function: "phy_cal_param_track",
                start: 0x62,
                end: 0x7e,
            },
            Place::Range {
                function: "phy_cal_param_track",
                start: 0x154,
                end: 0x170,
            },
        ],
    },
    Decision {
        reason: "diagnostic print of the Bluetooth TX gain; production emits no vendor \
            console output",
        places: &[Place::Range {
            function: "phy_bt_get_tx_gain",
            start: 0xfe,
            end: 0x11c,
        }],
    },
    Decision {
        reason: "repeated analog I2C busy polls before a write: the poll count is hardware \
            timing and the command itself is compared",
        places: &[Place::Range {
            function: "phy_chip_i2c_writeReg",
            start: 0x3a,
            end: 0x3b,
        }],
    },
    Decision {
        reason: "temperature clamps at 250 and -200: the sensor scenario compares every \
            8-bit code in every DAC window and none reaches them",
        places: &[
            Place::Range {
                function: "phy_code_to_temp",
                start: 0x1a,
                end: 0x1b,
            },
            Place::Range {
                function: "phy_code_to_temp",
                start: 0x36,
                end: 0x3e,
            },
            Place::Range {
                function: "phy_code_to_temp",
                start: 0x44,
                end: 0x4a,
            },
        ],
    },
    Decision {
        reason: "frequency-memory parameter selectors 0 and 1, which only callers outside \
            the claimed closures pass; every claimed caller selects 2",
        places: &[
            Place::Range {
                function: "phy_get_freq_mem_param",
                start: 0x4,
                end: 0x16,
            },
            Place::Range {
                function: "phy_get_freq_mem_param",
                start: 0x1a,
                end: 0x26,
            },
        ],
    },
    Decision {
        reason: "mixer digital-gain index above 10: its claimed callers pass indices from \
            the fixed RX gain tables and calibration loops, all at most 10",
        places: &[
            Place::Range {
                function: "phy_bt_rx_mx_dgain",
                start: 0x16,
                end: 0x17,
            },
            Place::Range {
                function: "phy_bt_rx_mx_dgain",
                start: 0x26,
                end: 0x2a,
            },
        ],
    },
    Decision {
        reason: "constant arguments only callers outside the claimed closures pass: the \
            linear-to-dB scale 3 of `phy_get_power_db` and PBus read selectors above 4 \
            (`phy_pbus_print`, `phy_set_rx_gain_cal_iq`, `phy_bt_txdc_cal*`) and the \
            channel-register argument 0 (`phy_wakeup_init`); a claimed \
            caller passing them would either reach these paths or leave its call \
            uncovered",
        places: &[
            Place::Range {
                function: "phy_linear_to_db",
                start: 0x1e,
                end: 0x1f,
            },
            Place::Range {
                function: "phy_linear_to_db",
                start: 0x68,
                end: 0x6e,
            },
            Place::Range {
                function: "phy_pbus_rd_addr",
                start: 0x2,
                end: 0x3,
            },
            Place::Range {
                function: "phy_pbus_rd_addr",
                start: 0x40,
                end: 0x4c,
            },
            Place::Range {
                function: "phy_pbus_rd_shift",
                start: 0x2,
                end: 0x3,
            },
            Place::Range {
                function: "phy_pbus_rd_shift",
                start: 0x36,
                end: 0x3a,
            },
            // `phy_chip_set_chan_misc_new` passes 1; only `phy_wakeup_init` passes 0.
            Place::Range {
                function: "phy_set_chan_reg",
                start: 0x26,
                end: 0x27,
            },
        ],
    },
    Decision {
        reason: "diagnostic print of the TX-power tracking child, selected by its third \
            argument; production emits no vendor console output",
        places: &[Place::Range {
            function: "phy_txpwr_cal_track_new",
            start: 0xd8,
            end: 0xfa,
        }],
    },
    Decision {
        reason: "diagnostic prints of the RFPLL capacitance tracking and correction, \
            selected by their first argument; production emits no vendor console output",
        places: &[
            Place::Range {
                function: "phy_rfpll_cap_correct_track",
                start: 0x1e,
                end: 0x32,
            },
            Place::Range {
                function: "phy_rfpll_cap_track_new",
                start: 0x68,
                end: 0x84,
            },
        ],
    },
    Decision {
        reason: "diagnostic print of the Wi-Fi TX gain; production emits no vendor console \
            output",
        places: &[Place::Range {
            function: "phy_wifi_get_tx_gain",
            start: 0xb2,
            end: 0xd6,
        }],
    },
    Decision {
        reason: "repeated software-frequency status polls before the start completes: the \
            poll count is hardware timing",
        places: &[Place::Range {
            function: "phy_set_chan_freq_sw_start",
            start: 0x18,
            end: 0x19,
        }],
    },
    Decision {
        reason: "sensor DAC outside the five calibrated windows: the ROM's default index 5 \
            reads beyond its five-entry attribute table, and production rejects the DAC \
            fail-closed",
        places: &[Place::Range {
            function: "phy_tsens_dac_to_index",
            start: 0x22,
            end: 0x28,
        }],
    },
    Decision {
        reason: "TX gain publication skip option (`phy_param[7]`), which the production \
            configuration fixes disabled",
        places: &[Place::Range {
            function: "phy_wifi_set_tx_gain_new",
            start: 0x4a,
            end: 0x4b,
        }],
    },
    Decision {
        reason: "RX gain memory writes over the tables generated from the fixed inputs of \
            `phy_set_rx_gain_table`: their RXBB DC index never exceeds 5 and the RF gain \
            index search always finds its entry",
        places: &[
            Place::Range {
                function: "phy_get_rxbb_dc_new",
                start: 0x6,
                end: 0xc,
            },
            Place::Range {
                function: "phy_rfrx_gain_index_new",
                start: 0x64,
                end: 0x65,
            },
        ],
    },
    Decision {
        reason: "TX baseband gain index above 4: the claimed `phy_txdc_cal_pwdet_init` \
            passes loop indices 0 to 2, and the other caller is outside the claimed \
            closures",
        places: &[
            Place::Range {
                function: "phy_index_to_txbbgain",
                start: 0x2,
                end: 0x3,
            },
            Place::Range {
                function: "phy_index_to_txbbgain",
                start: 0x1a,
                end: 0x1e,
            },
        ],
    },
    Decision {
        reason: "reentrancy guard `phy_param[0x194]` that RFPLL capacitance tracking sets for \
            the duration of its correction: production's exclusive radio ownership makes a \
            nested correction impossible",
        places: &[Place::Range {
            function: "phy_rfpll_cap_track_new",
            start: 0x42,
            end: 0x43,
        }],
    },
    Decision {
        reason: "analog I2C read-mask default for block identifiers outside 10 to 109: every \
            analog register the claimed closures access lies in blocks 0x62 to 0x6d",
        places: &[
            Place::Range {
                function: "phy_get_i2c_read_mask_new",
                start: 0xa,
                end: 0xb,
            },
            Place::Range {
                function: "phy_get_i2c_read_mask_new",
                start: 0x20,
                end: 0x24,
            },
        ],
    },
    Decision {
        reason: "step limit of the TX gain table walk: each step moves one entry toward a \
            table end and the walk stops at either end, so from a start index inside the \
            table it ends within count - 1 steps and the limit never stops it",
        places: &[Place::Range {
            function: "phy_get_tx_gain_value",
            start: 0x10,
            end: 0x11,
        }],
    },
    Decision {
        reason: "channel-14 MIC configuration: production rejects an enabled MIC option \
            and channel 14 fail-closed, as the qualified AP/STA profile requires",
        places: &[
            Place::Function("phy_chan14_mic_cfg_new"),
            Place::Function("phy_set_most_tpw"),
            // The `phy_param[0x26]` guard's enabled path up to the 802.11p guard.
            Place::Range {
                function: "phy_chip_set_chan",
                start: 0xb6,
                end: 0xca,
            },
        ],
    },
    Decision {
        reason: "external coexistence work modes production rejects without a write: mode 1, \
            whose vendor path sets bits 31:30 of the external configuration, and modes from 3, \
            which the vendor reports through the `g_coa_funcs_p` console slot",
        places: &[
            // The mode-1 and invalid-mode dispatch directions.
            Place::Range {
                function: "hal_set_extern_pti_mode",
                start: 0x20,
                end: 0x24,
            },
            Place::Range {
                function: "hal_set_extern_pti_mode",
                start: 0x2a,
                end: 0x2c,
            },
            // The mode-1 configuration and the invalid-mode report.
            Place::Range {
                function: "hal_set_extern_pti_mode",
                start: 0x44,
                end: 0x5a,
            },
            Place::Range {
                function: "hal_set_extern_pti_mode",
                start: 0xca,
                end: 0xec,
            },
        ],
    },
];
