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

Complete `dbg_dump_rx_ppdu` is also an independent instruction-level oracle
for the public S31 RX-control prefix. Its `0xd0`-masked format test routes both
HE SU (`4`) and HE extended-range SU (`6`) through the same SIG-A decoder. The
format-five branch at `0x4b4..0x586` decodes HE MU SIG-A1 from prefix bytes
`0x04..0x07` and SIG-A2 from `0x09..0x0a`, including SIG-B MCS/DCM,
three-bit bandwidth, compressed-SIG-B count, GI/LTF, Doppler, STBC and
padding. The format-seven branch at `0x412..0x4b2` independently decodes the
HE trigger-based common SIG-A view. These exact packet-metadata layouts now
live in `open-esp-radio-esp32s31-wifi-mac::rx::{HeSuSignal,HeMuSignal,
HeTriggerBasedSignal}`. This restores passive downlink/TB vector observation;
it does not by itself qualify payload reception, OFDMA scheduling or MU-MIMO.

The same complete body closes the location of S-MPDU status. At `0xf8` it
loads RX-prefix byte `0x1f`, at `0x11a` it isolates bit zero, and passes that
value as the final `cur_single_mpdu` argument of the common PPDU record. The
pinned Espressif header defines this as the IEEE VHT/HE S-MPDU form, not as
the inverse of arbitrary A-MPDU membership. A 2026-08-04 HE20 downlink proved
exact propagation: 117,190 benchmark-port datagrams and 147 connected Beacons
were not-S-MPDU, with zero unavailable provenance and exact payload delivery.
The Beacon result rejects the earlier over-broad physical-A-MPDU
interpretation. The format-two branch independently closes HT aggregation:
at `0x592..0x5e0` it extracts word bit 27 as the sixth argument named
`Aggregation`; normalized metadata uses that direct bit for HT. VHT/HE
A-MPDU containment is separately protocol-validated from the PPDU format,
because IEEE defines their S-MPDU as the sole MPDU in an A-MPDU; it is not
misreported as another hardware field. A forced HT20/MCS7/SGI downlink observed the direct
bit set for all 78,127 benchmark records with zero unavailable provenance.
See the [S-MPDU qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-rx-s-mpdu-metadata.md)
and the [direct HT aggregation qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-ht-rx-aggregation-metadata.md).
A separate [HE containment qualification](../qualification/targets/esp32s31/records/2026-08-04-esp32s31-he-rx-ampdu-containment.md)
proved 117,190 protocol-sourced true values with zero hardware or unavailable
classification.

The two complete per-user leaves close the next bounded word format.
`dbg_dump_musigb_non_mimo` (size `0x6e`) names STA-ID, NSTS, beamformed,
MCS, DCM and coding in one 21-bit non-MIMO word and treats STA-ID `0x7fe` as
a terminal non-MU-MIMO marker. `dbg_dump_musigb_mimo` (size `0x4a`) names
STA-ID, spatial configuration, MCS and coding in the MIMO view. Allocation-free
decoders live in `open-esp-radio-ieee80211::he`. The separate complete-SIG-B
container is anchored by complete
`test_hal_rx_mu_sigb.o::dbg_dump_rx_sigb` (size `0x1e6`): it publishes the
bit length and presence flag at RX-prefix offsets `0x2a..0x2b`, the common
and selected-user words at `0x2d..0x2f` and `0x28..0x2a`, and borrows the
optional complete bytes from `0x38`. The bounded Rust view checks that variable
length before producing a slice.

The two complete test parsers prove one bounded iterator and keep the other
layouts closed. `test_get_nonmumimo_common` proves common-field lengths of 18
bits for bandwidth selectors 0/1, 27 bits for 2/4/5 and 43 bits for 3/6/7.
`test_rx_parse_nonmumimo_complete_sigb` (size `0x3e4`) extracts the HE20
21-bit user words at absolute complete-stream bit offsets
`18,39,70,91,122,143,174,195,226`. Its exact count expression is
`(remaining / 52) * 2 + (remaining % 52 != 0)`: each pair is two 21-bit users
plus ten intervening CRC/tail bits. `He20MuSigBNonMimoUsers` now reproduces
that geometry without allocation, retains the raw word/bit offset and rejects
short streams or more than the blob's nine unrolled users. Wider bandwidths
are not folded onto the 18-bit common prefix. The complete compressed/MU-MIMO
parser (size `0x20c`) independently extracts at most four user words at
non-linear bit offsets `0,21,52,105`; its explicit third/fourth length guards
are `>72` and `>135`. `He20MuSigBMimoUsers` preserves that separate layout and
uses the one-based compressed user count from HE-SIG-A1. It rejects wider
bandwidth, counts outside one through four and truncated fields rather than
pretending that compressed users follow the non-MIMO pair geometry.

The adjacent compressed spatial-configuration lookup is also fully owned.
Complete `test_hal_rx_mu.o::{mumimo_spatial_cfg_get_nsts,
mumimo_spatial_cfg_get_nsts_tot}` (sizes `0x10e` and `0x44`) index seven
eight-byte-stride ROM objects for two through eight users. The exact objects
are `esp32s31_rev0_rom.elf::mu_mimo_special_cfg_user_num_2..8` at
`0x2f84fee8,0x2f84fe80,0x2f84fe28,0x2f84fdf0,0x2f84fdd0,0x2f84fdc0,
0x2f84fdb8`. `HeMuMimoSpatialConfiguration` validates the table domain,
returns each user's NSTS and reproduces the bounded total. The compressed
iterator additionally verifies the blob invariant that every decoded user
carries the same spatial-configuration encoding. Invalid encodings stay
typed failures rather than indexing ROM.

The non-compressed HE20 common RU Allocation is now owned as well. Complete
`test_hal_rx_mu_sigb.o::test_get_nonmumimo_common` (size `0xf6`) proves that
bandwidth selector zero reads the first complete-SIG-B byte and uses an
18-bit common prefix. Complete `test_hal_rx_mu.o::get_user_num` (size
`0x2e2`) maps that byte plus a zero-based user position to user count, RU
type and the numeric vendor `multiplexed` output. Its only data dependencies
are revision-v0 ROM `sigb_common_ru_allocation` at `0x2f84ff38` (16 rows of
nine bytes) and `sigb_ru_allocation_user_num` at `0x2f84ffc8` (16 bytes).
`He20MuSigBRuAllocation` reproduces every narrow computed class, retains the
exact numeric multiplexing output, rejects reserved encodings and rejects
the two RU types that the adjacent complete `rutype2str` does not support.
The complete-user iterator additionally requires the RU Allocation's user
count to match the count derived from the SIG-B bit length.

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

`HeTriggerScheduledRate::from_trigger_frame` closes the multi-user selection
boundary without assuming that the station is the first scheduled user. It
consumes the complete instruction-proven iterator, selects the associated
AID12, and rejects a missing assignment, duplicate assignment, malformed
trailing user, unsupported layout or padding-hidden assignment before
constructing a rate. This remains a software admission result until the
Trigger/TB hardware counters advance in HIL.

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

The report-rate write path is also complete. `trc.o::trc_set_bf_report_rate`
(size `0x52`) selects one of three policies from a signed link metric and two
now-mapped peer values at temporary vendor-record offsets `0x8f`/`0x90`.
Complete `wl_cnx.o::ic_set_sta` loads offset `0x8f` from the inversion of peer
state bit `0x35c[10]`; complete `ieee80211_parse_heopr` both stores that bit
from complete HE Operation IE byte five bit zero and names it
`ER-SU-Disable` in its pinned format string. Thus the consumed nonzero value is
ER-SU permitted. Offset `0x90` is HE PHY capability byte three bits 4:3,
which complete `ieee80211_parse_hecap` names the receive DCM constellation;
the report leaf only distinguishes zero from nonzero. It first calls
`hal_mac_ctl.o::hal_he_set_bf_report_rate`, which publishes three replicated
profiles at `0x20104464`, then tail-calls
`hal_he_set_ersu_ack_rate` (size `0x4e`). That second leaf writes `0x80`, or
`0xa0` for the low-metric DCM plus ER-SU branch, through four separate
low-to-high byte RMWs at `0x20104404`. The MAC rate policy and PAC transaction
now preserve this complete ordering. This closes report-rate selection; it
does not yet implement sounding or compressed feedback generation.

The complete `test_hal.o::esp_test_bf_set_feedback` function (size `0x182`)
adds two test-control words, but not a production sounding path. Its zero
enable branch clears `0x20104e00[23]`; the enabled branch sets that bit,
publishes two seven-bit arguments in bits 13:0 and applies bounded selector
cases zero through sixteen across bits 22:14 and companion bits
`0x2010409c[3:2]`. These fields are recorded in the SVD with deliberately
approximate submode names. The archive contains no selector enum names and no
production caller, so exposing a semantic Rust setter would overstate the
evidence.

Complete `wdev.o::is_ndpa_to_dut` (size `0x7e`) closes the receive admission
boundary before feedback generation. For an HE NDPA it extracts the six-bit
Dialog Token from byte 16, walks four-byte STA Info words from byte 17 and
matches each low eleven-bit AID against `hal_he_get_aid`. The allocation-free
`open-esp-radio-ieee80211::ndpa` parser reproduces that geometry after the
RX owner strips the four-byte FCS and rejects malformed partial STA Info.
The only production caller is complete `wdev.o::wDev_ProcessRxSucData` (size
`0x6a0`, call at `0x46a`). It feeds the Boolean result only to the adjacent
NDPA diagnostic `wifi_log`; there is no software feedback callback, allocation
or buffer publication on that path. Together with complete `hal_init_bf` and
the `WDEVAXDIAG0/3` beam/NDP/feedback fields, this proves that report sequencing
is hardware-owned. The typed PAC exposes those six non-latched progress fields
through `he_beamforming_diagnostics`; it still does not prove a successful
report without a real sounding exchange.
`HIL_OPEN_HE_NDPA_AX211_2026_07_30` then observed zero HE NDPA frames in four
successive ten-second windows after the open driver associated with the
controlled AX211 HE20 AP as AID 1. The same run completed WPA2, protected ARP
and a three-subframe A-MPDU/BlockAck exchange. Readback confirmed the hardware
sequence enabled with selector five, HE beam reporting enabled, BFRP time 16
and NDP time 113. With no NDPA, three subsequent best-effort diagnostic samples
had beam, BFRP detection, NDP success and feedback status all clear and both
timers zero. This is a negative sounding baseline for that AP configuration,
not evidence that the parser or S31 feedback hardware was exercised.

The preserved monitor capture `esp32s31-he-oracle-fixed-ch11.pcapng`
(SHA-256
`d50289842bd3cddbcebf3080c049cf6d6b387908b501b6b7333fbfb250e7abde`)
supplies a positive, complete vendor-firmware air oracle. Frame 1374 is a
21-byte FCS-stripped HE NDPA from BSSID `dc:15:c8:54:bc:1e` to the S31 at
`30:ed:a0:f3:f6:d0`: Dialog Token `0x37` and STA Info word `0x0820001d`
select AID 29, HE20 RU indices 0 through 8, feedback/Ng encoding zero,
disambiguation one, codebook zero and Nc zero. Frame 1375 is the required
following HE NDP. Frame 1376, 14.39 us later, is the S31's Action-No-Ack
HE Compressed Beamforming and CQI response. Its 40-bit MIMO Control
`0x0dc4008208` selects one column, two rows, 20 MHz, no grouping, codebook
one, SU feedback, one first/final segment, RU indices 0 through 8 and the
same Dialog Token; average-SNR byte `0x14` precedes an 80-byte feedback
matrix. `open-esp-radio-ieee80211::ndpa` now reproduces the exact NDPA and
parses that fixed response header without allocation. The matrix remains
opaque until an instruction-level or standard-backed angle-layout owner is
implemented. This capture also makes the HIL boundary explicit: injecting
NDPA alone cannot qualify beamformee behavior because the NDP between the
announcement and report is semantically required.

The same capture's frame 7624 closes the complete vendor HE association
request before that sounding exchange. Before the existing HT/HE tail, the
STA uses listen interval three and emits Supported Rates, an RSN element with
SPP A-MSDU-capable bit `0x0400`, Power Capability `[-11,20]` and Extended
Supported Rates in that exact order. Complete
`libnet80211.a[ieee80211_output.o]::ieee80211_assoc_req_construct` anchors the
ordering and conditionally calls
`libnet80211.a[ieee80211_he.o]::ieee80211_add_power_cap`; that leaf obtains the
minimum from `hal_get_tx_min_pwr` and the maximum from
`hal_get_tx_pwr(16,1)`. Complete `libpp.a[hal_mac_ctl.o]::hal_he_init` anchors
the installed minimum at -11. The remaining tail is HT Capabilities, HE
Capabilities, HE UL MU Power Capabilities, WMM Information and Extended
Capabilities. Complete
`libnet80211.a[ieee80211_he.o]::ieee80211_add_ulmu_pwrcap` obtains the primary
power index for MAC rates 16 through 25 and publishes the nine unsigned
differences from rate 16 after Extension ID 60 and two reserved zero bytes.
The open encoder now takes the minimum, maximum and calibrated indices through
typed, allocation-free Rust values; it retains no ROM callback or C-layout
parameter.
`HIL_OPEN_HE_ASSOC_COMPLETE_FRITZ_2026_07_30` measured live indices
`[20,20,20,20,19,19,18,18,16,15]`, hence differences
`[0,0,0,1,1,2,2,4,5]`. The FRITZ accepted the first association request as
AID 21, and WPA2, protected ARP, TX/RX AddBA and the first three-subframe
A-MPDU all completed with zero driver drops. A subsequent 175-packet,
35-second ICMP uplink had zero loss and 3.667 ms mean RTT. Neither that uplink
window nor the preceding idle observation contained an HE NDPA or Trigger;
the complete capability tail was therefore HIL-qualified as an association
contract, not as proof of sounding or OFDMA scheduling.
`HIL_OPEN_HE_ASSOC_COMPLETE_BODY_FRITZ_2026_07_30` then added the preceding
listen interval, RSN, Power Capability and IE ordering from the same frame
7624. The AP again accepted the first request, now as AID 16; WPA2, TX/RX
AddBA and a three-subframe TX A-MPDU completed with zero driver drops, and a
175-packet, 35-second ICMP uplink again had zero loss. The 48-second serial
window still measured HE NDPA count zero and Trigger count zero. Thus matching
the complete association body does not activate the missing sounding/OFDMA
scheduler; the remaining boundary is post-association node, rate-control or
traffic-ID lifecycle.

`HIL_OPEN_STA_SEQUENCE_SPACES_2026_07_30` then reproduced the complete
`libnet80211.a[wl_cnx.o]::cnx_auth_done` TID0, TID7, TID5 AddBA order and the
shared Dialog Tokens 1, 2, 3. Complete
`libnet80211.a[ieee80211_ht.o]::ieee80211_ampdu_request` instructions
0x9a..0xa2 load the advertised SSN from the node's TID-indexed halfword at
`(tid + 0x50) * 2 + 0x0e`. This exposed a real open-path error: the former
interface-global sequence counter let the three Action frames advance the
TID0 data SSN. The replacement allocation-free Rust owner has one non-QoS
counter and sixteen independent QoS/TID counters. In the corrected FRITZ
capture, Action frames 806, 811 and 816 use management sequence numbers
2869, 2870 and 2871 while advertising TID0 SSN 2866 and TID7/TID5 SSN 2865.
The preceding protected TID0 ARP is frame 792 at sequence 2865; the first
three-subframe A-MPDU is frames 821..823 at TID0 sequences 2866..2868.
Hardware BlockAck reports the same start 2866 and bitmap `0x7`. All three
agreements became operational, 175/175 ICMP packets completed with 4.252 ms
mean RTT, and the driver reported zero RX drops. The same observation still
contained no HE NDPA or Trigger, so exact BlockAck session order, token
ownership and per-TID SSNs are necessary but do not by themselves activate
sounding or OFDMA.

The next complete post-association chain is
`libnet80211.a[wl_cnx.o]::ic_set_sta` ->
`libpp.a[if_hwctrl.o]::ic_set_trc` -> `libpp.a[trc.o]::rcUpdatePhyMode` ->
`trc_set_bf_report_rate`. `ic_set_sta`'s own format string names the two
signed scalar inputs `rssi` and `nf`; `ic_set_trc` subtracts them and narrows
the result to a signed byte immediately before the tail-call to
`rcUpdatePhyMode`. At instruction 0x26c that complete 0x43e-byte body calls
`trc_set_bf_report_rate` after selecting the per-peer schedule. The open
driver now owns this as `StaRateControlAssociation`: typed PHY family, signed
link metric, schedule references and BF report profile, with no retained
0x98-byte vendor C-layout record.

The formerly generic schedule-extension boolean is also identified. Complete
`libnet80211.a[ieee80211_phy.o]::ieee80211_setup_lr_rates` clears peer bytes
`0x83..0x86`, stores the vendor LR-only flag at `0x83`, and increments the
count of locally matched LR rates at `0x84`. `ic_set_sta` passes a pointer to
that exact pair; `ic_set_trc` copies the count to temporary record byte
`0x8b`, while a nonzero LR-only flag forces PHY type six. `rcUpdatePhyMode`
tests the count to admit the LoRa fallback and the extra maximum indices.
The Rust input is consequently named `long_range_rates_present`; it is not an
HE capability and not a generic request for extended schedules.

The noise-floor producer is independently complete. Rev0 ROM
`phy_read_hw_noisefloor` at `0x2f827d72`, size `0x1a`, reads
`0x2010708c[11:0]`, subtracts `0x1000`, sign-extends and arithmetic-shifts by
two. Complete `libpp.a[wdev.o]::wDev_GetNoiseFloor`, size `0x36`, adds two,
shifts by two again and retains a signed byte. The SVD now names the read-only
sixteenth-dB source field and the handwritten PAC exposes the exact whole-dB
value; association computes the blob-compatible wrapping `RSSI - NF` metric.

`HIL_OPEN_STA_RATE_CONTROL_2026_07_30` exercised that complete chain on rev0
with the standard PSRAM/PSRAM image and the nearby FRITZ HE20 peer. The typed
PAC read `-92 dBm`; the scan result was `-20 dBm`, so the exact signed-byte
subtraction produced metric 72. The Rust-owned association selected Dot11Ax
schedule 1, rate code `0x19`, maximum schedule 13, 16 schedule entries and BF
report mode/rate 1/16. WPA2/CCMP, all three TID AddBA agreements, a
three-subframe TX A-MPDU and DHCP then completed. A 100-packet ICMP run had
zero loss and 3.692 ms mean RTT; the driver reported zero RX drops and
duplicates. That peer advertised ER-SU-Disable and no DCM receive
constellation, so both low-metric gates were false. Metric 72 takes the
complete `> 13` branch before those gates can affect the result; a capable,
low-metric peer is still required to HIL-qualify their alternative branch.
A reset repetition after the typed-polarity correction read HE Operation
`0x010004`, logged `ersu_disabled=true`, `ersu_permitted=false`, retained
hardware `AUTO_ACK_ALLOW_ERSU=false`, selected the same schedule/profile and
again completed WPA2, all three AddBA agreements, TX A-MPDU and DHCP.

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
`phy_txcal_debuge_mode_` and `phy_pbus_debugmode`. ROM therefore remains the
stronger oracle for complete PHY algorithms and fixed-address leaf behavior,
but `hal_debug.o` is not the only strong blob register-map source.

## Additional PHY, BT and test-HAL register oracles

The pinned `_oracles/libphy.a` (SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`)
contains three complete `phy_debug.o` functions that materially extend the
map:

- `phy_reg_check` (`0x3d2` bytes) walks fixed, word-aligned, half-open ranges
  and prints the block names. These ranges are recorded in the project
  evidence catalogs and checked against `memory.toml`. They prove finite
  occupied dump windows, not individual register names.
- `phy_i2c_check` (`0x1f6` bytes) proves ten bank IDs, hosts and index bounds.
  `PHY_I2C_COMMAND_RAM.BLOCK` now exposes those IDs as enumerated values.
- `phy_pbus_print` (`0xf4` bytes) associates selector/path pairs with RFRX,
  RFTX, BB and DCO. The association is recorded on the command fields without
  over-naming the packed result windows.

`rfpll_cap_check` and ROM `phy_read_pll_cap` additionally establish the
logical-bank layout `RFPLL[5] = IR_CAP_EXT[7:0]`,
`RFPLL[7].bit2 = IR_CAP_EXT[8]`, and `RFPLL[6] = IR_DAC_EXT[7:0]`. These are
PHY-I²C bank registers, not direct MMIO registers, so they are retained here
as evidence until the PAC has a typed logical-bank layer.

The pinned `_oracles/libbtbb.a` (SHA-256
`ebd67c6a32081d0fcd143bbb0ed53f962fc587058e20eceb59012beae47528df`)
is a second major source:

- complete `bt_bb_v2.o::ble_tx_config_check` (`0x1c0` bytes) proves the two
  Bluetooth delay bytes in the canonical FECTRL word at `0x20100870`, plus
  timing fields in BT-only registers;
- paired `bt_cte.o` setters and getters independently prove field position and
  width for CTE and BTAGC registers. Examples include
  `0x20102124[21:14] CTE_DELAY_RX_AOA`,
  `0x20102128[31:24] CTE_ANT_DELAY_TX_AOD_1US`,
  `0x20102164[0] CTE_SQI_EN`, and
  `0x20102898[29:21] AGC_TARGET_VALUE`.

The complete BLE TX checker specifically decodes
`0x201020b8[30:23] TX_CCA_START_TS`,
`0x201020b8[19:12] TX_CCA_END_TS`,
`0x20102014[26:19] BB_TX_ON_DELAY`,
`0x20102014[15:11] TX_RAMP_DELAY`,
`0x2010201c[31:27] TX_RAMPUP_DELAY`,
`0x20100870[7:0] RF_ON_BT_DELAY_CODE`,
`0x20100870[15:8] PA_ON_BT_DELAY`, and
`0x20101650[7:0] LC_TX_ON_DELAY`. Its printed `CONSTANT_DC_TIME` is the
bounded subtraction
`LC_TX_ON_DELAY - BB_TX_ON_DELAY - TX_RAMP_DELAY - TX_RAMPUP_DELAY`.
`bt_bb_ble_diag` independently proves three selectors at
`0x2010210c[15:13]`, `[19:16]` and `[23:20]`, but no pinned string gives them
a stronger meaning than `DIAG_*_SELECT`.

The BT-only addresses are deliberately not introduced as independent
peripheral singletons in the Wi-Fi-oriented SVD. They belong in one canonical
BT-radio map; duplicating them here would create two Rust owners for one
physical register.

Finally, pinned `_oracles/libpp.a` supplies two previously omitted sources.
Complete `hal_debug.o::print_isr_regs` (`0x5a` bytes) reads and names the WDEV,
WDEV_INT1 and WDEVPWR ENABLE/RAW/STATUS banks. WDEV_INT1 ENABLE aliases the
already writable HE parent word at `0x20104c2c`, so it remains one canonical
register; its RAW/STATUS words and the WDEVPWR diagnostic bank remain read-only
until a writer proves their side effects. Complete
`test_hal.o::esp_test_get_csi_dump_cfg` (`0x8a` bytes) proves the fields at
`0x2010411c` and the `RXBFRPTO_MS` field at `0x20104c78[7:1]`.
`esp_test_set_csi_dump_cfg` (`0x22e` bytes) has a reproducible vendor defect:
`cfg[8]` gates the BEAMFORMED update, but `cfg[1]` supplies the written bit.
The SVD describes the physical bit and records the defect; the generated PAC
does not emulate it.
