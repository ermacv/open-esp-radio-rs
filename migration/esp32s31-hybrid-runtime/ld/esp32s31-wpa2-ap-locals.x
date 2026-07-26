/*
 * Stable aliases for the pinned local ESP32-S31 AP association callback.
 * The archive is unchanged; the strict audit pins its section size/digest.
 */
EXTERN(esp_supplicant_init)

SECTIONS
{
  .text.esp_wifi_async_wpa2_ap_locals : ALIGN(2)
  {
    __esp_hostap_sta_join = .;
    KEEP(*(.text.hostap_sta_join))
    __esp_hostap_sta_join_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;
