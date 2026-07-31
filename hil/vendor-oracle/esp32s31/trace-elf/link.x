ENTRY(_start)

MEMORY
{
  TRACE (rwx) : ORIGIN = 0x10000000, LENGTH = 4M
}

SECTIONS
{
  .text : ALIGN(4)
  {
    KEEP(*(.text._start))
    *(.literal .literal.*)
    *(.text .text.*)
  } > TRACE

  .note.open_esp_radio.oracle : ALIGN(4)
  {
    KEEP(*(.note.open_esp_radio.oracle))
  } > TRACE

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
    *(.srodata .srodata.*)
  } > TRACE

  .data : ALIGN(4)
  {
    *(.sdata .sdata.*)
    *(.data .data.*)
  } > TRACE

  .bss (NOLOAD) : ALIGN(4)
  {
    *(.sbss .sbss.*)
    *(.bss .bss.*)
    *(COMMON)
  } > TRACE

  /DISCARD/ :
  {
    *(.eh_frame .eh_frame.*)
  }
}
