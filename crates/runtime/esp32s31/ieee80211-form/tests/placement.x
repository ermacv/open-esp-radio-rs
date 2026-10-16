/* Link-only S31 form fixture, not a boot image.
 * Physical ranges are platform/esp32s31/chip.toml [memory]. The section
 * spellings are platform/espressif/linker/runtime/link.x's IRQ/critical/DMA
 * contract. Ordinary code/data are deliberately cached so a missing IRQ
 * dependency placement fails the final ELF audit. */
ENTRY(_start)
MEMORY {
  SRAM (rwx) : ORIGIN = 0x2f000000, LENGTH = 0x7afc0
  CACHED (rwx) : ORIGIN = 0x50010000, LENGTH = 0xff0000
}
SECTIONS {
  .rwtext : ALIGN(4) { *(.rwtext .rwtext.*) } > SRAM
  .critical.rodata : ALIGN(4) { *(.critical.rodata .critical.rodata.*) } > SRAM
  .scope.backing : ALIGN(4) { *(.critical.data.open_radio_scope) } > SRAM
  .critical.data : ALIGN(4) { *(.critical.data .critical.data.*) } > SRAM
  .dma (NOLOAD) : ALIGN(16) { *(.dma.bss .dma.bss.*) } > SRAM
  .scope.stack (NOLOAD) : ALIGN(16) { __scope_stack_bottom = .; . += 4096; __scope_stack_top = .; } > SRAM
  .text : ALIGN(4) { *(.text .text.*) } > CACHED
  .rodata : ALIGN(4) { *(.rodata .rodata.*) } > CACHED
  .data : ALIGN(4) { *(.sdata .sdata.* .data .data.*) } > CACHED
  .bss (NOLOAD) : ALIGN(4) { *(.sbss .sbss.* .bss .bss.* COMMON) } > CACHED
  /DISCARD/ : { *(.eh_frame .eh_frame.*) }
}
