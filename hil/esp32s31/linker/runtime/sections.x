/* Runtime image sections and their relocation/load contracts. */
SECTIONS
{
  /* esp-riscv-rt keeps its reset initializer in the same LTO object as the
     interrupt vectors used by this independently entered runtime. Define the
     official ESP32-S31 RTC-fast zero/persistent ranges even though runtime
     handoff bypasses the normal reset entry. RTC-fast executable or initialized
     data would require a separate bootstrap copy contract and is rejected. */
  .rtc_fast.unsupported ORIGIN(RTC_FAST) (NOLOAD) :
  {
    *(.rtc_fast.literal .rtc_fast.literal.*);
    *(.rtc_fast.text .rtc_fast.text.*);
    *(.rtc_fast.data .rtc_fast.data.*);
  } > RTC_FAST

  .rtc_fast.bss (NOLOAD) :
  {
    . = ALIGN(4);
    _rtc_fast_bss_start = ABSOLUTE(.);
    *(.rtc_fast.bss .rtc_fast.bss.*);
    _rtc_fast_bss_end = ABSOLUTE(.);
  } > RTC_FAST

  .rtc_fast.persistent (NOLOAD) :
  {
    . = ALIGN(4);
    _rtc_fast_persistent_start = ABSOLUTE(.);
    *(.rtc_fast.persistent .rtc_fast.persistent.*);
    _rtc_fast_persistent_end = ABSOLUTE(.);
  } > RTC_FAST

  .runtime.header ORIGIN(RUNTIME_CODE) : ALIGN(4)
  {
    __runtime_image_start = ABSOLUTE(.);
    LONG(0x32475453);              /* "STG2" little-endian marker */
    LONG(1);                       /* bootstrap/runtime ABI version */
    LONG(__runtime_image_start);    /* required PSRAM load address */
    LONG(_runtime_start);           /* inherited-stack entry point */
    LONG(__runtime_payload_end);    /* initialized payload end VMA */
    /* Only a PSRAM BSS may be cleared by the bootstrap: an SRAM BSS can
       overlap the still-running bootstrap.  Runtime clears the real range. */
    LONG(RUNTIME_DATA_IN_PSRAM ? __runtime_data_bss_start : __runtime_payload_end);
    LONG(RUNTIME_DATA_IN_PSRAM ? __runtime_data_bss_end : __runtime_payload_end);
    LONG(44);                      /* header size in bytes */
    LONG(__runtime_text_start);     /* executable range start */
    LONG(__runtime_text_end);       /* executable range end */
    LONG(0);                       /* host packer writes payload CRC-32 */
  } > RUNTIME_CODE

  .isr.text ORIGIN(INTERNAL_LOW) :
    AT(ALIGN(LOADADDR(.runtime.header) + SIZEOF(.runtime.header), 64))
  {
    __runtime_isr_start = ABSOLUTE(.);
    KEEP(*(.trap .trap.*));
    KEEP(*(.isr.text .isr.text.*));
    KEEP(*(.flash.critical.text .flash.critical.text.*));
    *(.rwtext .rwtext.*);
    . = ALIGN(4);
    __runtime_isr_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_isr_load_start = LOADADDR(.isr.text);
  __runtime_isr_load_end = LOADADDR(.isr.text) + SIZEOF(.isr.text);

  /* Modules classify optional performance-sensitive code with semantic
     .hot.text.* input sections. The selected memory profile decides that this
     class executes from internal SRAM; this linker remains unaware of module
     names and Rust symbol mangling. */
  .hot.text ALIGN(ADDR(.isr.text) + SIZEOF(.isr.text), 64) :
    AT(ALIGN(LOADADDR(.isr.text) + SIZEOF(.isr.text), 64))
  {
    __runtime_hot_text_start = ABSOLUTE(.);
    *(.hot.text .hot.text.*);
    . = ALIGN(4);
    __runtime_hot_text_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_hot_text_load_start = LOADADDR(.hot.text);
  __runtime_hot_text_load_end = LOADADDR(.hot.text) + SIZEOF(.hot.text);

  /* Explicit ISR/flash-safe constants and initialized interrupt-owned state.
     Runtime copies this range before interrupts are enabled. */
  .critical.data ALIGN(__runtime_hot_text_end, 64) :
    AT(ALIGN(__runtime_hot_text_load_end, 64))
  {
    __runtime_critical_data_start = ABSOLUTE(.);
    KEEP(*(.isr.rodata .isr.rodata.*));
    KEEP(*(.flash.critical.rodata .flash.critical.rodata.*));
    KEEP(*(.data.critical .data.critical.*));
    KEEP(*(.critical.data .critical.data.*));
    KEEP(*(.flash.critical.data .flash.critical.data.*));
    . = ALIGN(4);
    __runtime_critical_data_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_critical_data_load_start = LOADADDR(.critical.data);
  __runtime_critical_data_load_end = LOADADDR(.critical.data) + SIZEOF(.critical.data);

  .critical.bss ALIGN(ADDR(.critical.data) + SIZEOF(.critical.data), 64) (NOLOAD) :
    AT(ALIGN(ADDR(.critical.data) + SIZEOF(.critical.data), 64))
  {
    __runtime_critical_bss_start = ABSOLUTE(.);
    KEEP(*(.critical.bss .critical.bss.*));
    KEEP(*(.flash.critical.bss .flash.critical.bss.*));
    . = ALIGN(64);
    __runtime_critical_bss_end = ABSOLUTE(.);
  } > INTERNAL_LOW

  .dma.data ALIGN(ADDR(.critical.bss) + SIZEOF(.critical.bss), 64) :
    AT(ALIGN(__runtime_critical_data_load_end, 64))
  {
    __runtime_dma_data_start = ABSOLUTE(.);
    KEEP(*(.dma.data .dma.data.*));
    . = ALIGN(4);
    __runtime_dma_data_end = ABSOLUTE(.);
  } > INTERNAL_LOW
  __runtime_dma_data_load_start = LOADADDR(.dma.data);
  __runtime_dma_data_load_end = LOADADDR(.dma.data) + SIZEOF(.dma.data);

  .dma.bss ALIGN(ADDR(.dma.data) + SIZEOF(.dma.data), 64) (NOLOAD) :
    AT(ALIGN(ADDR(.dma.data) + SIZEOF(.dma.data), 64))
  {
    __runtime_dma_bss_start = ABSOLUTE(.);
    KEEP(*(.dma.bss .dma.bss.*));
    . = ALIGN(64);
    __runtime_dma_bss_end = ABSOLUTE(.);
  } > INTERNAL_LOW

  /* Put ordinary code directly after the SRAM exception load image. VMA and
     LMA are identical for both Flash-XIP and relocated-PSRAM code, keeping
     objcopy's flat payload contiguous. */
  .text ALIGN(__runtime_dma_data_load_end, 256) : ALIGN(64)
  {
    __runtime_text_start = ABSOLUTE(.);
    KEEP(*(.text._start));
    KEEP(*(.text.runtime_trap));
    KEEP(*(.text.instruction_stream));
    *(.text .text.*);
    *(.init .init.*);
    __runtime_text_end = ABSOLUTE(.);
  } > RUNTIME_CODE

  .rodata : ALIGN(16)
  {
    *(.rodata .rodata.*);
    *(.srodata .srodata.*);
    *(.espressif.metadata .espressif.metadata.*);
  } > RUNTIME_CODE

  .data (RUNTIME_DATA_IN_PSRAM ?
         (RUNTIME_CODE_IN_PSRAM ?
          ALIGN(ADDR(.rodata) + SIZEOF(.rodata), 16) :
          ORIGIN(RUNTIME_DATA)) :
         ALIGN(__runtime_dma_bss_end, 64)) :
    AT(ALIGN(LOADADDR(.rodata) + SIZEOF(.rodata), 16))
  {
    __runtime_data_start = ABSOLUTE(.);
    __global_pointer$ = . + 0x800;
    *(.sdata .sdata.*);
    *(.rwdata .rwdata.*);
    *(.data .data.*);
    . = ALIGN(16);
    __runtime_data_end = ABSOLUTE(.);
  } > RUNTIME_DATA
  __runtime_data_load_start = LOADADDR(.data);
  __runtime_data_load_end = LOADADDR(.data) + SIZEOF(.data);

  .runtime.payload_end ALIGN(__runtime_data_load_end, 16) : ALIGN(16)
  {
    /* Keep one real byte after the final alignment. llvm-objcopy emits flat
       binaries from section contents, so an empty trailing alignment gap
       would otherwise be omitted from the embedded payload. */
    BYTE(0);
    __runtime_payload_end = ABSOLUTE(.);
  } > RUNTIME_CODE

  /* Large CPU-only buffers can stay in PSRAM even in the SRAM-data profile.
     They are deliberately NOLOAD and initialized explicitly by their owning
     Rust modules after runtime handoff. */
  .psram.noinit (RUNTIME_CODE_IN_PSRAM ?
                 ALIGN(ADDR(.runtime.payload_end) + SIZEOF(.runtime.payload_end), 64) :
                 ALIGN(ADDR(.data) + SIZEOF(.data), 64)) (NOLOAD) :
  {
    __runtime_psram_noinit_start = ABSOLUTE(.);
    KEEP(*(.psram.noinit .psram.noinit.*));
    . = ALIGN(64);
    __runtime_psram_noinit_end = ABSOLUTE(.);
  } > PSRAM_EXTERNAL

  .bss (RUNTIME_DATA_IN_PSRAM ?
        ALIGN(__runtime_psram_noinit_end, 64) :
        ALIGN(ADDR(.data) + SIZEOF(.data), 64)) (NOLOAD) :
  {
    __runtime_data_bss_start = ABSOLUTE(.);
    _bss_start = ABSOLUTE(.);
    *(.sbss .sbss.*);
    *(.bss .bss.*);
    *(.noinit .noinit.*);
    *(COMMON);
    . = ALIGN(16);
    __runtime_data_bss_end = ABSOLUTE(.);
    _bss_end = ABSOLUTE(.);
  } > RUNTIME_DATA

  __runtime_bss_start = RUNTIME_DATA_IN_PSRAM ? __runtime_data_bss_start : __runtime_payload_end;
  __runtime_bss_end = RUNTIME_DATA_IN_PSRAM ? __runtime_data_bss_end : __runtime_payload_end;
  __runtime_image_end = RUNTIME_DATA_IN_PSRAM ? __runtime_data_bss_end : __runtime_payload_end;
  __sbss = __runtime_data_bss_start;
  __ebss = __runtime_data_bss_end;

  /* The bootstrap hands the scheduler-free CPU0 Embassy executor an
     internal-SRAM stack. Keep an explicit bounded range even when every
     post-init application section is linked into PSRAM: hardware ISRs run on
     the interrupted thread-mode stack. */
  _dram_data_start = 0x2f000000;
  _stack_end = ALIGN(RUNTIME_DATA_IN_PSRAM ?
                     __runtime_dma_bss_end :
                     __runtime_data_bss_end, 64);
  _stack_end_cpu0 = _stack_end;
  _stack_start = 0x2f07afc0;
  _stack_start_cpu0 = _stack_start;

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.*);
    *(.eh_frame_hdr);
  }
}

ASSERT(__runtime_payload_end <= ORIGIN(RUNTIME_CODE) + LENGTH(RUNTIME_CODE),
       "runtime initialized payload does not fit selected code region");
ASSERT(SIZEOF(.rtc_fast.unsupported) == 0,
       "runtime RTC-fast code/data requires an explicit bootstrap copy contract");
ASSERT(__runtime_psram_noinit_end <= ORIGIN(PSRAM_EXTERNAL) + LENGTH(PSRAM_EXTERNAL),
       "PSRAM runtime explicit no-init storage does not fit");
ASSERT((RUNTIME_DATA_IN_PSRAM &&
        __runtime_data_bss_end <= ORIGIN(PSRAM_EXTERNAL) + LENGTH(PSRAM_EXTERNAL)) ||
       (!RUNTIME_DATA_IN_PSRAM &&
        __runtime_data_bss_end <= ORIGIN(RUNTIME_DATA) + LENGTH(RUNTIME_DATA)),
       "runtime data/BSS does not fit selected memory profile");
ASSERT(__runtime_data_load_start >= ORIGIN(RUNTIME_CODE) &&
       __runtime_data_load_end <= ORIGIN(RUNTIME_CODE) + LENGTH(RUNTIME_CODE),
       "runtime initialized data load image is outside payload");
ASSERT(__runtime_isr_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "PSRAM runtime interrupt code does not fit internal SRAM");
ASSERT(__runtime_hot_text_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime hot code does not fit internal SRAM");
ASSERT(__runtime_critical_data_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime initialized interrupt state does not fit internal SRAM");
ASSERT(__runtime_critical_bss_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "PSRAM runtime critical state does not fit low internal SRAM");
ASSERT(__runtime_dma_data_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "runtime initialized DMA state does not fit internal SRAM");
ASSERT(__runtime_dma_bss_end <= ORIGIN(INTERNAL_LOW) + LENGTH(INTERNAL_LOW),
       "PSRAM runtime DMA storage does not fit internal SRAM");
ASSERT((__runtime_isr_load_start & 3) == 0 &&
       ((__runtime_isr_end - __runtime_isr_start) & 3) == 0,
       "PSRAM runtime interrupt copy must be word aligned");
ASSERT((__runtime_hot_text_load_start & 3) == 0 &&
       ((__runtime_hot_text_end - __runtime_hot_text_start) & 3) == 0,
       "runtime hot-code copy must be word aligned");
ASSERT((__runtime_dma_bss_start & 3) == 0 &&
       ((__runtime_dma_bss_end - __runtime_dma_bss_start) & 3) == 0,
       "PSRAM runtime DMA BSS must be word aligned");
ASSERT((__runtime_critical_bss_start & 3) == 0 &&
       ((__runtime_critical_bss_end - __runtime_critical_bss_start) & 3) == 0,
       "PSRAM runtime critical BSS must be word aligned");
ASSERT((__runtime_critical_data_load_start & 3) == 0 &&
       ((__runtime_critical_data_end - __runtime_critical_data_start) & 3) == 0,
       "runtime initialized interrupt state copy must be word aligned");
ASSERT((__runtime_dma_data_load_start & 3) == 0 &&
       ((__runtime_dma_data_end - __runtime_dma_data_start) & 3) == 0,
       "runtime initialized DMA state copy must be word aligned");
ASSERT(_stack_start - _stack_end >= 0x10000,
       "PSRAM runtime leaves less than 64 KiB for the CPU0 stack");
ASSERT(_runtime_start >= __runtime_text_start && _runtime_start < __runtime_text_end,
       "runtime entry point is outside executable text");
