/* Physical address spaces used by the independently linked runtime image. */
MEMORY
{
  /* For the SRAM-data profile this aliases the complete low SRAM arena. The
     output sections pack ISR, critical state and ordinary mutable state by
     their actual sizes instead of reserving fixed partitions. */
  RUNTIME_DATA (RWX) : ORIGIN = RUNTIME_DATA_ORIGIN, LENGTH = RUNTIME_DATA_LENGTH

  /* Bootstrap hands off a stack at the physical SRAM top. Reserve its lowest
     64 KiB boundary and pack every runtime SRAM section below it. */
  INTERNAL_LOW (RWX) : ORIGIN = 0x2F000000, LENGTH = 0x0006AFC0
  RTC_FAST (RWX) : ORIGIN = 0x2E000000, LENGTH = 0x00008000

  /* xtask selects ordinary code as Flash XIP at the embedded-payload address
     or relocated PSRAM starting after the bootstrap page. */
  RUNTIME_CODE (RX) : ORIGIN = RUNTIME_CODE_ORIGIN, LENGTH = RUNTIME_CODE_LENGTH
  PSRAM_EXTERNAL (RWX) : ORIGIN = 0x50010000, LENGTH = 0x00FF0000
}
