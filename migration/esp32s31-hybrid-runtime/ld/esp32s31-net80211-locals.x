/* Pinned libnet80211.a/ieee80211_hostap.o state used by the strict AP beacon
 * completion replacement. These are normal final-link aliases; the vendor
 * archive is not modified. Archive digest/section sizes are audited by xtask.
 *
 * The platform's main .bss output section consumes every .bss.* input. Keep
 * the reviewed inputs in a small preceding output section so their addresses
 * remain nameable in both AP and STA-only final links.
 */
EXTERN(ap_rxcb)

SECTIONS
{
  .text.esp_wifi_async_net80211 : ALIGN(2)
  {
    __esp_s31_addba_response_txcb = .;
    KEEP(*(.text.addba_response_txcb))
    __esp_s31_addba_response_txcb_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;

SECTIONS
{
  .bss.esp_wifi_async_net80211 (NOLOAD) : ALIGN(4)
  {
    __esp_s31_beacon_send_start_flag = .;
    KEEP(*(.bss.beacon_send_start_flag))
    __esp_s31_beacon_send_start_flag_end = .;

    . = ALIGN(4);
    __esp_s31_beacon_timer = .;
    KEEP(*(.bss.beacon_timer))
    __esp_s31_beacon_timer_end = .;

    . = ALIGN(4);
    __esp_s31_beacon_next_tbtt = .;
    KEEP(*(.bss.BcnIntvl))
    __esp_s31_beacon_next_tbtt_end = .;

    . = ALIGN(4);
    __esp_s31_ap_rxcb = .;
    KEEP(*(.bss.ap_rxcb))
    __esp_s31_ap_rxcb_end = .;

    __esp_s31_beacon_dtim_send_mc = .;
    KEEP(*(.bss.g_beacon_dtim_send_mc))
    __esp_s31_beacon_dtim_send_mc_end = .;
  } > RWDATA
}
INSERT BEFORE .bss;

ASSERT(__esp_s31_beacon_send_start_flag_end -
       __esp_s31_beacon_send_start_flag == 0x1,
       "ESP32-S31 beacon_send_start_flag ABI changed");
ASSERT(__esp_s31_beacon_timer_end - __esp_s31_beacon_timer == 0x14,
       "ESP32-S31 beacon_timer ABI changed");
ASSERT(__esp_s31_beacon_next_tbtt_end - __esp_s31_beacon_next_tbtt == 0x4,
       "ESP32-S31 beacon next-TBTT ABI changed");
ASSERT(__esp_s31_ap_rxcb_end - __esp_s31_ap_rxcb == 0x4,
       "ESP32-S31 AP RX callback ABI changed");
ASSERT(__esp_s31_beacon_dtim_send_mc_end -
       __esp_s31_beacon_dtim_send_mc == 0x1,
       "ESP32-S31 beacon DTIM flag ABI changed");
/*
 * The pinned input section is 0x11c bytes. A final executable link relaxes
 * it to 0x10c; current LLD retains 0x11c for an intermediate `-r` audit link.
 */
ASSERT(__esp_s31_addba_response_txcb_end -
       __esp_s31_addba_response_txcb == 0x10c ||
       __esp_s31_addba_response_txcb_end -
       __esp_s31_addba_response_txcb == 0x11c,
       "ESP32-S31 ADDBA response callback ABI changed");
