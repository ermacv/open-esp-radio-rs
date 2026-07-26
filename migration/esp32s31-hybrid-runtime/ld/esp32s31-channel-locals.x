/* Stable alias for the pinned local scan channel-completion callback. */
EXTERN(ieee80211_scan_attach)

SECTIONS
{
  .text.esp_wifi_async_channel_locals : ALIGN(2)
  {
    __esp_scan_op_end = .;
    KEEP(*(.text.scan_op_end))
    __esp_scan_op_end_end = .;
  } > ROTEXT
}
INSERT BEFORE .text;

/* Flash placement permits RISC-V call relaxation while the pinned input
 * section itself remains covered by the archive audit. */
/* Interposing the SRAM PM/coex leaf keeps its call sequence long; the
 * non-interposed and earlier strict links remain accepted for archive-audit
 * tooling. */
ASSERT(__esp_scan_op_end_end - __esp_scan_op_end == 0x26e ||
       __esp_scan_op_end_end - __esp_scan_op_end == 0x22e ||
       __esp_scan_op_end_end - __esp_scan_op_end == 0x232,
       "ESP32-S31 scan_op_end ABI changed");
