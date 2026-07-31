PROVIDE(interrupt0 = DefaultHandler);

/* Text and constants share the same virtual flash window. Reserve the pages
   occupied by rodata before placing executable text into that window. */
SECTIONS {
  .rotext_dummy (NOLOAD) :
  {
    . = ALIGN(ALIGNOF(.rodata));
    . = ALIGN(ALIGNOF(.rodata.wifi));
    . = . + SIZEOF(.flash.appdesc);
    . = . + SIZEOF(.rodata);
    . = . + SIZEOF(.rodata.wifi);
    /* Keep XIP text on the same 64 KiB MMU page boundary encoded in the
       second-stage bootloader and application descriptor. */
    . = ALIGN(0x10000) + 0x20;
  } > ROTEXT
}
INSERT BEFORE .text;

SECTIONS {
  INCLUDE "rwtext.x"

  /* Runtime destination for the canonical benchmark stream. NOLOAD keeps
     the ESP image loader from treating it as initialized data. */
  .code_bench.iram (NOLOAD) : ALIGN(64)
  {
    __code_bench_iram_start = ABSOLUTE(.);
    . = . + (__code_bench_flash_end - __code_bench_flash_start);
    __code_bench_iram_end = ABSOLUTE(.);
  } > RWTEXT

  INCLUDE "rwdata.x"
}

ASSERT(__code_bench_iram_end <= ORIGIN(RWTEXT) + LENGTH(RWTEXT),
       "code benchmark does not fit in internal instruction SRAM");

INCLUDE "bootstrap/flash-sections.x"
INCLUDE "text.x"
PROVIDE(__flash_text_start = ADDR(.text));
PROVIDE(__flash_text_end = ADDR(.text) + SIZEOF(.text));
INCLUDE "bootstrap/psram-sections.x"
INCLUDE "rtc_fast.x"
INCLUDE "stack.x"
INCLUDE "metadata.x"
INCLUDE "eh_frame.x"

/* The trap prologue uses this exact boundary to distinguish an invalid stack
   in executable SRAM from a valid stack in the data portion of unified SRAM. */
_dram_data_start = ADDR(.data);
INCLUDE "hal-defaults.x"
