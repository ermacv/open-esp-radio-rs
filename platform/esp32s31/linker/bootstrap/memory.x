/* Physical address spaces used by the Flash-resident bootstrap image.
   Addresses and sizes are `oer-esp32s31-platform-layout` symbols. */
MEMORY
{
    /* ESP32-S31 has unified instruction/data SRAM.  ESP-IDF uses the same
       region for iram_text_seg and dram_seg, so RAM-resident code consumes
       only its actual section size instead of a fixed IRAM reservation.

       The SRAM end is ESP-IDF's SRAM_SEG_END: the memory above it is retained
       for the second-stage loader and the ROM boot stack. */
    SRAM   (RWX) : ORIGIN = SRAM_ORIGIN, LENGTH = SRAM_LENGTH

    /* ESP32-S31 has one unified 64 MiB flash-mapped instruction/data window. */
    ROTEXT  (RX) : ORIGIN = BOOTSTRAP_FLASH_TEXT_ORIGIN, LENGTH = BOOTSTRAP_FLASH_TEXT_LENGTH

    /* Cached external RAM aperture. The board currently has 16 MiB fitted.
       Explicit PSRAM sections occupy a prefix; the remainder stays available
       to the application as ordinary external memory. */
    PSRAM   (RWX) : ORIGIN = PSRAM_ORIGIN, LENGTH = PSRAM_LENGTH
}

REGION_ALIAS("RODATA", ROTEXT);
REGION_ALIAS("RWTEXT", SRAM);
REGION_ALIAS("RWDATA", SRAM);
REGION_ALIAS("RTC_FAST_RWTEXT", SRAM);
REGION_ALIAS("RTC_FAST_RWDATA", SRAM);
