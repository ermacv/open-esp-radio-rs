# ESP32-S31 debug oracles

This index separates debug functions that reveal MMIO layout from functions
that only decode packets, DMA descriptors or software state.

The audited binary source is `_oracles/libpp.a`, SHA-256
`f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`,
particularly the complete `hal_debug.o` member and its function-local format
strings. Addresses and fields are accepted into the SVD only when the loads,
shifts and masks agree with those strings.
The Trigger iterator additionally uses `_oracles/libnet80211.a`, SHA-256
`92550813de20ed0d51110dbd72b646e891a86a8a2f81fa53714ebf2ebf9c8f40`.

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
| `dbg_read_tx_sig` | active TX queue PLCP/SIG vectors | complete HT/HE branches plus the internal VHT words, HT20/HT40 length/count and antenna-selector fields decoded |
| `dbg_dump_txq_txinfo` | per-queue ACK/BlockAck and trigger-based TX result | complete two-word field decode |
| `dbg_lmac_hw_statis_dump` | RX/TX interrupt, Trigger and hang/panic hardware counters | complete address/name map; opaque full-width values retained where no mask exists |
| `dbg_lmac_diag_statis_dump` | sparse `diag0..diag12` and `diagsel` bank | complete address/name map; higher-level meanings pending |

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

The complete `_oracles/libnet80211.a[test_rx_trig.o]::
esp_test_rx_parse_trig` adds the formerly missing iteration boundary. It is
`0x1d6` bytes and advances Basic users by 5+1 bytes, MU-BAR users by 5+4,
BFRP/MU-RTS/BSRP/BQRP users by five, and NFRP as one terminal five-byte user.
It stops only when both AID12 is `0xfff` and RU allocation is `0x7f`.
`TriggerUserIterator` reproduces those finite strides without allocation and
retains the padding sentinel for diagnostics. The blob test deliberately
traps on Groupcast MU-BAR and does not bound reserved layouts, so the Rust
iterator returns `UnsupportedUserLayout` for them instead of guessing.

Complete `_oracles/libpp.a[hal_utilities.o]::ru2str` (size `0x8c`) closes
the next wire-to-scheduler boundary. Its positive one-based index domains are
raw allocations 0..8 for RU26, 37..40 for RU52, 53..54 for RU106 and 61..62
for RU242. Values 69 and above are labeled `484 OFDM`; they are retained as a
distinct unsupported-wide class because ESP32-S31 non-AP operation is
20-MHz-only. Gap encodings do not produce a fresh valid index in the blob and
the Rust classifier returns `None` rather than inheriting its mutable string
buffer.

The MAC-side `HeTriggerScheduledRate` then joins those wire fields to the
complete RU rate tables. It accepts only a Basic Trigger, 20-MHz uplink,
the associated AID, one spatial stream, a classified narrow RU, MCS0..9 and
the separately typed BCC/LDPC DCM sets. Complete
`test_rx_trig.o::esp_test_cal_tx_tb` (size `0xa44`) is the arithmetic oracle
for RU class, coding, MCS, GI/LTF and scheduled spatial-stream inputs. This
is a host-verifiable admission/rate plan, not yet evidence that hardware
entered the TB transmit state.

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
- `dbg_disable_report_cbf`
- `dbg_tb_ignore_cca_enable`
- `dbg_check_mutimer`

Their symbol names alone are not sufficient to assign bit semantics. A field
is promoted only after the complete body and all reachable leaves are bounded.

`dbg_complete_ignore_no_key` is not a mutator despite its imperative-looking
name. Its complete body only filters a completion object, reads the already
mapped key-validity word at `0x20104814`, and logs descriptor/software state.
It adds no new fixed address or write semantics.

The direct mutator audit is also now bounded further. Complete
`dbg_tb_ignore_cca_enable` already supplies the polarity of
`0x20104c7c[12]`. Complete `dbg_disable_report_cbf` operates only on the
already mapped beamforming-control word `0x20104c78[20]`; it clears the bit
for a nonzero “disable” argument and sets it for zero. This strengthens the
compressed-beamforming-report interpretation but adds no new address.
`dbg_clr_hw_count` is only a tail call to `esp_test_clr_hw_statistics`, but
the reachable leaf is useful: it asserts `0x20104c00[16]`, pulses
`0x20104308[0]` high then low, and clears the first bit. The SVD records this
as one bounded hardware-statistics-clear transaction.

## Other archives and ROM

The cross-object `pp_debug.o` audit is now complete. Its RX statistics mostly
independently confirm the already decoded `0x2010435c..0x2010439c` window.
The newly useful observations are `CTS_INTERRUPT` at `0x20104384`,
`TRIGGER` at `0x2010439c`, the four-word TX statistics bank
`0x20104e08..0x20104e14`, and the sparse diagnostic bank spanning
`0x201043b4..0x20104e64`. The complete functions apply no masks, so the SVD
uses the blob's exact labels while retaining opaque 32-bit values.
`dbg_lmac_rxtx_statis_dump` itself primarily reads software counters and adds
no fixed MMIO address.

The complete Wi-Fi inventory contains 69 distinct exported `dbg_*` symbols:
67 in `libpp.a` and two software-statistics dumps in `libnet80211.a`.
Within `libpp.a`, 46 live in `hal_debug.o`, 16 in `pp_debug.o`, two in
`lmac.o`, two in `hal_he_common.o`, and one in the MU-SIG-B test object.
The empty two-byte `pp_debug.o` stubs (`dbg_ebuf_loc_show`,
`dbg_his_lmac_*`, `dbg_lmac_init`, `dbg_perf_path_set/show`) contain only a
return instruction. `dbg_lmac_ps_statis_*`, `dbg_lmac_statis_dump`,
`dbg_perf_throughput_cal`, `dbg_cnt_lmac_drop` and `dbg_lmac_get_acs` access
software globals or caller-owned objects. They cannot add radio MMIO
addresses.

One adjacent non-`dbg_*` helper is directly useful to the beamforming
frontier. Complete `hal_debug.o::esp_test_get_bfr_avgsnr` reads
`0x20105f94[7:0]` as stream-zero average-SNR code and converts it to dB as
`(code + 88) / 4`; code `0x7f` is printed as a lower bound. This word is now
in the SVD and exposed through an integer-only PAC snapshot.

`test_hal_rx_mu_sigb.o::dbg_dump_rx_sigb` reads `0x20104028`, but mixes that
observation with an RX object and test-only parser state. It remains useful for
HE-SIG-B layout and MU receive testing, but is not promoted to an MMIO field
until that boundary is independently established.

By contrast, `hal_he_common.o::dbg_hal_check_set_mplen_bitmap` and
`dbg_hal_check_clr_mplen_bitmap` traverse software-owned allocation bitmaps;
they help recover the MPDU-link allocator but do not add fixed MMIO addresses.
`hal_debug.o::dbg_check_mutimer` likewise snapshots software history around a
MU timer rather than directly naming a new register bank.

`libnet80211.a` contains `dbg_hmac_*_statis_dump`, `esp_wifi_statis_dump`,
`bsscolor_event_dump` and TWT dump functions. They primarily expose software
state and protocol objects rather than raw MAC addresses, but they can connect
an already recovered register field to its higher-level 802.11 meaning.

The rev0 ROM ELF contains no callable `dbg_*` register decoder. Its
debug-related public symbols are mainly configuration leaves such
as `phy_chan_dump_cfg`, `phy_csidump_force_lltf_cfg`,
`phy_txcal_debuge_mode_` and `phy_pbus_debugmode`. The register-map recovery
value is consequently concentrated in blob `hal_debug.o`; ROM remains the
stronger oracle for complete PHY algorithms and fixed-address leaf behavior.
