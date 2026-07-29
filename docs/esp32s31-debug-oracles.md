# ESP32-S31 debug oracles

This index separates debug functions that reveal MMIO layout from functions
that only decode packets, DMA descriptors or software state.

The audited binary source is `_oracles/libpp.a`, SHA-256
`f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`,
particularly the complete `hal_debug.o` member and its function-local format
strings. Addresses and fields are accepted into the SVD only when the loads,
shifts and masks agree with those strings.

## Direct MMIO decoders

| Function | Hardware area | SVD status |
| --- | --- | --- |
| `dbg_read_rx_misc` | RX filters, interface/BSSID policy, HE BSSID/color, HE options, nominal padding, RX power save and beamforming | decoded |
| `dbg_read_rx_count` | sparse RX success/error/hang counters | decoded and HIL observed |
| `dbg_read_color_collision` | 64-bit BSS-color bitmap and collision control | decoded and HIL observed |
| `dbg_read_ax_diag` | ten Trigger RX/OFDMA diagnostic words | decoded; no-Trigger HIL baseline |
| `dbg_read_axtb_diag`, `dbg_read_tb_diag` | four Trigger-based TX diagnostic words | decoded; no-Trigger HIL baseline |
| `dbg_read_bsr_info` | BSR control and eight hardware/software TID values | decoded and HIL observed |
| `dbg_read_muedca_timer` | four MU-EDCA timer words | decoded and HIL observed |
| `dbg_read_txq_conf1`, `dbg_read_txq_conf2` | reverse-addressed TB/EDCA queue configuration | decoded and HIL observed |
| `dbg_read_ack_rate`, `dbg_read_cts_rate` | ACK/CTS OFDM, CCK and SCCK response-rate tables | decoded |
| `dbg_read_bfr_rate` | BPSK/QPSK/16-QAM beamforming report profiles | decoded |
| `dbg_read_imrsp_power` | ten immediate-response format/rate/power profiles | decoded by the TX-power peripheral |
| `dbg_read_wdevdelay` | TX/RX CCK timing fields | decoded |
| `dbg_read_rx_ba` | eight reverse-addressed `WDEVTXQBA` TX BlockAck result banks (despite the function name) | complete five-word geometry and named SSN/TID/TA fields decoded |
| `dbg_read_internal_txba` | standalone internal TX BlockAck result | complete five-word geometry and fragment/SSN/TID fields decoded |
| `dbg_read_key_entry` | per-queue key selector plus MAC crypto key table and validity state | selector, table geometry and validity bitmap decoded |
| `dbg_read_tx_mplen` | 120-entry linked MPDU-length table | complete aperture with MPDU length and next-link fields decoded |
| `dbg_read_tx_sig` | active TX queue PLCP/SIG vectors | major queue words mapped; residual diagnostic fields pending |
| `dbg_dump_txq_txinfo` | per-queue ACK/BlockAck and trigger-based TX result | complete two-word field decode |

`dbg_read_nav_misc` is a two-byte `ret` and contains no register evidence.
`dbg_read_tx_power` is not a passive decoder: it calls PHY maximum-power
queries before logging the MAC table, so its fifty `0x20107428` RMW side
effects are recorded separately from the table reads.

## Packet and descriptor decoders

These functions are valuable for restoring wire and DMA formats, but must not
be cited as MMIO evidence unless an instruction independently loads a fixed
hardware address:

- `dbg_dump_trig_common_info`
- `dbg_dump_trig_user_ru`
- `dbg_dump_trig_user_ss`
- `dbg_dump_trig_basic_dependent`
- `dbg_dump_trig_bfrp_dependent`
- `dbg_dump_trig_mubar_dependent`
- `dbg_dump_trig_nfrp_user`
- `dbg_dump_trs_control`
- `dbg_dump_uph_control`
- `dbg_dump_musigb_mimo`
- `dbg_dump_musigb_non_mimo`
- `dbg_dump_rx_ba`
- `dbg_dump_rx_errors`
- `dbg_dump_rx_links`
- `dbg_dump_rx_ppdu`
- `dbg_dump_tx_end`
- `dbg_dump_tx_link`
- `dbg_read_tx_ppdu`

The `dbg_dump_trig_*` group is the primary oracle for Trigger Common Info,
per-user RU/SS fields and Basic/BFRP/MU-BAR/NFRP dependent information.
SVD-independent, allocation-free decoders for all of those printed fields now
live in `open-esp-radio-ieee80211::trigger`. The same module owns the fields
printed by `dbg_dump_trs_control` and `dbg_dump_uph_control`. These functions
therefore restore the 802.11ax wire format, but do not expand the MMIO address
map.

Exact Trigger encodings recovered from the complete functions are:

| Oracle | Recovered byte/bit boundary |
| --- | --- |
| `dbg_dump_trig_common_info` | eight-byte Common Info: type, UL length, More TF, CS, BW, GI/LTF, MU-MIMO LTF mode, HE-LTF/midamble, STBC, LDPC extra symbol, AP power, padding, PE, spatial reuse, Doppler and HE-SIG-A2 reserved |
| `dbg_dump_trig_user_ru` | five-byte RA-RU user: AID12, RU region/allocation, coding, MCS, DCM, RA-RU count/continuation and target RSSI |
| `dbg_dump_trig_user_ss` | five-byte scheduled user: shared prefix plus starting spatial stream, stream count and target RSSI |
| `dbg_dump_trig_basic_dependent` | one byte: MPDU MU spacing, TID aggregation limit and preferred AC |
| `dbg_dump_trig_bfrp_dependent` | one-byte feedback-segment retransmission bitmap |
| `dbg_dump_trig_mubar_dependent` | four-byte BAR control/information, including policy, type, TID and SSN |
| `dbg_dump_trig_nfrp_user` | five-byte starting AID, feedback type, target RSSI and multiplexing layout |
| `dbg_dump_trs_control` | 32-bit TRS HE-control information |
| `dbg_dump_uph_control` | named UPH fields in bits 6:13; upper bits remain explicitly unowned |

## Mutating debug helpers

The following symbols change hardware or debug policy and need the same
transaction-level treatment as ordinary HAL setters:

- `dbg_clr_hw_count`
- `dbg_complete_ignore_no_key`
- `dbg_disable_report_cbf`
- `dbg_tb_ignore_cca_enable`
- `dbg_check_mutimer`

Their symbol names alone are not sufficient to assign bit semantics. A field
is promoted only after the complete body and all reachable leaves are bounded.

## Other archives and ROM

The first cross-object scan found additional direct-MMIO candidates in
`libpp.a[pp_debug.o]`: `dbg_lmac_rxtx_statis_dump` reaches `0x20104388`,
`dbg_lmac_hw_statis_dump` reaches `0x2010435c` and `0x20104e08`, and
`dbg_lmac_diag_statis_dump` reaches `0x201043b4` and `0x20104e50`. Their
complete field-name/width audit is still pending, so these addresses are not
yet promoted to the SVD. `test_hal_rx_mu_sigb.o::dbg_dump_rx_sigb` also reads
`0x20104028`, but mixes that observation with an RX object and needs the same
boundary audit.

By contrast, `hal_he_common.o::dbg_hal_check_set_mplen_bitmap` and
`dbg_hal_check_clr_mplen_bitmap` traverse software-owned allocation bitmaps;
they help recover the MPDU-link allocator but do not add fixed MMIO addresses.
`hal_debug.o::dbg_check_mutimer` likewise snapshots software history around a
MU timer rather than directly naming a new register bank.

`libnet80211.a` contains `dbg_hmac_*_statis_dump`, `esp_wifi_statis_dump`,
`bsscolor_event_dump` and TWT dump functions. They primarily expose software
state and protocol objects rather than raw MAC addresses, but they can connect
an already recovered register field to its higher-level 802.11 meaning.

The rev0 ROM ELF does not contain an equivalent rich `dbg_read_*` register
suite. Its debug-related public symbols are mainly configuration leaves such
as `phy_chan_dump_cfg`, `phy_csidump_force_lltf_cfg`,
`phy_txcal_debuge_mode_` and `phy_pbus_debugmode`. The register-map recovery
value is consequently concentrated in blob `hal_debug.o`; ROM remains the
stronger oracle for complete PHY algorithms and fixed-address leaf behavior.
