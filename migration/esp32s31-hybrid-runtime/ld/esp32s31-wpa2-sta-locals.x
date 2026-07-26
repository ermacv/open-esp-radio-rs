/* Stable aliases for the pinned local ESP32-S31 STA link callbacks. */
EXTERN(esp_supplicant_init)

SECTIONS
{
  .text.esp_wifi_async_wpa2_sta_locals : ALIGN(2)
  {
    __esp_wpa_sta_connected_cb = .;
    KEEP(*(.text.wpa_sta_connected_cb))
    __esp_wpa_sta_connected_cb_end = .;
    __esp_wpa_sta_disconnected_cb = .;
    KEEP(*(.text.wpa_sta_disconnected_cb))
    __esp_wpa_sta_disconnected_cb_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;
