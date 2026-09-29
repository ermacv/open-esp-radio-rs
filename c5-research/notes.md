## Modem offset conflict at S31 0x890 (resolved)
0x890 is the same register on S31 and C5: bt_rx_force, phy_set_txclk_en,
phy_fe_adc_on and txon_set use +0x890 on both chips. C5 phy_force_txrx_off
writes +0x88c (the phy_pbus_force_mode register) where S31 writes +0x890:
a behavioral change of the vendor function, not an address shift. The
offset map keeps 0x890 -> 0x890.
## ESP32-C5 IEEE 802.15.4 chip data (for the C5 HAL)
- TX power levels: C5 libbtbb (sha256 9cbaf5bc...) bt_bb_v2.o bt_bb_get_tx_pwr_table, size 0x90: count 16;
  if phy_param+0x434 (bttx_low_power, set only by phy_set_bttx_low_power, .data default 0, never called by IDF 4d59230d)
  != 0: rodata [-24,-24,-24,-24,-21,-17,-15,-11,-8,-5,-1,2,5,9,13,16]; else phy_get_data_sat(3i-24, max 20, min -15)
  = [-15,-15,-15,-15,-12,-9,-6,-3,0,3,6,9,12,15,18,20] (default).
- coex_pti_tab: C5 libcoexist (sha256 de883d64...) coexist_core.o .dram1.2, 49 bytes, identical to S31;
  coex_ieee802154_pti_get / esp_coex_ieee802154_{txrx,ack}_pti_set bodies identical to S31.
- DMA window: IDF 4d59230d soc/esp32c5/include/soc/soc.h SOC_DMA_LOW 0x40800000, SOC_DMA_HIGH 0x40860000.
- TX-on delays: 0x104=40, 0x114=122, 0x110=50, 0x10c=0 (order 104,114,110,10c).
