# ESP32-S31 Wi-Fi source capabilities

This document describes the source-owned production boundary. It is not a
copy of the chip data sheet and it is not a hardware-qualification claim.
Register setters, frame parsers, diagnostics, and host-only state machines do
not establish an implemented feature unless the production owner graph carries the operation to, or from, the physical MAC boundary.

Implementation status applies only to the exact feature named by each row.
`IMPLEMENTED` is a source-composition claim, not a hardware test result.
A software substitute, a cold register default, or a parser for one input does
not raise the status of the hardware/protocol feature itself. Role, security,
channel, and PHY scopes are explicit and are not inherited between rows.

The status terms are:

- **IMPLEMENTED**: a bounded production path owns the complete operation for the
  scope stated in that row;
- **PARTIAL**: a source subset exists, but the named feature lacks a complete
  production owner or covers only the stated role/security/PHY subset;
- **FAIL-CLOSED**: a typed production path intentionally stops before
  activation or publication because required evidence is missing;
- **ABSENT**: there is no production protocol owner for the operation.

The [qualification specification](../../../../qualification/targets/esp32s31/wifi-sta.toml)
controls readiness and required hardware evidence independently.

Source ownership follows the [chip STA composition](sta/README.md),
[AP engine](ap/src/engine.rs), [MAC TX](mac/src/tx.rs) and
[MAC RX](mac/src/rx.rs). [Local STA](sta/src/profile.rs) and
[AP](ap/src/profile.rs) profiles select advertisements. Portable codecs and
role policy remain in [`driver/ieee80211`](../../../ieee80211/README.md);
concrete execution belongs to the [Embassy runtime](../../../runtime/README.md).

## Interfaces and operating modes

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Four virtual Wi-Fi interfaces | PARTIAL | The service owns exactly one STA VIF and one AP VIF on one channel context. Hardware address-match slots do not create protocol VIFs. ESP-NOW borrows the STA VIF and monitor mode is a tap, not another VIF. |
| Infrastructure station | IMPLEMENTED | Scan, authentication, association, Open/WPA2 data, reconnect, HT20/HT40 and HE20 station paths are present. |
| SoftAP | IMPLEMENTED | Open and WPA2-Personal AP service, association table, per-peer keys, beaconing, data, power-save buffering and teardown are present. The AP path is legacy/HT, not HE. |
| Simultaneous STA + SoftAP | IMPLEMENTED | One physical RX/TX/IRQ owner serves one STA and one AP on the same channel. A mismatch is rejected. During concurrent start, STA discovery is constrained to the AP channel; dynamic SoftAP channel following during a general STA scan is not implemented. |
| Promiscuous mode | PARTIAL | Standalone normalized monitor capture is implemented. Raw-DMA capture, protocol-validated capture, and monitor operation concurrent with STA/AP are not published capabilities. |

## Legacy and HT MAC behavior

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| RTS protection | FAIL-CLOSED | ERP Use Protection, all HT protection modes and finite HE TXOP-duration RTS thresholds are retained as typed BSS/peer policy. The initial rate and every retry rate are preflighted before sequence, PN or DMA publication; a protection-required transaction returns `PhysicalPublicationUnverified`. There is no complete RTS formatter, queue image or completion owner, and no on-air capability is claimed. |
| CTS-to-Self protection | FAIL-CLOSED | Typed TX-protection policy identifies group exchanges that require CTS-to-Self and rejects them before sequence, PN or DMA publication. There is no CTS frame, queue-publication or completion owner; an `SW_CTS` register name or cold queue bit is not a physical implementation. |
| Immediate Block ACK | PARTIAL | Negotiated immediate BlockAck, hardware bitmap capture/matching, software RX reorder, retained TX MPDUs and selective retry are implemented for the bounded WPA2/QoS STA/AP aggregate paths. STA TX owns TIDs 0/7/5, AP TX owns TID 0, and Open links do not start a BlockAck agreement; other security/TID combinations are not claimed. |
| Fragmentation and defragmentation | PARTIAL | Bounded STA/AP RX reassembly is source-owned for Open and individually addressed WPA2-Personal/CCMP Data or QoS Data. Each role owns two fixed contexts of 1,508 retained MSDU bytes: an 8-byte LLC/SNAP header plus at most one 1,500-byte Ethernet payload. CCMP admission requires hardware authentication, a separate PN per fragment, exact Retry fingerprints and final replay commit before the sole Ethernet publication. TX, group/four-address and protected A-MSDU fragmentation remain absent; no on-air qualification is implied. |
| TX/RX A-MPDU | PARTIAL | Bounded WPA2-Personal/CCMP A-MPDU is source-owned for HT/HE STA and HT AP: negotiated per-TID BA windows, RX reorder, retained TX MPDUs and selective retry are connected. Open connected TX is deliberately non-QoS and an Open AP has no BlockAck owner. Other security/TID combinations and HE trigger-based aggregation are not claimed. |
| TX/RX A-MSDU | PARTIAL | STA/AP RX decapsulation is present for admitted Open and authenticated WPA2 data. TX scopes differ: STA coalesces one exact two-frame WPA2 protected pair; AP coalesces one exact Open HT/QoS pair, or a WPA2 TID-0 pair only when the operational BA agreement echoed A-MSDU support. Each TX path is bounded to 3,839 bytes and checks peer/order, descriptor+MIC+FCS capacity and fallback before sequence/PN publication. General multi-lease scheduling and the larger class are absent. |
| TXOP | PARTIAL | This is a STA-only production subset: negotiated WMM TXOP values reach per-AC station policy and bound the HE A-MPDU APEP ceiling; nonzero HT TXOP duration is rejected. AP TX has no negotiated TXOP owner, and neither role has a general multi-PPDU TXOP scheduler. |
| WMM | PARTIAL | STA owns WMM parsing, DSCP/VLAN classification, ACM downgrade, four per-AC EDCA queues, TID selection and BA routing. AP advertises a fixed WMM parameter set and its bounded QoS/A-MPDU path uses BE/TID-0, but it has no per-peer parameter negotiation, network classification or four-AC scheduler. U-APSD is disabled in both roles. |
| 20/40 MHz 802.11n, MCS0-MCS7 | IMPLEMENTED | HT20/HT40 one-stream rate selection, SGI/LGI formatters, retry schedules and the 150-Mbit/s HT40 MCS7 SGI formatter path are present. |
| HT Duplicate MCS32 | FAIL-CLOSED | Peer capability and HT40/MCS32 RX normalization are retained, while local capability advertisement remains clear. Physical TX needs an exact descriptor/PLCP/HT-SIG/length/protection/power/retry oracle and separate on-air qualification. |

## Security

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Open BSS | IMPLEMENTED | STA and AP plaintext management/data paths are present. |
| WPA2-Personal / CCMP | IMPLEMENTED | STA supplicant and AP authenticator paths own the four-way handshake, strict RSN selection, pairwise/group keys, CCMP PN exhaustion, RX replay admission and key teardown. STA GTK rekey is atomic; AP GTK rotation has no complete Group-Key Handshake/acknowledgement owner. |
| WPA2-Enterprise | ABSENT | There is no 802.1X/EAP supplicant/authenticator or enterprise credential owner. |
| WPA3-Personal / WPA3-Enterprise | ABSENT | SAE, transition policy, enterprise authentication and their lifecycle are not present. |
| BIP / protected management frames | ABSENT | PMF negotiation, IGTK/BIGTK ownership, management replay and BIP MIC verification/publication are not present. |
| GCMP, TKIP, WAPI and WEP | ABSENT | The public security domain intentionally exposes only Open and WPA2-Personal/CCMP. No other cipher is advertised or selected by fallback. |

## TSF, beacon monitoring, and power saving

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Hardware TSF | IMPLEMENTED | Coherent station TSF reads, AP TSF lifecycle and beacon timestamps are owned. |
| Automatic hardware beacon monitoring | FAIL-CLOSED | Software beacon-loss monitoring, active probe recovery and TIM/DTIM parsing are implemented, but they are not the automatic hardware feature. A one-shot epoch binds BSSID+AID to STA-policy readback and validates the four-bit miss limit, then stops before MMIO; `automatic_monitor_active()` remains false because raw timeout units, automatic-filter lifecycle and the exact WDEVPWR cause are unproven. |
| Legacy station power save | PARTIAL | PM=1/PM=0 transitions are ACK-gated; TIM/DTIM/listen interval, PS-Poll and doze permits are source-owned. The S31 boundary probes and rolls back only reviewed wake-prefix fields and does not claim RF/PHY/BB/clock sleep. Passive connected ESP-NOW RX is rejected when this policy is enabled, and concurrent STA+AP remains always awake. |
| TWT requester | FAIL-CLOSED | Portable individual-TWT parsing, bounded requester state, deadlines, teardown and TSF wake planning exist. Explicit agreements are not activated: a local explicit proposal reports the missing TWT Information action/update semantics, and a peer-accepted explicit agreement is queued for immediate teardown rather than installed. S31 implicit agreement admit/install/remove also stops before schedule activation because coexistence mapping, wake compare, retention and restore ordering are missing; the capability bit stays clear. |
| Intra-PPDU power save | FAIL-CLOSED | HE peer setup writes the recovered MAC intra-PPDU and BSS-color-check bits, but no connected owner binds detection to a safe RF/PHY/BB/clock stop/wake transaction. Register setup alone is not a power-saving operation, so no runtime or energy capability is claimed. |

## 802.11ax / HE

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| HE20 non-AP, 1T1R, MCS0-MCS9 | IMPLEMENTED | Associated STA HE20 SU S-MPDU/A-MPDU, BCC/LDPC, supported GI/LTF combinations and DCM profiles are source-owned. The SoftAP does not advertise or transmit HE. |
| Multiple BSSIDs | ABSENT | The capability bit is clear, association programs BSSID index zero, and there is no nontransmitted-profile scan/association owner. |
| Triggered response scheduling | FAIL-CLOSED | Basic Trigger parsing, association/AID validation, bounded response deadlines and queue/MPLEN/BSR preparation exist. Production configuration disables the path, and the final HE-TB PHY vector/doorbell transition is explicitly unverified. |
| MU-RTS, MU-BAR, Multi-STA BA | ABSENT | Trigger-type parsing alone is present. There is no complete scheduling, response, bitmap, timeout or retry owner for these exchanges. |
| Two NAVs | ABSENT | No independent intra-BSS/basic NAV state and update lifecycle is exposed by production code. |
| BSS coloring | PARTIAL | The associated peer's effective color is parsed, programmed and placed in HE SU TX vectors. Collision detection, color-change announcements and dynamic recoloring are absent. |
| Spatial reuse | ABSENT | RX HE metadata can expose spatial-reuse fields, but TX uses zero and there is no OBSS-PD, SRP admission, CCA-threshold or collision-policy owner. |
| Uplink power headroom | PARTIAL | Wire decoding and the vendor-shaped UL-MU power capability element exist. There is no connected runtime producer/consumer that closes the UPH exchange. |
| Operating Mode Control | PARTIAL | Exact PAC fields and optional software HE-Control queue images exist, but no connected OMC negotiation/publication lifecycle invokes them. |
| Buffer Status Report | PARTIAL | HE BSR initialization, queue byte accounting and hardware-generated control-field geometry are recovered. Ordinary production frames do not request the hardware BSR path, and trigger-based publication remains fail-closed. |
| TXOP-duration RTS threshold | FAIL-CLOSED | A finite peer threshold is parsed and retained in the STA TX-protection policy. A missing HE duration or an exchange requiring RTS is rejected before sequence, PN or DMA publication rather than treated as disabled or as a generic byte threshold; the protected physical image remains unproven. |
| UORA | ABSENT | Cold initialization writes the recovered 7/31 contention-window defaults, and portable Trigger parsing can recognize random-access RU fields. Neither is a UORA owner: there is no OBO/backoff, RU eligibility, attempt/result state machine or UL-OFDMA random-access publication path. |
| Uplink OFDMA | FAIL-CLOSED | The Trigger/RU schedule can reach the prepared-queue frontier, but the HE-TB PHY publication oracle is missing. |
| Downlink OFDMA / MU-MIMO | PARTIAL | HE-MU common metadata and bounded HE20 SIG-B user decoding exist. There is no dated production payload qualification or complete wider-bandwidth layout, so metadata parsing is not promoted to a connectivity claim. |
| Beamformee | FAIL-CLOSED | NDPA detection, association binding and report-rate hardware setup exist. Feedback formatting/publication is unverified and all dependent capability bits are cleared. |

## Frequency, antenna, FTM, and ESP-NOW

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| 2412-2484 MHz | PARTIAL | Channels 1-13 are implemented. Channel 14 / 2484 MHz is representable in portable types but is rejected by the S31 PHY while the required channel-14 MIC/RF/regulatory behavior is unproven. |
| Antenna diversity | FAIL-CLOSED | PAC/HAL own a reviewed enable bit and cold antenna initialization, but there is no public RF/GPIO antenna-selection owner, runtime policy, board contract or HIL claim. |
| 802.11mc FTM | FAIL-CLOSED | Allocation-free Public Action codecs and a bounded single-burst ASAP requester own peer/token identity, retries, deadlines and raw four-timestamp exchanges. The S31 frontier validates a prepared request and can reversibly probe the recovered PHY-enable leaf, then rejects before connected Action publication. RX/TX antenna-point timestamps, clock and calibration contracts, capability advertisement, distance results and on-air operation remain unclaimed. |
| ESP-NOW v1/v2 plaintext | IMPLEMENTED | Bounded peer/channel ownership, v1/v2 encode/decode, standard DSSS1M, OFDM and HT20 MCS0-MCS7 LGI/SGI TX/RX paths and retry publication are present on the STA VIF. Standalone RX is always awake; connected passive RX is admitted only when legacy station power save is disabled because it has no independent wake owner. |
| ESP-NOW encrypted peers | FAIL-CLOSED | Portable LMK/peer ownership exists, but S31 key-selector and Action-frame AAD/CCMP contract are unproven, so encrypted publication is rejected. |
| ESP-NOW Long Range PHY | FAIL-CLOSED | Exact low-rate identities, retry records and a reversible low-rate-gate probe are retained. Physical TX requires an affine gate epoch spanning enable, doorbell, completion/retries and exact restore; RX needs matching gate ownership and descriptor-rate normalization. Those contracts plus the LR PLCP/queue vector are missing, so LR never publishes or masquerades as a standard observed PHY. |

## Physical publication preconditions

The following contracts are absent or unverified at their production boundary.
Each corresponding path rejects activation or keeps its capability bit clear:

1. MCS32 and ESP-NOW LR descriptor, PLCP, queue, power and retry images, plus
   an affine LR gate epoch through completion and RX normalization;
2. RTS/CTS duration, basic-rate, power, retry and completion ownership after
   the typed protection-policy frontier;
3. station WDEVPWR compare/cause binding plus RF/PHY/BB/clock sleep and exact
   wake/rollback ordering;
4. HE-TB RU/GI/LTF/MCS/DATA_LENGTH vector and final doorbell publication;
5. beamforming feedback memory/formatter/publication and FTM timestamp paths;
6. channel-14 RF/MIC/regulatory setup.

Protocol preparation cannot grant sequence, PN, replay, DMA or capability
publication when the corresponding physical transaction is unproven.
