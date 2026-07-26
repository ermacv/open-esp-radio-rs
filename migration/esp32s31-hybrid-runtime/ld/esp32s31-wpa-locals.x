/*
 * Link-time aliases for local functions in the audited ESP32-S31 WPA blob.
 *
 * This script does not alter libwpa_supplicant.a. It gives a stable external
 * name to the beginning of a function-section already selected from wpa.c.obj.
 * The xtask audit verifies the archive digest and the 0x146-byte function size.
 */
EXTERN(wpa_michael_mic_failure)

SECTIONS
{
  .text.esp_wifi_async_wpa_locals : ALIGN(2)
  {
    __esp_wpa_sm_key_request = .;
    KEEP(*(.text.wpa_sm_key_request))
    __esp_wpa_sm_key_request_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;
