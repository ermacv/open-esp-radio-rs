/* Physical address spaces used by the independently linked runtime image.
   Addresses and sizes are `oer-esp32s31-platform-layout` symbols. */
MEMORY
{
  /* All application-owned internal SRAM is one arena. Section placement is
     determined only by explicitly SRAM-qualified ISR, hot, critical and DMA
     inputs. Linker assertions retain only a bounded transient bootstrap
     handoff margin. */
  INTERNAL_LOW (RWX) : ORIGIN = SRAM_ORIGIN, LENGTH = SRAM_LENGTH
  RTC_FAST (RWX) : ORIGIN = 0x2E000000, LENGTH = 0x00008000

  /* Stage two after the bootstrap's PSRAM probe page: the relocated
     header, code, read-only and initialized data, then task stacks and BSS. */
  PSRAM_RUNTIME (RWX) : ORIGIN = RUNTIME_PSRAM_ORIGIN, LENGTH = RUNTIME_PSRAM_LENGTH
}
