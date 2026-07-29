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

## 802.11ax

| Datasheet capability | Open-driver status | Current evidence and missing boundary |
|---|---|---|
| 1T1R, 2.4 GHz | implemented, NSS1 only | All typed HT/HE rates are single-stream. A dedicated antenna/NSS boundary HIL is still required before claiming the complete RF feature. |
| 20 MHz-only non-AP mode | **HIL TX/RX association** | Open STA scan, authentication, association, WPA2, DHCP and protected traffic complete as HE20. No HE SoftAP claim is made. |
| MCS0 through MCS9 | **HIL TX** | Bounded HE SU A-MPDU matrix completed MCS0..9 with BlockAck. Downlink rate-forcing and per-MCS RX qualification remain. |
| Uplink and downlink OFDMA | oracle only | Trigger/MU scheduling symbols and MMIO leaves exist in the pinned oracle. No owned trigger parser, RU allocation, HE-TB TX, or OFDMA RX path is complete. |
| Downlink MU-MIMO | oracle only | MU/beamforming leaves exist in the pinned oracle. No open MU receive qualification exists. |
| 0.8, 1.6 and 3.2 us GI | **HIL TX** | HE SU MCS0..9 passed with 2xLTF/0.8 us, 2xLTF/1.6 us and 4xLTF/3.2 us. The peer did not advertise the optional 1xLTF/0.8-us capability, and the vendor `ppSelectTxFormat`/`ppCertSetRate` producers do not emit selector zero. |
| DCM up to 16-QAM | **HIL TX BPSK**; QPSK/16-QAM implemented | Peer TX/RX constellation capabilities are parsed independently. The typed HE-SU BCC path admits only DCM MCS0/1/3, programs HE-SIG-A1 DCM bit 7, uses the recovered RU242 rate tables for all three qualified GI/LTF selectors, reproduces the blob's minimum-subframe-length calculation, and owns the complete zero-TXOP ROM APEP table plus the blob's DCM-halving branch. On a controlled AX211 AP that advertises BPSK DCM RX, open MCS0 passed both S-MPDU/ordinary ACK and one-subframe A-MPDU/BlockAck at all three GI/LTF selectors. The HIL originally forced at least two full-size MPDUs; that exceeded the rate-dependent PPDU-duration budget and was not a PHY failure. MCS1/QPSK and MCS3/16-QAM still need a capable peer. MCS4 belongs to the still-unowned LDPC profile; DCM RX and injection of extra empty delimiters for short MPDUs remain. |
| Single-user/multi-user beamformee | oracle only | Capability and report-rate leaves are known. Sounding, feedback ownership, compressed report generation and HIL are missing. |
| CQI | capability owned; report path not implemented | The preserved vendor-compatible STA HE Capability IE advertises both triggered and non-triggered CQI. Their independent standard bits are now parsed and source-audited against complete `ieee80211_add_hecap`. No owned Trigger/CQI report producer or HIL exists yet, so this advertisement must be narrowed unless the report path is implemented before interoperability testing. |
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
2. Port the nonzero-EDCA-TXOP HE APEP table producer, then qualify DCM
   MCS1/QPSK and MCS3/16-QAM against a peer that advertises them. Wire the
   recovered minimum length into empty-delimiter injection before small-frame
   HIL.
3. Add the separate LDPC profile required for DCM MCS4, then add DCM RX.
4. Add RX STBC and CQI capability/vector ownership.
5. Build the trigger/RU boundary required for HE-TB uplink OFDMA.
6. Add downlink OFDMA, SU/MU beamformee feedback, then DL MU-MIMO.
7. Add HT duplicate-mode MCS32 and complete the HT rate/width/GI HIL matrix.
