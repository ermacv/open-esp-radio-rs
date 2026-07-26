/*
 * Link-time aliases for the local Enterprise EAP state used by the audited
 * ESP32-S31 esp_eap_client.c.obj. This does not modify the vendor archive.
 */
EXTERN(wpa2_task)

SECTIONS
{
  .text.esp_wifi_async_eap_locals : ALIGN(2)
  {
    __esp_eap_start_eapol = .;
    KEEP(*(.text.eap_start_eapol))
    __esp_eap_start_eapol_end = .;

    __esp_wpa2_set_eap_state = .;
    KEEP(*(.text.wpa2_set_eap_state))
    __esp_wpa2_set_eap_state_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;

SECTIONS
{
  .bss.esp_wifi_async_eap_locals (NOLOAD) : ALIGN(4)
  {
    __esp_s_wpa2_rxq = .;
    KEEP(*(.sbss.s_wpa2_rxq))
    __esp_s_wpa2_rxq_end = .;

    __esp_s_wifi_wpa2_sync_sem = .;
    KEEP(*(.sbss.s_wifi_wpa2_sync_sem))
    __esp_s_wifi_wpa2_sync_sem_end = .;

    __esp_s_wpa2_queue = .;
    KEEP(*(.sbss.s_wpa2_queue))
    __esp_s_wpa2_queue_end = .;

    __esp_g_eap_sm = .;
    KEEP(*(.sbss.gEapSm))
    __esp_g_eap_sm_end = .;

    __esp_s_wpa2_data_lock = .;
    KEEP(*(.sbss.s_wpa2_data_lock))
    __esp_s_wpa2_data_lock_end = .;
  }
}
INSERT AFTER .bss;
