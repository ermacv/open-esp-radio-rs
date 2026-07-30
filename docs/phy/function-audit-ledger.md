# ESP32-S31 complete PHY function-audit ledger

This ledger tracks the strict audit defined in
[audit-method.md](audit-method.md). Inventory completeness and behavioural
audit completeness are separate numbers.

Audit baseline: 2026-07-30.

## Coverage

| Population | Inventoried | Strictly closed | Body audited, proof open | Unreviewed |
| --- | ---: | ---: | ---: | ---: |
| `libphy.a` external code functions | 161 | 17 | 44 | 100 |
| ROM external `phy_*` code functions | 305 | 0 | 2 | 303 |
| **Total** | **466** | **17** | **46** | **403** |

The earlier cold-Wi-Fi analysis is valuable evidence, but its profile-scoped
rows are not promoted into these strict counts until every instruction, branch
and register-relevant child is rechecked under the complete standard.

## Archive member progress

| Member | Functions | Closed | Body audited/open | Current strict state |
| --- | ---: | ---: | ---: | --- |
| `phy_api.o` | 36 | 0 | 36 | All direct bodies audited; delegated ROM/register proofs open |
| `phy_basic.o` | 1 | 1 | 0 | Complete member body inventory; function is NOT-PORTED |
| `phy_debug.o` | 12 | 0 | 0 | UNREVIEWED |
| `phy_feature.o` | 8 | 8 | 0 | Complete member body inventory; no Rust-owned entry is MATCHED |
| `phy_hw_freq.o` | 7 | 7 | 0 | Complete member body inventory; one no-effect, two NOT-PORTED, four MISMATCH |
| `phy_i2c.o` | 11 | 0 | 0 | UNREVIEWED |
| `phy_init.o` | 19 | 0 | 0 | UNREVIEWED under strict all-branch criterion |
| `phy_reg.o` | 12 | 0 | 0 | UNREVIEWED |
| `phy_rfpll.o` | 4 | 1 | 3 | Direct bodies audited; child proofs and known channel mismatches open |
| `phy_rx_cal.o` | 13 | 0 | 0 | UNREVIEWED |
| `phy_rx_gain.o` | 6 | 0 | 0 | UNREVIEWED |
| `phy_track.o` | 9 | 0 | 0 | UNREVIEWED |
| `phy_tsens.o` | 5 | 0 | 5 | All direct bodies audited; ROM children/integration proofs open |
| `phy_tx_cal.o` | 10 | 0 | 0 | UNREVIEWED |
| `phy_tx_gain.o` | 8 | 0 | 0 | UNREVIEWED |
| **Total** | **161** | **17** | **44** | |

The six archive members without external code functions remain in the artifact
inventory but add no function rows.

## ROM progress

| Function | Address | Size | Status | Rust comparison |
| --- | ---: | ---: | --- | --- |
| `phy_chan14_mic_cfg` | `0x2f826144` | `0x42` | BODY-AUDITED | NOT-PORTED; transitive TX-gain child proof remains open |
| `phy_chan14_mic_enable` | `0x2f826186` | `0x26` | BODY-AUDITED | NOT-PORTED; caller and `phy_chan14_mic_cfg` child proof remain open |

All other ROM functions are currently UNREVIEWED for strict-count purposes,
even when they contributed evidence to the earlier profile audit or vendor
defect analysis.

## Closed archive functions

| Member/function | Size | Status | Register result |
| --- | ---: | --- | --- |
| `phy_basic.o::phy_chan14_mic_cfg_new` | `0x46` | NOT-PORTED | Vendor RMW of `0x20107400` and subsequent TX-gain regeneration are absent from Rust |
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

Detailed proof:
[libphy `phy_basic.o`](audit/libphy-phy_basic.md) and
[libphy `phy_feature.o`](audit/libphy-phy_feature.md), and
[libphy `phy_hw_freq.o`](audit/libphy-phy_hw_freq.md).

Body-audited archive members:
[libphy `phy_api.o`](audit/libphy-phy_api.md),
[libphy `phy_tsens.o`](audit/libphy-phy_tsens.md), and
[libphy `phy_rfpll.o`](audit/libphy-phy_rfpll.md).
