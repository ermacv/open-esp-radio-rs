/* Flash-mapped metadata, constants and relocation payloads owned by bootstrap. */
SECTIONS {
  .flash.appdesc : ALIGN(4)
  {
      KEEP(*(.flash.appdesc));
      KEEP(*(.flash.appdesc.*));
  } > RODATA

  .rodata_merge : ALIGN(4) {
    . = ALIGN(ALIGNOF(.rodata));
  } > RODATA

  .rodata : ALIGN(4)
  {
    . = ALIGN(4);
    _rodata_start = ABSOLUTE(.);

    /* The code-location benchmark emits one canonical instruction stream.
       It executes in place here and is copied byte-for-byte to IRAM and
       PSRAM, so all three measurements use identical machine code. */
    . = ALIGN(64);
    __code_bench_flash_start = ABSOLUTE(.);
    KEEP(*(.code_bench.source));
    __code_bench_flash_end = ABSOLUTE(.);

    /* A memory-profile runtime image is carried as ordinary Flash rodata.
       PSRAM-code profiles copy it after external-memory initialization;
       Flash-code profiles execute these same bytes directly through XIP. */
    . = ALIGN(64);
    __psram_runtime_payload_flash_start = ABSOLUTE(.);
    KEEP(*(.psram.runtime.payload));
    __psram_runtime_payload_flash_end = ABSOLUTE(.);

    /* Flash::tune_120mhz consumes cold XIP pages. Runtime copying happens
       first, so keep a disjoint page-aligned reference span for the sweep. */
    . = ALIGN(4096);
    __flash_tuning_reference_start = ABSOLUTE(.);
    KEEP(*(.flash.tuning.reference));
    __flash_tuning_reference_end = ABSOLUTE(.);

    *(.rodata .rodata.*)
    *(.srodata .srodata.*)

    /* Store the external-memory initializers in a normal flash-mapped
       segment. The second-stage bootloader must not load 0x50000000. */
    . = ALIGN(16);
    __psram_init_load = ABSOLUTE(.);
    __psram_text_load = ABSOLUTE(.);
    KEEP(*(.psram.text .psram.text.*));
    __psram_text_load_end = ABSOLUTE(.);
    __psram_text_size = __psram_text_load_end - __psram_text_load;
    . = ALIGN(16);
    __psram_rodata_load = ABSOLUTE(.);
    KEEP(*(.psram.rodata .psram.rodata.*));
    __psram_rodata_load_end = ABSOLUTE(.);
    __psram_rodata_size = __psram_rodata_load_end - __psram_rodata_load;
    . = ALIGN(16);
    __psram_data_load = ABSOLUTE(.);
    KEEP(*(.psram.data.init .psram.data.init.*));
    __psram_data_load_end = ABSOLUTE(.);
    __psram_data_size = __psram_data_load_end - __psram_data_load;
    . = ALIGN(16);
    __psram_init_load_end = ABSOLUTE(.);
    _rodata_end = ABSOLUTE(.);
  } > RODATA

  .rodata.wifi : ALIGN(4)
  {
    . = ALIGN(4);
    *(.rodata_wlog_*.*)
    . = ALIGN(4);
  } > RODATA
}

/* The Flash-code runtime is linked to execute directly from its bytes inside
   the bootstrap image. Keep this address stable whenever a payload exists. */
ASSERT((__psram_runtime_payload_flash_end == __psram_runtime_payload_flash_start) ||
       (__psram_runtime_payload_flash_start == 0x40000140),
       "embedded runtime payload moved from its fixed Flash XIP address");
ASSERT((__flash_tuning_reference_start & 0xfff) == 0 &&
       (__flash_tuning_reference_end - __flash_tuning_reference_start) >= 0x10000,
       "Flash tuning reference must contain sixteen aligned 4-KiB pages");
