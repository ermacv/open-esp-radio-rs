/* PSRAM destinations reserved and initialized by bootstrap before handoff. */
SECTIONS
{
  .psram.runtime (NOLOAD) : ALIGN(16)
  {
    __psram_init_start = ABSOLUTE(.);
    __psram_text_start = ABSOLUTE(.);
    . = . + __psram_text_size;
    __psram_text_end = ABSOLUTE(.);
    . = ALIGN(16);
    __psram_rodata_start = ABSOLUTE(.);
    . = . + __psram_rodata_size;
    __psram_rodata_end = ABSOLUTE(.);
    . = ALIGN(16);
    __psram_data_start = ABSOLUTE(.);
    . = . + __psram_data_size;
    __psram_data_end = ABSOLUTE(.);
    . = ALIGN(16);
    __psram_init_end = ABSOLUTE(.);
    __psram_bss_start = ABSOLUTE(.);
    KEEP(*(.psram.bss .psram.bss.*));
    . = ALIGN(16);
    __psram_bss_end = ABSOLUTE(.);
    __psram_noinit_start = ABSOLUTE(.);
    KEEP(*(.psram.noinit .psram.noinit.*));
    . = ALIGN(16);
    __psram_noinit_end = ABSOLUTE(.);
  } > PSRAM
  __psram_reserved_end = ALIGN(0x10000);
}
