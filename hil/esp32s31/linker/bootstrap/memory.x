/* Physical address spaces used by the Flash-resident bootstrap image. */
MEMORY
{
    /* ESP32-S31 has unified instruction/data SRAM.  ESP-IDF uses the same
       region for iram_text_seg and dram_seg, so RAM-resident code consumes
       only its actual section size instead of a fixed IRAM reservation.

       0x2F07AFC0 is ESP-IDF's SRAM_SEG_END: the memory above it is retained
       for the second-stage loader and the ROM boot stack. */
    SRAM   (RWX) : ORIGIN = 0x2F000000, LENGTH = 0x0007AFC0

    /* ESP32-S31 has one unified 64 MiB flash-mapped instruction/data window. */
    ROTEXT  (RX) : ORIGIN = 0x40000020, LENGTH = 0x03FFFFE0

    /* Cached external RAM aperture. The board currently has 16 MiB fitted.
       Explicit PSRAM sections occupy a prefix; the remainder stays available
       to the application as ordinary external memory. */
    PSRAM   (RWX) : ORIGIN = 0x50000000, LENGTH = 0x01000000
}

REGION_ALIAS("RODATA", ROTEXT);
REGION_ALIAS("RWTEXT", SRAM);
REGION_ALIAS("RWDATA", SRAM);
REGION_ALIAS("RTC_FAST_RWTEXT", SRAM);
REGION_ALIAS("RTC_FAST_RWDATA", SRAM);
