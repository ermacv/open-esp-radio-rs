# ESP32-S31 Wi-Fi feature frontier

This document describes the source-owned production boundary. It is not a
copy of the chip data sheet and it is not a hardware-qualification claim.
Register setters, frame parsers, diagnostics, and host-only state machines do
not count as a live feature unless the production owner graph can carry the
operation to, or from, the physical MAC boundary.

The status terms are:

- **LIVE**: a bounded production path owns the complete operation;
- **PARTIAL**: a useful subset is live, but the data-sheet feature is not
  complete;
- **FAIL-CLOSED**: the request and its missing evidence are typed, but no
  physical publication is made;
- **ABSENT**: there is no production owner for the operation.

Qualification remains independently controlled by the ESP32-S31 Wi-Fi
ledger and dated HIL records.

## Interfaces and operating modes

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Four virtual Wi-Fi interfaces | PARTIAL | The service owns exactly one STA VIF and one AP VIF on one channel context. Hardware address-match slots do not create protocol VIFs. ESP-NOW borrows the STA VIF and monitor mode is a tap, not another VIF. |
| Infrastructure station | LIVE | Scan, authentication, association, Open/WPA2 data, reconnect, HT20/HT40 and HE20 station paths are present. |
| SoftAP | LIVE | Open and WPA2-Personal AP service, association table, per-peer keys, beaconing, data, power-save buffering and teardown are present. The AP path is legacy/HT, not HE. |
| Simultaneous STA + SoftAP | LIVE | One physical RX/TX/IRQ owner serves one STA and one AP on the same channel. A mismatch is rejected. During concurrent start, STA discovery is constrained to the AP channel; dynamic SoftAP channel following during a general STA scan is not implemented. |
| Promiscuous mode | PARTIAL | Standalone normalized monitor capture is live. Raw-DMA capture, protocol-validated capture, and monitor operation concurrent with STA/AP are not published capabilities. |

## Legacy and HT MAC behavior

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| RTS protection | FAIL-CLOSED | Peer/protection policy can be retained without silently setting raw queue bits, but the complete RTS formatter, duration/basic-rate/power image, retry behavior and completion lifecycle are not source-proven. |
| CTS-to-Self protection | FAIL-CLOSED | No complete physical CTS-to-Self queue transaction is published. It must remain independent from RTS policy and from unrelated `SW_CTS` register names. |
| Immediate Block ACK | LIVE | Negotiated immediate BlockAck, hardware bitmap capture/matching, software RX reorder, retained TX MPDUs and selective retry are live for the bounded QoS STA/AP aggregate paths. This is not a claim for every security/TID combination. |
| Fragmentation and defragmentation | PARTIAL | Bounded Open-network RX reassembly is live for STA and AP, including timeout, duplicate and eviction ownership. Protected RX fragments are rejected before replay admission, and all TX fragmentation is absent. |
| TX/RX A-MPDU | LIVE | Bounded HT/HE STA and HT AP aggregation, negotiated BA windows, RX reorder and selective TX retry are present. HE trigger-based aggregation is a separate fail-closed frontier. |
| TX/RX A-MSDU | PARTIAL | STA/AP RX decapsulation is present. Bounded 3,839-byte TX is live for the STA protected pair and for AP Open HT/QoS or WPA2 TID-0 pairs when the peer negotiated BA+A-MSDU. Exact peer/order, descriptor+MIC+FCS capacity and fallback are checked before sequence/PN publication. General multi-lease scheduling and the larger A-MSDU class are absent. |
| TXOP | PARTIAL | Negotiated WMM TXOP values reach per-AC policy. For HE A-MPDU they bound the APEP byte ceiling. There is no general multi-PPDU TXOP scheduler, and nonzero HT TXOP duration is rejected. |
| WMM | LIVE | WMM parameter parsing, DSCP/VLAN classification, ACM downgrade, four access-category queues, per-AC EDCA state, TID selection and BA routing are source-owned. U-APSD remains disabled. |
| 20/40 MHz 802.11n, MCS0-MCS7 | LIVE | HT20/HT40 one-stream rate selection, SGI/LGI formatters, retry schedules and the qualified 150-Mbit/s HT40 MCS7 SGI path are present. |
| HT Duplicate MCS32 | FAIL-CLOSED | Peer capability and HT40/MCS32 RX normalization are retained, while local capability advertisement remains clear. Physical TX needs an exact descriptor/PLCP/HT-SIG/length/protection/power/retry oracle and separate on-air qualification. |

## Security

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Open BSS | LIVE | STA and AP plaintext management/data paths are present. |
| WPA2-Personal / CCMP | LIVE | STA supplicant and AP authenticator paths own the four-way handshake, strict RSN selection, pairwise/group keys, CCMP PN exhaustion, RX replay admission and key teardown. STA GTK rekey is atomic; AP GTK rotation remains separate work. |
| WPA2-Enterprise | ABSENT | There is no 802.1X/EAP supplicant/authenticator or enterprise credential owner. |
| WPA3-Personal / WPA3-Enterprise | ABSENT | SAE, transition policy, enterprise authentication and their lifecycle are not present. |
| BIP / protected management frames | ABSENT | PMF negotiation, IGTK/BIGTK ownership, management replay and BIP MIC verification/publication are not present. |
| GCMP, TKIP, WAPI and WEP | ABSENT | The public security domain intentionally exposes only Open and WPA2-Personal/CCMP. No other cipher is advertised or selected by fallback. |

## TSF, beacon monitoring, and power saving

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| Hardware TSF | LIVE | Coherent station TSF reads, AP TSF lifecycle and beacon timestamps are owned. |
| Automatic hardware beacon monitoring | PARTIAL | Software beacon-loss monitoring, active probe recovery and TIM/DTIM parsing are live. A one-shot runtime epoch binds BSSID+AID to exact STA-policy readback and validates the four-bit miss limit. It intentionally stops before MMIO because beacon-interval-to-raw-time conversion, automatic filter lifecycle and the exact STA WDEVPWR cause are not yet proven. |
| Legacy station power save | PARTIAL | PM=1/PM=0 transitions are ACK-gated; TIM/DTIM/listen interval, PS-Poll and doze permits are source-owned. The ESP32-S31 boundary probes and rolls back only reviewed wake-prefix fields. It does not claim RF/PHY/BB/clock sleep. Concurrent STA+AP remains always awake. |
| TWT requester | FAIL-CLOSED | Portable individual-TWT parsing, bounded requester state, deadlines, teardown and TSF wake planning exist. S31 admit/install/remove fails before claiming an ITWT schedule because coexistence mapping, wake compare, retention and restore ordering are missing. The capability bit stays clear. |
| Intra-PPDU power save | PARTIAL | Reviewed HE peer setup enables the MAC intra-PPDU and BSS-color-check bits. No runtime evidence connects that detector to a safe RF/PHY stop/wake transaction, so no energy-saving result is claimed. |

## 802.11ax / HE

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| HE20 non-AP, 1T1R, MCS0-MCS9 | LIVE | Associated STA HE20 SU S-MPDU/A-MPDU, BCC/LDPC, supported GI/LTF combinations and DCM profiles are source-owned. The SoftAP does not advertise or transmit HE. |
| Multiple BSSIDs | ABSENT | The capability bit is clear, association programs BSSID index zero, and there is no nontransmitted-profile scan/association owner. |
| Triggered response scheduling | FAIL-CLOSED | Basic Trigger parsing, association/AID validation, bounded response deadlines and queue/MPLEN/BSR preparation exist. Production configuration disables the path, and the final HE-TB PHY vector/doorbell transition is explicitly unverified. |
| MU-RTS, MU-BAR, Multi-STA BA | ABSENT | Trigger-type parsing alone is present. There is no complete scheduling, response, bitmap, timeout or retry owner for these exchanges. |
| Two NAVs | ABSENT | No independent intra-BSS/basic NAV state and update lifecycle is exposed by production code. |
| BSS coloring | PARTIAL | The associated peer's effective color is parsed, programmed and placed in HE SU TX vectors. Collision detection, color-change announcements and dynamic recoloring are absent. |
| Spatial reuse | ABSENT | RX HE metadata can expose spatial-reuse fields, but TX uses zero and there is no OBSS-PD, SRP admission, CCA-threshold or collision-policy owner. |
| Uplink power headroom | PARTIAL | Wire decoding and the vendor-shaped UL-MU power capability element exist. There is no connected runtime producer/consumer that closes the UPH exchange. |
| Operating Mode Control | PARTIAL | Exact PAC fields and optional software HE-Control queue images exist, but no connected OMC negotiation/publication lifecycle invokes them. |
| Buffer Status Report | PARTIAL | HE BSR initialization, queue byte accounting and hardware-generated control-field geometry are recovered. Ordinary production frames do not request the hardware BSR path, and trigger-based publication remains fail-closed. |
| TXOP-duration RTS threshold | FAIL-CLOSED | A finite peer threshold cannot currently become a complete protected TX image; it is rejected rather than treated as disabled or as a generic RTS threshold. |
| UORA | PARTIAL | The reviewed default UORA contention-window register image is initialized. There is no random-access state machine or successful UL-OFDMA publication. |
| Uplink OFDMA | FAIL-CLOSED | The Trigger/RU schedule can reach the prepared-queue frontier, but the HE-TB PHY publication oracle is missing. |
| Downlink OFDMA / MU-MIMO | PARTIAL | HE-MU common metadata and bounded HE20 SIG-B user decoding exist. There is no dated production payload qualification or complete wider-bandwidth layout, so metadata parsing is not promoted to a connectivity claim. |
| Beamformee | FAIL-CLOSED | NDPA detection, association binding and report-rate hardware setup exist. Feedback formatting/publication is unverified and all dependent capability bits are cleared. |

## Frequency, antenna, FTM, and ESP-NOW

| Data-sheet feature | Status | Current production boundary |
| --- | --- | --- |
| 2412-2484 MHz | PARTIAL | Channels 1-13 are live. Channel 14 / 2484 MHz is representable in portable types but is rejected by the S31 PHY while the required channel-14 MIC/RF/regulatory behavior is unproven. |
| Antenna diversity | FAIL-CLOSED | PAC/HAL own a reviewed enable bit and cold antenna initialization, but there is no public RF/GPIO antenna-selection owner, runtime policy, board contract or HIL claim. |
| 802.11mc FTM | FAIL-CLOSED | A one-bit PHY enable leaf is recovered. FTM action frames, dialog/session state, timestamp capture/calibration, clock ownership and result API are absent. |
| ESP-NOW v1/v2 plaintext | LIVE | Bounded peer/channel ownership, v1/v2 encode/decode, standard DSSS1M, OFDM and HT20 MCS0-MCS7 LGI/SGI TX/RX paths and retry publication are present on the STA VIF. |
| ESP-NOW encrypted peers | FAIL-CLOSED | Portable LMK/peer ownership exists, but S31 key-selector and Action-frame AAD/CCMP contract are unproven, so encrypted publication is rejected. |
| ESP-NOW Long Range PHY | FAIL-CLOSED | Exact low-rate identities and a reversible low-rate gate are retained. The rate-to-PLCP/queue-vector mapping and RX normalization are missing, so LR never masquerades as a standard observed PHY. |

## Missing evidence that blocks physical publication

The largest gaps cannot be closed by widening an enum or copying a register
name. They need synchronous evidence that binds an input request to the
complete hardware transaction:

1. MCS32 and ESP-NOW LR descriptor, PLCP, queue, power and retry images;
2. RTS/CTS duration, basic-rate, power, retry and completion ownership;
3. station WDEVPWR compare/cause binding plus RF/PHY/BB/clock sleep and exact
   wake/rollback ordering;
4. HE-TB RU/GI/LTF/MCS/DATA_LENGTH vector and final doorbell publication;
5. beamforming feedback memory/formatter/publication and FTM timestamp paths;
6. channel-14 RF/MIC/regulatory setup.

Until those inputs exist, the corresponding production boundaries must stay
fail-closed and their over-the-air capability bits must remain clear.

## Remaining implementation program

The remaining work is not one homogeneous list. Some gaps can be advanced
from the reviewed source tree alone; others would require inventing a physical
contract if implemented without new evidence.

| Workstream | Source-owned work that can proceed | Evidence or scope boundary that remains |
| --- | --- | --- |
| Interface ownership | Add an explicit multi-VIF allocator, concurrent monitor ownership and an AP channel-migration transaction around STA scanning. | Only one STA and one AP protocol owner currently exist; four hardware address slots are not four complete VIFs. |
| Legacy/HT MAC | Add bounded TX fragmentation, general multi-lease A-MSDU scheduling, U-APSD and multi-PPDU TXOP ownership. | Physical RTS/CTS-to-Self and MCS32 still need complete queue/PLCP/duration/power/retry evidence. |
| WPA2/AP security | Retain an authorized peer's KCK/KEK under zeroization, implement per-peer Group-Key Handshake retries, and replace the AP GTK only after a bounded acknowledgement barrier. | WPA3, Enterprise, PMF/BIP and the other advertised cipher families are independent protocol/credential projects, not CCMP mode switches. |
| Beacon and low power | Bind the software beacon epoch to automatic filter enable/disable, raw timeout units and the reviewed wake cause; then make doze cancellation-safe across every owner. | Actual modem sleep and TWT installation require RF/PHY/BB/clock retention, wake compare and rollback evidence. |
| HE control plane | Add selected MBSSID profile ownership, dual-NAV state, spatial-reuse policy, OMC/UPH/BSR exchanges and bounded MU control-session state. | HE-TB/OFDMA, UORA publication and beamforming feedback cannot go live before their final PHY vectors and doorbells are source-proven. |
| Ranging and special PHYs | Complete the allocation-free FTM requester protocol and a board-level antenna-routing contract. | Distance needs calibrated hardware timestamps; ESP-NOW LR/encryption, antenna switching and channel 14 each need their missing physical or board contract. |

The implementation rule is therefore: complete protocol logic and preserve
ownership wherever the source is sufficient, but stop before sequence, PN,
replay, DMA or capability publication whenever the physical transaction is
not proven.
