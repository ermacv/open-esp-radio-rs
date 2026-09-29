# ESP32-C5 Wi-Fi vs ESP32-S31 (research, 2026-09-27; not tracked)

Inputs: esp32-wifi-lib af55a0ca (libpp, libnet80211 for esp32s31/esp32c5), S31 register model.

## Blobs
- libpp / libnet80211: mostly identical after normalization (xchip/libpp, xchip/libnet80211).
- C5 has extra VHT/11ac functions (C5 is 2.4+5 GHz 11ax/ac); S31 has none.
- Differences: TX PPDU/PLCP setup, TXQ enable/disable, multi-BSSID, trc rate control, pm_beacon_offset, rx buffer init.

## MAC register window (S31 0x2010_xxxx -> C5 0x600A_xxxx)
Superseded by the "Register map, item 1" section below (the first version read truncated function bodies).
Of the 313 registers in S31 wifi-mac-*.toml: 214 are observed directly, 36 bracketed by same-delta segments, 63 unknown.
- Unknown = per-queue arrays (TX_QUEUE_INFORMATION/BA_*_Qn, tx-queue-vector %s, tx-completion %s), RX BA entry %s, and 0x4C68/0x4C6C.
- Per-queue TX block: S31 stride 0x7C, C5 0x78. The dropped word is at vector-relative +0x68..+0x70. The delta grows by 4 per queue (-0x1c..-0x2c).
  Every per-queue register therefore needs its own C5 formula, not one delta.
- Segments: 0 (0x0010..0x2004, 0x2060..0x4464, 0x4800..0x4c98, 0x5800..0xd8ac, 0xf008..0xf4d8);
  -4 (0x2014, 0x4ca0..0x4eb8, 0xd8b4..0xd8d0); -8 (0x44e0..0x452c, 0x5130).
- hal_mac_txq_enable: same logic, register -4, wDevCtrl field offsets +4.

## DMA
- Descriptor format is identical: 12 bytes, 14-bit size/length. Both wDev_ProcessRxSucData versions use slli/srli 0x12.
- S31 mac_rxbuf_init also writes 0x4C68/0x4C6C/0x4C70: a DMA window with 0x2F00_0000 high bits. C5 lacks these writes.
  C5 DMA addresses internal SRAM directly, so the S31 DMA_LOW/DMA_HIGH (0x2f00_0000..0x2f08_0000) and the model addresses become chip parameters.
- S31 ProcessRxSucData also reads 0x2010_D800 into rx-control bytes 0xC..0xF (when a flag nibble is 0). C5 lacks this read; the remaining RX path is the same.

## Rust coupling (S31 production)
| crate | LOC | hal refs | phy refs | 0x2f0 | s31 mentions |
|---|---|---|---|---|---|
| driver/ieee80211 | 7.6k | 20 | 6 | 3 | 167 |
| driver/ieee80211/mac | 23.5k | 48 | 0 | 4 | 251 |
| driver/ieee80211/dma | 8.7k | 3 | 0 | 50 | 19 |
| roles sta | 18.7k | 16 | 1 | 2 | 156 |
| roles ap | 9.5k | 9 | 0 | 1 | 66 |
| runtime ieee80211 | 75.6k | 63 | 14 | 28 | 818 |
- No crate touches the PAC directly; all MMIO goes through HAL ieee80211 (2.3k LOC) over WifiRadioRegisters (PAC wifi, 8.1k LOC).
- A C5 port = C5 PAC wifi (generated from derived C5 fragments) + C5 HAL ieee80211, then generalize the driver/role/runtime over a HAL trait (variant 2).
- The PHY coupling (baseband, channel) waits for the PHY split P2-10.

## Register map, item 1 (full function bodies; 2026-09-27)
The earlier offset map read only the first basic block of each function (bodycmp split on .L labels). This section replaces it.
Sources: libpp, libnet80211, libcoexist, libphy, ROM (S31 rev0 vs C5 rev100); per-function verdicts in diff/function-verdicts.json.
Of the 395 functions that touch the MAC: 337 are the same modulo the address map, 15 differ only in software struct layout, 41 are structural, 2 value-differs.
The only real field difference is in hal_he_get_nontrans_bssid_wakeup_only_en (bit 19 -> 18).

Of the 892 register instances in S31 wifi-mac-*.toml: address resolution {'fixed-equal-length': 277, 'queue-bank': 124, 'fixed-lib': 12, 'indexed': 342, None: 18, 'fixed-lib-loop': 118, 'fixed-equal-length-overrides-aligned': 1}.
Field status {'confirmed': 454, 'partly': 237, 'review': 48, 'no-address': 4, 'unobserved': 149}: confirmed = all touching functions are the same modulo the map; partly = some are structural.

Queue banks: 8 banks; S31 0x54d8-0x7c*i maps to C5 0x54b0-0x78*i.
Bank offsets +0x00..+0x68 are unchanged, +0x74.. shift -4. The dropped S31 word is +0x6c or +0x70 and is unmodeled in S31.
Proof: hal_attenna_init walks 8 banks on both chips (0x5510..0x5130 step 0x7c vs 0x54e8..0x5128 step 0x78); mac_tx_set_plcp1 (-0x7c/-0x78); hal_mac_get_txq_complete.

MPDU-length link table 0x55f0..0x57cc is unchanged (hal_he_init loop bounds and dbg_read_tx_mplen identical).
Key table 0x5800+0x28*e is unchanged (all 10 words observed).
BF_VECTOR_CONTROL/BF_ENABLE 0x447c/0x4480 are unchanged; C5 hal_init_bf writes one extra field.
RX_BUFFER_LIMIT/BASE 0x4c68/0x4c6c (DMA window) are absent on C5.

Still unresolved (not touched by any code on either chip): RX BA entry bitmap status (13), rx-filter POLICY1/2 and BASEBAND_ERROR_STATUS2, VHT_SIGNAL_1/VHT_MODE banks (bank rule only), TX BA TA_LOW Q1..Q7 (bank rule only).
C5 ROM MAC functions that libpp re-implements disagree in places (hal_mac_get_txq_state 0x4cbc: libpp 0x4cb8, ROM 0x4cb0). Library evidence takes precedence.
PHY caution: C5 ROM phy_* (rev100) use 0x88c/0x430 where S31 uses 0x890/0x438, but C5 libphy phy_force_txrx_off_new uses 0x890.
Map with per-register evidence: diff/wifi-mac-c5-map.json.
