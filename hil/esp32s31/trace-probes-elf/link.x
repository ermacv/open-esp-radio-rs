ENTRY(_start)

MEMORY
{
  TRACE (rwx) : ORIGIN = 0x10000000, LENGTH = 2M
}

SECTIONS
{
  .text : ALIGN(4)
  {
    KEEP(*(.text._start))
    KEEP(*(.text.open_phy_trace_*))
    *(.text .text.*)
  } > TRACE

  .rodata : ALIGN(4)
  {
    *(.rodata .rodata.*)
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
