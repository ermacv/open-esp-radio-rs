# ESP32-S31 Wi-Fi feature status

This file tracks the open driver's implementation and hardware qualification
against the chip capability contract. A feature appearing in the datasheet is
not considered implemented merely because a register, enum, or vendor symbol
for it has been identified.

Primary capability source: Espressif, *ESP32-S31 Series Datasheet*, preliminary
v0.5, section 4.3.2.1 “Wi-Fi Radio and Baseband”:
<https://documentation.espressif.com/esp32-s31_datasheet_en.html>.

Status meanings:

- **HIL TX** or **HIL RX**: exercised end to end by the open PHY/MAC on hardware.
- **implemented**: an owned Rust path exists and host checks pass, but the
  complete feature still lacks matching HIL.
- **oracle only**: blob/ROM behavior has been located but no complete open path
  exists.
- **not implemented**: no complete open path exists.

## Qualification ledger

This is the canonical index of completed and blocked HIL cells. Do not repeat
an identical cell merely to rediscover its status. Repeat it only when at
least one of these inputs changes:

- driver code on the named path;
- PHY/MAC register definition used by that path;
- `esp-hal`/PAC/bootstrap revision or memory placement;
- peer hardware, firmware or advertised capability;
- traffic shape or a stricter acceptance gate.

Every newly qualified behavior must update this table after its reusable
logic has moved from the HIL application into a driver crate. The application
may retain board setup, credentials, traffic generation and reporting only.
Detailed raw captures may stay in the application artifact directory, but the
committed record must contain the exact vector, result, artifact hashes and
the driver API that now owns the behavior.

| Qualification ID | Exact cell and authoritative result | Driver owner / retained evidence | Repeat only when |
|---|---|---|---|
| `HIL_OPEN_HE20_DRIVER_REPO_BIDIRECTIONAL_2026_07_31` | FRITZ HE20 MCS9, LDPC, 2xLTF/0.8-us GI, 32-member A-MPDU/BlockAck, `psram-code-psram-data`, with both firmware and host runner built exclusively from this repository and the published `esp-hal` branch: 10.012-Mbit/s RX median plus 67.636-Mbit/s concurrent TX floor; zero observed `BUFFER_FULL`/`FIFO_OVERFLOW`. | `HeRate`, `StaTxRatePolicy`, `HtAmpduTxStorage`, `AmpduRetryState`, `ReferencedAmpduIngressPolicy`, `open-esp-radio-embassy-net`; exact commands and SHA-256 evidence in [`hil/2026-07-31-driver-repo-he20-bidirectional.md`](hil/2026-07-31-driver-repo-he20-bidirectional.md). | Driver data path, interrupt/retry ordering, HIL bootstrap/memory placement, FRITZ peer, or traffic/acceptance gate changes. |
| `HIL_OPEN_HT40_SGI_BIDIRECTIONAL_2026_07_30` | HT40 MCS7, 400-ns GI, A-MPDU/BlockAck, `psram-code-psram-data`: 25.006-Mbit/s RX median plus 68.276-Mbit/s concurrent TX floor; zero final `BUFFER_FULL`/`FIFO_OVERFLOW`. | `HtRate`, `HtAmpduTxStorage`, `AmpduRetryState`, `ReferencedHtAmpduBatch`; detailed result in [`ESP32S31_RUST_INTEGRATION_AUDIT.md`](ESP32S31_RUST_INTEGRATION_AUDIT.md). | HT formatter, A-MPDU ownership, retry/IRQ ordering, peer or memory profile changes. |
| `HIL_OPEN_HE20_MCS_GI_MATRIX_2026_07_31` | HE SU MCS0..9 at selectors 1/2/3: four complete 30-cell rounds, 64 real A-MPDUs per cell, no failed profile, terminal retry or RX DMA starvation. | `HeRate`, `HeAmpduTxConfig`, `StaTxRatePolicy`; detailed result in [`ESP32S31_RUST_INTEGRATION_AUDIT.md`](ESP32S31_RUST_INTEGRATION_AUDIT.md). | HE formatter/rate policy/APEP/BlockAck path or peer changes. |
| `HIL_OPEN_HE20_GI0_PEER_REJECT_2026_07_31` | Negative cell: the controlled AX211 AP advertises `one_ltf_800ns_gi=false`; selector 0 completes association/AddBA but every data A-MPDU receives no valid BlockAck. This is a capability rejection, not an open-driver positive HIL. | `StaTxRatePolicy` retains selector 0 as a typed value but must capability-gate it. | Use a peer that advertises selector 0, or change capability parsing. |
| `HIL_OPEN_HE20_DCM_MATRIX_2026_07_31` | Raw BCC DCM MCS0 at selectors 1/2/3: 44 complete three-cell rounds, 64 A-MPDUs per cell, no failed profile, terminal retry, `BUFFER_FULL` or `FIFO_OVERFLOW`. | `HeRate::{bcc_dcm,ampdu_empty_delimiters,maximum_apep_bytes}`, HE TX formatter; detailed result in [`ESP32S31_RUST_INTEGRATION_AUDIT.md`](ESP32S31_RUST_INTEGRATION_AUDIT.md). | DCM formatter/APEP/delimiter logic or peer changes. |
| `HIL_OPEN_HE20_DCM_CONNECTED_2026_07_31` | Connected BCC DCM MCS0, selector 1, 20 MHz, A-MPDU/BlockAck and simultaneous traffic: RX 1.002 Mbit/s, TX floor 0.749 Mbit/s, `spill=0`, zero DMA starvation. Linux independently reported every sampled uplink vector as `4.3 MBit/s HE-MCS 0 HE-DCM 1`. | `HeDcmRate`, `StaTxRatePolicy`, `ReferencedAmpduIngressPolicy`; immutable record in [`hil/2026-07-31-he20-dcm-connected.md`](hil/2026-07-31-he20-dcm-connected.md). | DCM rate policy, referenced-batch ingress, peer DCM capability, pacing shape or memory profile changes. |
| `HIL_OPEN_HE20_DCM_LDPC_CONNECTED_2026_07_31` | Connected LDPC DCM MCS0, selector 1, 20 MHz, A-MPDU/BlockAck and simultaneous traffic: RX 1.001 Mbit/s, TX floor 0.749 Mbit/s, `spill=0`, zero DMA starvation. Linux independently reported all 15 sampled uplink vectors as `4.3 MBit/s HE-MCS 0 HE-DCM 1`; the strict device gate required `he_ldpc=1`. | `HeDcmRate::ldpc`, `StaTxRatePolicy` LDPC/DCM capability gate and `ReferencedAmpduIngressPolicy`; immutable record in [`hil/2026-07-31-he20-dcm-ldpc-connected.md`](hil/2026-07-31-he20-dcm-ldpc-connected.md). | DCM/LDPC coding, peer LDPC capability, referenced-batch ingress, pacing shape or memory profile changes. |
| `HIL_OPEN_HE_TRIGGER_ABSENT_AX211_2026_07_31` | Sustained HE SU on the current Linux AX211 AP produced zero Trigger frames and zero HE-TB transmissions. Repeating ordinary traffic against the same AP cannot qualify OFDMA. | Trigger parser/scheduled-rate/queue types are implemented; missing boundary is an external valid Trigger producer. | AP/firmware gains Trigger scheduling or a different controllable Trigger source is connected. |
| `HIL_OPEN_HE_SOUNDING_ABSENT_2026_07_31` | Current FRITZ and Linux AP scenarios produced no open-path NDPA/sounding exchange despite successful HE association, AddBA and traffic. | NDPA parser/encoder and report-rate programming are implemented; sounding activation remains open. | AP/sounding policy or running feedback scheduler changes. |

## 802.11ax

| Datasheet capability | Open-driver status | Current evidence and missing boundary |
|---|---|---|
| 1T1R, 2.4 GHz | implemented, NSS1 only | All typed HT/HE rates are single-stream. A dedicated antenna/NSS boundary HIL is still required before claiming the complete RF feature. |
| 20 MHz-only non-AP mode | **HIL TX/RX association** | Open STA scan, authentication, association, WPA2, DHCP and protected traffic complete as HE20. No HE SoftAP claim is made. |
| MCS0 through MCS9 | **HIL TX and live rate selection** | Bounded HE SU A-MPDU matrix completed MCS0..9 with BlockAck. The negotiated HE RX MCS/NSS map now supplies the typed association maximum instead of `None`; an MCS0..9 peer selects Rust-owned Dot11Ax/0 and drives the live formatter as HE MCS9/LDPC, 2xLTF/0.8-us GI at 114.7 Mbit/s. Android-AP HIL retained that rate for 6,069 A-MSDU/A-MPDU submissions with zero partial aggregates and five-second payload samples up to 80.7 Mbit/s. After schedule-to-format ownership moved into `StaTxRatePolicy`, the current PSRAM/PSRAM image completed four consecutive 30-profile BCC matrices against a FRITZ peer: every MCS0..9/GI profile submitted 64 A-MPDUs with zero failed profiles, terminal retries or RX DMA starvation. Downlink rate-forcing, per-MCS RX qualification and running A-MPDU rate lowering remain. |
| Uplink and downlink OFDMA | Trigger iteration, scheduled-rate admission, queue publication, source-accurate TB completion and passive HE-MU/TB vector decoding implemented; scheduler oracle only | Allocation-free Rust decodes Trigger Common Info, both RU/SS User Info forms, Basic/BFRP/MU-BAR/NFRP dependent fields and TRS/UPH HE-control information. Complete blob bodies anchor fail-closed User Info iteration, exact type-dependent strides, the AID12/RU padding sentinel and raw-allocation classification for RU26/52/106/242; wider and gap encodings remain explicitly unselectable. A typed scheduled-user plan additionally requires Basic/HE20, matching AID, NSS1, MCS0..9 and a valid BCC/LDPC DCM combination, then selects the exact ordinary/DCM RU rate table. The MAC can publish a bounded Trigger-eligible A-MPDU queue, MPDU-length chain and BSR state. `TxCompletion::completes_vendor_trigger_flow` and `AmpduRetryDecision::FinishTriggerFlow` reproduce the complete `libpp.a[lmac.o]` status-five/zero-TB-count path without fabricating a BlockAck or adding ordinary MPDU attempts; eight host tests now cover the retry owner, including the exact positive predicate and all counter/status rejection boundaries. Complete `dbg_dump_rx_ppdu` independently anchors typed HE-MU SIG-B MCS/DCM/GI/STBC metadata and HE-TB common SIG-A metadata. Sustained HE-SU traffic against FRITZ and Android Wi-Fi 6 APs produced no Trigger: FRITZ advertised triggered SU feedback/CQI but did not schedule it, while the Android AP advertised neither and reports no HE MU beamformer. Region/index HIL, real HE-TB TX qualification and OFDMA payload RX remain incomplete. |
| Downlink MU-MIMO | passive complete-vector decoders implemented; payload HIL missing | The RX owner exposes the complete blob-decoded HE-MU common SIG-A view, including compressed-SIG-B user count, GI/LTF and STBC. It bounds and borrows the variable complete-SIG-B bytes and decodes the hardware-selected 21-bit MIMO/non-MIMO user view. Allocation-free iterators cover all users in both exact HE20 blob layouts: non-MIMO includes the ten-bit pair gaps and up to nine users, while compressed/MU-MIMO retains its separate non-linear `0,21,52,105` offsets and one-to-four user bound. The complete non-MIMO common parser and `get_user_num` now anchor a typed HE20 RU Allocation decoder, including both exact revision-v0 ROM tables, all computed RU26/52/106/242 classes, numeric multiplexing output and a user-count consistency check. The seven exact revision-v0 ROM spatial-configuration tables are owned as typed two-to-eight-user NSTS mappings; compressed users must agree on one valid encoding. Wider and reserved layouts fail closed. Payload qualification and beamforming feedback remain open. |
| 0.8, 1.6 and 3.2 us GI | **HIL TX** | HE SU MCS0..9 passed with 2xLTF/0.8 us, 2xLTF/1.6 us and 4xLTF/3.2 us; the 2026-07-31 post-transfer requalification repeated all 30 combinations for four complete rounds with zero failed profiles or terminal retries. The peer did not advertise the optional 1xLTF/0.8-us capability, and the vendor `ppSelectTxFormat`/`ppCertSetRate` producers do not emit selector zero. |
| DCM up to 16-QAM | **HIL TX BPSK**; QPSK/16-QAM and LDPC MCS4 implemented; RX vector decoding implemented | Peer TX/RX constellation and payload-LDPC capabilities are parsed independently. The typed HE-SU BCC path admits DCM MCS0/1/3, while the separate LDPC type admits the ROM-evidenced MCS4 column without permitting BCC+MCS4. Both paths program HE-SIG-A1 DCM; complete blob/ROM `mac_tx_set_hesig` proves queue HE-A2 control `0x105` for BCC and `0x107` for LDPC. Ordinary LDPC TX is HIL-qualified across MCS0..9 and all three vendor-produced GI/LTF selectors: three complete 30-profile A-MPDU matrices had no failed profile or terminal retry. The live DCM matrix includes MCS4 only for a peer advertising both 16-QAM DCM RX and LDPC payload. On a controlled AX211 AP advertising BPSK DCM RX, open MCS0 DCM passed both S-MPDU/ordinary ACK and A-MPDU/BlockAck at all three GI/LTF selectors. The current PSRAM/PSRAM tree subsequently completed 44 consecutive three-profile rounds, 64 real A-MPDUs per profile, with no failed profile, terminal retry, RX `BUFFER_FULL` or `FIFO_OVERFLOW`; profile-local blob-counter bracketing also kept `rx_buffer_full=0`. The RX owner decodes HE-SU DCM and the HE-MU SIG-B DCM common bit from the public prefix, independently anchored by complete `dbg_dump_rx_ppdu`; controlled DCM payload RX remains. MCS1/QPSK, MCS3/16-QAM and LDPC MCS4 DCM still need capable peers. |
| Single-user/multi-user beamformee | hardware feedback sequence configured; vendor exchange and open encoder recovered; open sounding HIL missing | The complete blob link-metric policy now produces a typed report profile and programs both hardware leaves in exact order: three BPSK/QPSK/16-QAM selectors followed by all four ordinary/ER-SU ACK-rate bytes. Allocation-free HE NDPA parsing reproduces the complete blob's Dialog Token, four-byte STA Info iteration and local 11-bit AID membership check. A preserved complete-vendor monitor capture (`esp32s31-he-oracle-fixed-ch11.pcapng`, SHA-256 `d50289842bd3cddbcebf3080c049cf6d6b387908b501b6b7333fbfb250e7abde`) closes the HE20 STA Info layout: frame 1374 carries word `0x0820001d` for AID 29 and RU indices 0..8; after the NDP, frame 1376 is the S31 HE Compressed Beamforming and CQI response 14.39 us later. A typed fail-closed encoder now reproduces that exact 21-byte NDPA and owns every field. The open STA also reproduces frame 7624's complete vendor HE association body: listen interval, rate IEs, RSN `0x0400`, Power Capability from the Rust-owned calibrated profile, HT/HE/UL-MU power, WMM and Extended Capabilities. FRITZ HIL accepted that request, completed WPA2/AddBA/A-MPDU and carried a second zero-loss 35-second uplink, but still emitted no NDPA or Trigger. The open path now also reproduces the complete vendor TID0/TID7/TID5 AddBA order, shared Dialog Tokens 1/2/3 and independent per-TID SSNs; a corrected air capture matched the TID0 AddBA SSN to the first A-MPDU BlockAck start and still observed no NDPA or Trigger. Post-association `ic_set_sta -> ic_set_trc -> rcUpdatePhyMode -> trc_set_bf_report_rate` is now Rust-owned too: live ROM-derived noise floor and scan RSSI select the exact Dot11Ax schedule/report profile, while ER-SU-Disable, DCM receive constellation and vendor LR-rate inputs are mapped to their real protocol meanings. Repeated FRITZ HIL still observed zero NDPA/Trigger, localizing the missing activation beyond association, BlockAck and initial rate-control into the running ACK-SNR/PER transition or the sounding-policy scheduler. Complete `hal_init_bf` plus typed non-latched beam/NDP/feedback diagnostics establish that report sequencing is hardware-owned; the test-only feedback selector remains mapped in SVD, but its numeric mode meanings are unproven. The same sounding exchange with the fully open driver therefore remains unqualified. |
| CQI | capability parsed; report path not implemented or advertised | The vendor oracle advertises both triggered and non-triggered CQI, and their independent standard bits are parsed and source-audited against complete `ieee80211_add_hecap`. The open association profile now clears exactly those two claims while preserving the separately owned beamforming-feedback bits. An owned Trigger/CQI report producer and HIL are still required before either CQI bit can be enabled. |
| RX STBC, one spatial stream | implemented through RX metadata; HIL pending capable peer | The local STA capability advertises RX STBC below 80 MHz exactly as complete `ieee80211_add_hecap` derives it from `g_phy_cap_rx_stbc`. Peer TX/RX STBC bits are parsed independently, and the owned S31 RX metadata decoder exposes HE-SIG-A2 `stbc`. The nearby FRITZ capability has HE TX STBC clear; controlled downlink STBC and payload-integrity HIL remain. |

## 802.11b/g/n

| Datasheet capability | Open-driver status | Current evidence and missing boundary |
|---|---|---|
| MCS0 through MCS7, 20/40 MHz | implemented; endpoint **HIL TX** | The typed formatter covers the full single-stream set. HIL qualifies HT20 MCS0/LGI and HT40 MCS7/SGI, including A-MPDU/BlockAck; a complete per-MCS/per-width HIL matrix remains. |
| MCS32 | not implemented | The current `HtMcs` type intentionally admits only single-stream MCS0..7. Duplicate-mode MCS32 needs a distinct typed representation and oracle comparison. |
| Data rate up to 150 Mbit/s | **HIL TX PHY** | HT40 MCS7 with 400-ns GI programs the 150-Mbit/s PHY rate. Open A-MSDU/A-MPDU HIL exceeded 100 Mbit/s application uplink; PHY rate is not the same as Ethernet goodput. |
| 0.4-us guard interval | **HIL TX** | Typed `HtGuardInterval::Short400Ns`; qualified at HT40 MCS7. |
| Adjustable transmitting power | implemented, partial HIL | Rate-to-power selection and the runtime quarter-dBm limit are owned Rust logic. Controlled power A/B affects association as expected, but a calibrated multi-level conducted-power measurement is still missing. |

## Current next order

1. Keep HE SU MCS9/three-GI regression green with zero final retry failures.
2. Qualify DCM MCS1/QPSK and MCS3/16-QAM against a peer that advertises them;
   the WMM-fed nonzero-TXOP APEP producer and recovered empty-delimiter policy
   are now owned.
3. Qualify the owned LDPC+DCM MCS4 profile against a peer advertising both
   capabilities, then add DCM RX.
4. Add RX STBC and CQI capability/vector ownership.
5. Build the trigger/RU boundary required for HE-TB uplink OFDMA.
6. Add downlink OFDMA, SU/MU beamformee feedback, then DL MU-MIMO.
7. Add HT duplicate-mode MCS32 and complete the HT rate/width/GI HIL matrix.
